/******************************************************************************
 * Copyright © 2014-2018 The SuperNET Developers.                             *
 *                                                                            *
 * See the AUTHORS, DEVELOPER-AGREEMENT and LICENSE files at                  *
 * the top-level directory of this distribution for the individual copyright  *
 * holder information and the developer policies on copyright and licensing.  *
 *                                                                            *
 * Unless otherwise agreed in a custom licensing agreement, no part of the    *
 * SuperNET software, including this file may be copied, modified, propagated *
 * or distributed except according to the terms contained in the LICENSE file *
 *                                                                            *
 * Removal or modification of this copyright notice is prohibited.            *
 *                                                                            *
 ******************************************************************************/
//
//  utxo.rs
//  marketmaker
//
//  Copyright © 2017-2018 SuperNET. All rights reserved.
//
pub mod rpc_clients;

use base64::{encode_config as base64_encode, URL_SAFE};
use bitcrypto::{dhash160};
use byteorder::{LittleEndian, WriteBytesExt};
use chain::{TransactionOutput, TransactionInput, OutPoint, Transaction as UtxoTransaction};
use chain::constants::{SEQUENCE_FINAL};
use common::{dstr, lp, MutexGuardWrapper};
use futures::{Future};
use gstuff::now_ms;
use keys::{KeyPair, Private, Public, Address, Secret};
use keys::bytes::Bytes;
use keys::generator::{Random, Generator};
use primitives::hash::{H256, H264, H512};
use script::{Opcode, Builder, Script, TransactionInputSigner, UnsignedTransactionInput, SignatureVersion};
use serialization::{serialize, deserialize};
use sha2::{Sha256, Digest};
use std::borrow::Cow;
use std::convert::AsMut;
use std::ffi::CStr;
use std::mem::transmute;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use self::rpc_clients::{UtxoRpcClientEnum, UnspentInfo, ElectrumClient, ElectrumClientImpl, NativeClient};
use super::{IguanaInfo, MarketCoinOps, MmCoin, MmCoinEnum, SwapOps, Transaction, TransactionEnum, TransactionFut};

/// Clones slice into fixed size array
/// https://stackoverflow.com/a/37682288/8707622
fn clone_into_array<A: Default + AsMut<[T]>, T: Clone>(slice: &[T]) -> A {
    let mut a = Default::default();
    <A as AsMut<[T]>>::as_mut(&mut a).clone_from_slice(slice);
    a
}

/// Extended UTXO transaction, contains redeem script to spend p2sh output
/// Every transaction should contain separate redeem script for every p2sh output
/// However as MM creates only 1 p2sh output per every swap transaction
/// we can use single redeem script at least for now.
#[derive(Debug, Clone)]
pub struct ExtendedUtxoTx {
    pub transaction: UtxoTransaction,
    pub redeem_script: Bytes,
}

impl ExtendedUtxoTx {
    pub fn transaction_bytes(&self) -> Bytes {
        serialize(&self.transaction)
    }
}

impl Transaction for ExtendedUtxoTx {
    fn to_raw_bytes(&self) -> Vec<u8> {
        let mut resulting_bytes = vec![];
        let tx_bytes = serialize(&self.transaction);
        let tx_len_bytes: [u8; 4] = unsafe { transmute(tx_bytes.len() as u32) };
        resulting_bytes.extend_from_slice(&tx_len_bytes);
        resulting_bytes.extend_from_slice(&tx_bytes);
        let redeem_len_bytes: [u8; 4] = unsafe { transmute(self.redeem_script.len() as u32) };
        resulting_bytes.extend_from_slice(&redeem_len_bytes);
        resulting_bytes.extend_from_slice(&self.redeem_script);
        resulting_bytes
    }

    fn extract_secret(&self) -> Result<Vec<u8>, String> {
        let script: Script = self.transaction.inputs[0].script_sig.clone().into();
        for (i, instr) in script.iter().enumerate() {
            let instruction = instr.unwrap();
            if i == 1 {
                if instruction.opcode == Opcode::OP_PUSHBYTES_32 {
                    return Ok(instruction.data.unwrap().to_vec());
                }
            }
        }
        ERR!("Couldn't extract secret")
    }

    fn tx_hash(&self) -> String {
        format!("{}", self.transaction.hash().reversed())
    }
}

#[derive(Debug)]
pub struct UtxoCoinImpl {  // pImpl idiom.
    ticker: String,
    /// https://en.bitcoin.it/wiki/List_of_address_prefixes
    /// https://github.com/jl777/coins/blob/master/coins
    pub_addr_prefix: u8,
    p2sh_addr_prefix: u8,
    wif_prefix: u8,
    pub_t_addr_prefix: u8,
    p2sh_t_addr_prefix: u8,
    /// True if coins uses Proof of Stake consensus algo
    /// Proof of Work is expected by default
    /// https://en.bitcoin.it/wiki/Proof_of_Stake
    /// https://en.bitcoin.it/wiki/Proof_of_work
    is_pos: bool,
    /// Special field for Zcash and it's forks
    /// Defines if Overwinter network upgrade was activated
    /// https://z.cash/upgrade/overwinter/
    overwintered: bool,
    /// The tx version used to detect the transaction ser/de/signing algo
    /// For now it's mostly used for Zcash and forks because they changed the algo in
    /// Overwinter and then Sapling upgrades
    /// https://github.com/zcash/zips/blob/master/zip-0243.rst
    tx_version: i32,
    /// If true - use Segwit protocol
    /// https://en.bitcoin.it/wiki/Segregated_Witness
    segwit: bool,
    /// Default decimals amount is 8 (BTC and almost all other UTXO coins)
    /// But there are forks which have different decimals:
    /// Peercoin has 6
    /// Emercoin has 6
    /// Bitcoin Diamond has 7
    decimals: u8,
    /// Is coin protected by Komodo dPoW?
    /// https://komodoplatform.com/security-delayed-proof-of-work-dpow/
    notarized: bool,
    /// RPC port
    rpc_port: u16,
    /// RPC username
    rpc_user: String,
    /// RPC password
    rpc_password: String,
    /// RPC client
    rpc_client: UtxoRpcClientEnum,
    /// ECDSA key pair
    key_pair: KeyPair,
    /// Lock the mutex when we deal with address utxos
    my_address: Address,
    /// Is current coin KMD asset chain?
    /// https://komodoplatform.atlassian.net/wiki/spaces/KPSD/pages/71729160/What+is+a+Parallel+Chain+Asset+Chain
    asset_chain: bool,
    tx_fee: u64,
}

/// Generates unsigned transaction (TransactionInputSigner) from specified utxos and outputs.
/// This function expects that utxos are sorted by amounts in ascending order
/// Consider sorting before calling this function
fn generate_transaction(
    utxos: Vec<UnspentInfo>,
    mut outputs: Vec<TransactionOutput>,
    lock_time: u32,
    version: i32,
    overwintered: bool,
    tx_fee: u64,
    decimals: u8,
    change_script_pubkey: Bytes,
) -> Result<TransactionInputSigner, String> {
    if utxos.is_empty() {
        return ERR!("Couldn't generate tx from empty utxos set");
    }

    if outputs.is_empty() {
        return ERR!("Couldn't generate tx from empty outputs set");
    }

    let mut target_value = 0;
    for output in outputs.iter() {
        target_value += output.value;
    }

    if target_value == 0 {
        return ERR!("Total target value calculated from outputs {:?} is zero", outputs);
    }
    target_value += tx_fee;

    let mut value_to_spend = 0;
    let mut inputs = vec![];
    for utxo in utxos.iter() {
        value_to_spend += utxo.value;
        inputs.push(UnsignedTransactionInput {
            previous_output: utxo.outpoint.clone(),
            sequence: SEQUENCE_FINAL,
            amount: utxo.value,
        });
        if value_to_spend >= target_value { break; }
    }

    if value_to_spend < target_value {
        return ERR!("Couldn't collect enough value from utxos {:?} to create tx with outputs {:?}", utxos, outputs);
    }

    if value_to_spend > target_value {
        outputs.push({
            TransactionOutput {
                value: value_to_spend - target_value,
                script_pubkey: change_script_pubkey
            }
        });
    }

    let mut version_group_id = 0;
    if version == 3 {
        version_group_id = 0x03c48270;
    } else if version == 4 {
        version_group_id = 0x892f2085;
    }

    Ok(TransactionInputSigner {
        inputs,
        outputs,
        lock_time,
        version,
        overwintered,
        expiry_height: 0,
        join_splits: vec![],
        shielded_spends: vec![],
        shielded_outputs: vec![],
        value_balance: 0,
        version_group_id,
    })
}

fn payment_script(
    time_lock: u32,
    secret_hash: &[u8],
    pub_0: &Public,
    pub_1: &Public
) -> Result<Script, String> {
    let builder = Builder::default();
    let mut wtr = vec![];
    try_s!(wtr.write_u32::<LittleEndian>(time_lock));
    Ok(builder
        .push_opcode(Opcode::OP_IF)
        .push_bytes(&wtr)
        .push_opcode(Opcode::OP_CHECKLOCKTIMEVERIFY)
        .push_opcode(Opcode::OP_DROP)
        .push_bytes(pub_0)
        .push_opcode(Opcode::OP_CHECKSIG)
        .push_opcode(Opcode::OP_ELSE)
        .push_opcode(Opcode::OP_SIZE)
        .push_bytes(&[32])
        .push_opcode(Opcode::OP_EQUALVERIFY)
        .push_opcode(Opcode::OP_HASH160)
        .push_bytes(secret_hash)
        .push_opcode(Opcode::OP_EQUALVERIFY)
        .push_bytes(pub_1)
        .push_opcode(Opcode::OP_CHECKSIG)
        .push_opcode(Opcode::OP_ENDIF)
        .into_script())
}

fn script_sig(message: &H256, key_pair: &KeyPair) -> Result<Bytes, String> {
    let signature = try_s!(key_pair.private().sign(message));

    let mut sig_script = Bytes::default();
    sig_script.append(&mut Bytes::from((*signature).to_vec()));
    // Using SIGHASH_ALL only for now
    sig_script.append(&mut Bytes::from(vec![1]));

    Ok(sig_script)
}

fn script_sig_with_pub(message: &H256, key_pair: &KeyPair) -> Result<Bytes, String> {
    let sig_script = try_s!(script_sig(message, key_pair));

    let builder = Builder::default();

    Ok(builder
        .push_data(&sig_script)
        .push_data(&key_pair.public().to_vec())
        .into_bytes())
}

/// Creates signed input spending p2pkh output
fn p2pkh_spend(
    signer: &TransactionInputSigner,
    input_index: usize,
    key_pair: &KeyPair,
    prev_script: &Script
) -> Result<TransactionInput, String> {
    let script = Builder::build_p2pkh(&key_pair.public().address_hash());
    if script != *prev_script {
        return ERR!("p2pkh script {} built from input key pair doesn't match expected prev script {}", script, prev_script);
    }

    let sighash = signer.signature_hash(input_index, 0, &script, SignatureVersion::Base, 1);

    let script_sig = try_s!(script_sig_with_pub(&sighash, key_pair));

    Ok(TransactionInput {
        script_sig,
        sequence: signer.inputs[input_index].sequence,
        script_witness: vec![],
        previous_output: signer.inputs[input_index].previous_output.clone()
    })
}

/// Creates signed input spending p2sh output
fn p2sh_spend(
    signer: &TransactionInputSigner,
    input_index: usize,
    key_pair: &KeyPair,
    script_data: Script,
    redeem_script: Script,
) -> Result<TransactionInput, String> {
    let sighash = signer.signature_hash(input_index, 0, &redeem_script, SignatureVersion::Base, 1);

    let sig = try_s!(script_sig(&sighash, &key_pair));

    let mut resulting_script = Builder::default().push_data(&sig).into_bytes();
    if !script_data.is_empty() {
        resulting_script.extend_from_slice(&script_data);
    }

    let redeem_part = Builder::default().push_data(&redeem_script).into_bytes();
    resulting_script.extend_from_slice(&redeem_part);

    Ok(TransactionInput {
        script_sig: resulting_script.into(),
        sequence: signer.inputs[input_index].sequence,
        script_witness: vec![],
        previous_output: signer.inputs[input_index].previous_output.clone()
    })
}

fn p2sh_spending_tx(
    prev_transaction: ExtendedUtxoTx,
    outputs: Vec<TransactionOutput>,
    script_data: Script,
    key_pair: &KeyPair,
    version: i32,
    overwintered: bool,
    lock_time: u32,
    sequence: u32,
) -> Result<UtxoTransaction, String> {
    let mut version_group_id = 0;
    if version == 3 {
        version_group_id = 0x03c48270;
    } else if version == 4 {
        version_group_id = 0x892f2085;
    }

    let unsigned = TransactionInputSigner {
        lock_time,
        version,
        overwintered,
        inputs: vec![UnsignedTransactionInput {
            sequence,
            previous_output: OutPoint {
                hash: prev_transaction.transaction.hash(),
                index: 0,
            },
            amount: prev_transaction.transaction.outputs[0].value,
        }],
        outputs: outputs.clone(),
        expiry_height: 0,
        join_splits: vec![],
        shielded_spends: vec![],
        shielded_outputs: vec![],
        value_balance: 0,
        version_group_id,
    };
    let signed_input = try_s!(
        p2sh_spend(&unsigned, 0, key_pair, script_data, prev_transaction.redeem_script.into())
    );
    Ok(UtxoTransaction {
        version: unsigned.version,
        overwintered: unsigned.overwintered,
        lock_time: unsigned.lock_time,
        inputs: vec![signed_input],
        outputs,
        expiry_height: unsigned.expiry_height,
        join_splits: vec![],
        shielded_spends: vec![],
        shielded_outputs: vec![],
        value_balance: 0,
        version_group_id: unsigned.version_group_id,
        binding_sig: H512::default(),
        join_split_sig: H512::default(),
        join_split_pubkey: H256::default(),
    })
}

fn address_from_raw_pubkey(pub_key: &[u8], prefix: u8, t_addr_prefix: u8) -> Result<Address, String> {
    Ok(Address {
        t_addr_prefix,
        prefix,
        hash: try_s!(Public::from_slice(pub_key)).address_hash(),
    })
}

fn sign_tx(
    unsigned: TransactionInputSigner,
    key_pair: &KeyPair,
    prev_script: Script
) -> Result<UtxoTransaction, String> {
    let mut signed_inputs = vec![];
    for (i, _) in unsigned.inputs.iter().enumerate() {
        signed_inputs.push(
            try_s!(p2pkh_spend(&unsigned, i, key_pair, &prev_script))
        );
    }
    Ok(UtxoTransaction {
        inputs: signed_inputs,
        outputs: unsigned.outputs.clone(),
        version: unsigned.version,
        overwintered: unsigned.overwintered,
        lock_time: unsigned.lock_time,
        expiry_height: unsigned.expiry_height,
        join_splits: vec![],
        shielded_spends: vec![],
        shielded_outputs: vec![],
        value_balance: 0,
        version_group_id: unsigned.version_group_id,
        binding_sig: H512::default(),
        join_split_sig: H512::default(),
        join_split_pubkey: H256::default(),
    })
}

#[derive(Clone, Debug)]
pub struct UtxoCoin(Arc<UtxoCoinImpl>);
impl Deref for UtxoCoin {type Target = UtxoCoinImpl; fn deref (&self) -> &UtxoCoinImpl {&*self.0}}

// We can use a shared UTXO lock for all UTXO coins at 1 time.
// It's highly likely that we won't experience any issues with it as we won't need to send "a lot" of transactions concurrently.
lazy_static! {static ref UTXO_LOCK: Mutex<()> = Mutex::new(());}

impl UtxoCoin {
    fn send_outputs_from_my_address(&self, outputs: Vec<TransactionOutput>, redeem_script: Bytes) -> TransactionFut {
        let change_script_pubkey = Builder::build_p2pkh(&self.key_pair.public().address_hash()).to_bytes();
        let arc = self.clone();
        let utxo_lock = MutexGuardWrapper(try_fus!(UTXO_LOCK.lock()));
        let unspent_fut = self.rpc_client.list_unspent_ordered(&self.my_address);
        Box::new(unspent_fut.then(move |unspents| -> TransactionFut {
            let unspents = try_fus!(unspents);
            let unsigned = try_fus!(generate_transaction(
                unspents,
                outputs,
                0,
                arc.tx_version,
                arc.overwintered,
                1000,
                arc.decimals,
                change_script_pubkey.clone()
            ));

            let signed = try_fus!(sign_tx(unsigned, &arc.key_pair, change_script_pubkey.into()));
            let tx = ExtendedUtxoTx {
                transaction: signed,
                redeem_script
            };
            Box::new(arc.send_raw_tx(tx.into()).then(move |res| {
                // Drop the UTXO lock only when the transaction send result is known.
                drop(utxo_lock);
                res
            }))
        }))
    }

    fn validate_payment(
        &self,
        payment_tx: TransactionEnum,
        time_lock: u32,
        first_pub0: &[u8],
        second_pub0: &[u8],
        _other_addr: &[u8],
        priv_bn_hash: &[u8],
        amount: u64,
    ) -> Result<(), String> {
        let tx = match payment_tx {
            TransactionEnum::ExtendedUtxoTx(tx) => tx,
            _ => panic!(),
        };

        let tx_from_rpc = try_s!(self.rpc_client.get_transaction(tx.transaction.hash().reversed().into()).wait());
        if serialize(&tx.transaction).take() != tx_from_rpc.hex.0 {
            return ERR!("Provided payment tx {:?} doesn't match tx data from rpc {:?}", tx, tx_from_rpc);
        }

        let expected_redeem = try_s!(payment_script(
            time_lock,
            priv_bn_hash,
            &try_s!(Public::from_slice(first_pub0)),
            &try_s!(Public::from_slice(second_pub0)),
        ));

        let actual_redeem = tx.redeem_script.into();
        if expected_redeem != actual_redeem {
            return ERR!("Provided redeem script {} doesn't match expected {}", actual_redeem, expected_redeem);
        }

        let expected_output = TransactionOutput {
            value: amount,
            script_pubkey: Builder::build_p2sh(&dhash160(&expected_redeem)).into(),
        };

        if tx.transaction.outputs[0] != expected_output {
            return ERR!("Provided payment tx output doesn't match expected {:?} {:?}", tx.transaction.outputs[0], expected_output);
        }
        Ok(())
    }
}

pub fn compressed_key_pair_from_bytes(raw: &[u8], prefix: u8) -> Result<KeyPair, String> {
    if raw.len() != 32 {
        return ERR!("Invalid raw priv key len {}", raw.len());
    }

    let private = Private {
        prefix,
        compressed: true,
        secret: Secret::from(raw)
    };
    Ok(try_s!(KeyPair::from_private(private)))
}

pub fn compressed_pub_key_from_priv_raw(raw_priv: &[u8]) -> Result<H264, String> {
    let key_pair: KeyPair = try_s!(compressed_key_pair_from_bytes(raw_priv, 0));
    Ok(H264::from(&**key_pair.public()))
}

impl SwapOps for UtxoCoin {
    fn send_taker_fee(&self, fee_pub_key: &[u8], amount: u64) -> TransactionFut {
        let address = try_fus!(address_from_raw_pubkey(fee_pub_key, self.pub_addr_prefix, self.pub_t_addr_prefix));
        let output = TransactionOutput {
            value: amount,
            script_pubkey: Builder::build_p2pkh(&address.hash).to_bytes()
        };
        self.send_outputs_from_my_address(vec![output], vec![].into())
    }

    fn send_maker_payment(
        &self,
        time_lock: u32,
        pub_a0: &[u8],
        pub_b0: &[u8],
        _taker_addr: &[u8],
        priv_bn_hash: &[u8],
        amount: u64,
    ) -> TransactionFut {
        let redeem_script = try_fus!(payment_script(
            time_lock,
            priv_bn_hash,
            &try_fus!(Public::from_slice(pub_b0)),
            &try_fus!(Public::from_slice(pub_a0)),
        ));
        let output = TransactionOutput {
            value: amount,
            script_pubkey: Builder::build_p2sh(&dhash160(&redeem_script)).into(),
        };
        self.send_outputs_from_my_address(vec![output], redeem_script.into())
    }

    fn send_taker_payment(
        &self,
        time_lock: u32,
        pub_a0: &[u8],
        pub_b0: &[u8],
        _maker_addr: &[u8],
        priv_bn_hash: &[u8],
        amount: u64,
    ) -> TransactionFut {
        let redeem_script = try_fus!(payment_script(
            time_lock,
            priv_bn_hash,
            &try_fus!(Public::from_slice(pub_a0)),
            &try_fus!(Public::from_slice(pub_b0)),
        ));
        let output = TransactionOutput {
            value: amount,
            script_pubkey: Builder::build_p2sh(&dhash160(&redeem_script)).into(),
        };
        self.send_outputs_from_my_address(vec![output], redeem_script.into())
    }

    fn send_maker_spends_taker_payment(
        &self,
        taker_payment_tx: TransactionEnum,
        b_priv_0: &[u8],
        secret: &[u8],
    ) -> TransactionFut {
        let prev_tx = match taker_payment_tx {TransactionEnum::ExtendedUtxoTx(e) => e, _ => panic!()};
        let key_pair = try_fus!(compressed_key_pair_from_bytes(b_priv_0, self.wif_prefix));
        let output = TransactionOutput {
            value: prev_tx.transaction.outputs[0].value - 1000,
            script_pubkey: Builder::build_p2pkh(&self.key_pair.public().address_hash()).to_bytes()
        };
        let script_data = Builder::default()
            .push_data(secret)
            .push_opcode(Opcode::OP_0)
            .into_script();
        let transaction = try_fus!(p2sh_spending_tx(
            prev_tx,
            vec![output],
            script_data,
            &key_pair,
            self.tx_version,
            self.overwintered,
            (now_ms() / 1000) as u32,
            SEQUENCE_FINAL
        ));
        self.send_raw_tx(ExtendedUtxoTx {
            transaction,
            redeem_script: vec![].into()
        }.into())
    }

    fn send_taker_spends_maker_payment(
        &self,
        maker_payment_tx: TransactionEnum,
        a_priv_0: &[u8],
        secret: &[u8],
    ) -> TransactionFut {
        let prev_tx = match maker_payment_tx {TransactionEnum::ExtendedUtxoTx(e) => e, _ => panic!()};
        let key_pair = try_fus!(compressed_key_pair_from_bytes(a_priv_0, self.wif_prefix));
        let output = TransactionOutput {
            value: prev_tx.transaction.outputs[0].value - 1000,
            script_pubkey: Builder::build_p2pkh(&self.key_pair.public().address_hash()).to_bytes()
        };
        let script_data = Builder::default()
            .push_data(secret)
            .push_opcode(Opcode::OP_0)
            .into_script();
        let transaction = try_fus!(p2sh_spending_tx(
            prev_tx,
            vec![output],
            script_data,
            &key_pair,
            self.tx_version,
            self.overwintered,
            (now_ms() / 1000) as u32,
            SEQUENCE_FINAL
        ));
        self.send_raw_tx(ExtendedUtxoTx {
            transaction,
            redeem_script: vec![].into()
        }.into())
    }

    fn send_taker_refunds_payment(
        &self,
        taker_payment_tx: TransactionEnum,
        a_priv_0: &[u8],
    ) -> TransactionFut {
        let prev_tx = match taker_payment_tx {TransactionEnum::ExtendedUtxoTx(e) => e, _ => panic!()};
        let key_pair = try_fus!(compressed_key_pair_from_bytes(a_priv_0, self.wif_prefix));
        let output = TransactionOutput {
            value: prev_tx.transaction.outputs[0].value - 1000,
            script_pubkey: Builder::build_p2pkh(&self.key_pair.public().address_hash()).to_bytes()
        };
        let script_data = Builder::default()
            .push_opcode(Opcode::OP_1)
            .into_script();
        let transaction = try_fus!(p2sh_spending_tx(
            prev_tx,
            vec![output],
            script_data,
            &key_pair,
            self.tx_version,
            self.overwintered,
            (now_ms() / 1000) as u32,
            SEQUENCE_FINAL - 1
        ));
        self.send_raw_tx(ExtendedUtxoTx {
            transaction,
            redeem_script: vec![].into()
        }.into())
    }

    fn send_maker_refunds_payment(
        &self,
        maker_payment_tx: TransactionEnum,
        b_priv_0: &[u8],
    ) -> TransactionFut {
        let prev_tx = match maker_payment_tx {TransactionEnum::ExtendedUtxoTx(e) => e, _ => panic!()};
        let key_pair = try_fus!(compressed_key_pair_from_bytes(b_priv_0, self.wif_prefix));
        let output = TransactionOutput {
            value: prev_tx.transaction.outputs[0].value - 1000,
            script_pubkey: Builder::build_p2pkh(&self.key_pair.public().address_hash()).to_bytes()
        };
        let script_data = Builder::default()
            .push_opcode(Opcode::OP_1)
            .into_script();
        let transaction = try_fus!(p2sh_spending_tx(
            prev_tx,
            vec![output],
            script_data,
            &key_pair,
            self.tx_version,
            self.overwintered,
            (now_ms() / 1000) as u32,
            SEQUENCE_FINAL - 1
        ));
        self.send_raw_tx(ExtendedUtxoTx {
            transaction,
            redeem_script: vec![].into()
        }.into())
    }

    fn validate_fee(
        &self,
        fee_tx: TransactionEnum,
        fee_addr: &[u8],
        amount: u64
    ) -> Result<(), String> {
        let tx = match fee_tx {
            TransactionEnum::ExtendedUtxoTx(tx) => tx,
            _ => panic!(),
        };

        let tx_from_rpc = try_s!(self.rpc_client.get_transaction(tx.transaction.hash().reversed().into()).wait());

        if tx_from_rpc.hex.0 != serialize(&tx.transaction).take() {
            return ERR!("Provided dex fee tx {:?} doesn't match tx data from rpc {:?}", tx, tx_from_rpc);
        }

        let address = try_s!(address_from_raw_pubkey(fee_addr, self.pub_addr_prefix, self.pub_t_addr_prefix));
        let expected_output = TransactionOutput {
            value: amount,
            script_pubkey: Builder::build_p2pkh(&address.hash).to_bytes()
        };

        if tx.transaction.outputs[0] != expected_output {
            return ERR!("Provided dex fee tx output doesn't match expected {:?} {:?}", tx.transaction.outputs[0], expected_output);
        }
        Ok(())
    }

    fn validate_maker_payment(
        &self,
        payment_tx: TransactionEnum,
        time_lock: u32,
        pub_a0: &[u8],
        pub_b0: &[u8],
        other_addr: &[u8],
        priv_bn_hash: &[u8],
        amount: u64,
    ) -> Result<(), String> {
        self.validate_payment(
            payment_tx,
            time_lock,
            pub_b0,
            pub_a0,
            other_addr,
            priv_bn_hash,
            amount
        )
    }

    fn validate_taker_payment(
        &self,
        payment_tx: TransactionEnum,
        time_lock: u32,
        pub_a0: &[u8],
        pub_b0: &[u8],
        other_addr: &[u8],
        priv_bn_hash: &[u8],
        amount: u64,
    ) -> Result<(), String> {
        self.validate_payment(
            payment_tx,
            time_lock,
            pub_a0,
            pub_b0,
            other_addr,
            priv_bn_hash,
            amount
        )
    }
}

impl MarketCoinOps for UtxoCoin {
    fn my_address(&self) -> Cow<str> {
        self.0.my_address.to_string().into()
    }

    fn my_balance(&self) -> Box<Future<Item=f64, Error=String> + Send> {
        self.rpc_client.display_balance(self.my_address.clone())
    }

    fn send_raw_tx(&self, tx: TransactionEnum) -> TransactionFut {
        let tx = match tx {TransactionEnum::ExtendedUtxoTx(e) => e, _ => panic!()};
        let send_fut = self.rpc_client.send_transaction(&tx.transaction, self.my_address.clone());
        Box::new(send_fut.map(move |_res| { tx.into() }))
    }

    fn wait_for_confirmations(
        &self,
        tx: TransactionEnum,
        confirmations: u32,
        wait_until: u64,
    ) -> Result<(), String> {
        let tx = match tx {TransactionEnum::ExtendedUtxoTx(e) => e, _ => panic!()};
        self.rpc_client.wait_for_confirmations(
            &tx.transaction,
            confirmations as u32,
            wait_until,
        )
    }

    fn wait_for_tx_spend(&self, transaction: TransactionEnum, wait_until: u64) -> Result<TransactionEnum, String> {
        let tx = match transaction {TransactionEnum::ExtendedUtxoTx(e) => e, _ => panic!()};
        let res = try_s!(self.rpc_client.wait_for_payment_spend(
            &tx.transaction,
            0,
            wait_until,
        ));

        Ok(TransactionEnum::ExtendedUtxoTx(ExtendedUtxoTx {
            transaction: res,
            redeem_script: vec![].into(),
        }))
    }

    fn tx_from_raw_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, String> {
        // should be at least 8 bytes length in case tx and redeem length is zero
        if bytes.len() < 8 {
            return ERR!("Input bytes slice len is too small");
        }
        let len_array = clone_into_array::<[u8; 4], u8>(&bytes[0..4]);
        let tx_len: u32 = unsafe { transmute(len_array) };
        let mut read: usize = 4;
        let transaction: UtxoTransaction = try_s!(deserialize(&bytes[read..read + tx_len as usize]).map_err(|err| format!("{:?}", err)));
        read += tx_len as usize;
        let redeem_len: u32 = unsafe { transmute(clone_into_array::<[u8; 4], u8>(&bytes[read..read + 4])) };
        read += 4 as usize;
        let redeem_script = Bytes::from(&bytes[read..read + redeem_len as usize]);
        Ok(ExtendedUtxoTx {
            transaction,
            redeem_script,
        }.into())
    }

    fn current_block(&self) -> Box<Future<Item=u64, Error=String> + Send> {
        self.rpc_client.get_block_count()
    }
}

impl IguanaInfo for UtxoCoin {
    fn ticker<'a> (&'a self) -> &'a str {&self.ticker[..]}
}
impl MmCoin for UtxoCoin {
    fn is_asset_chain(&self) -> bool { self.asset_chain }

    fn check_i_have_enough_to_trade(&self, amount: f64, maker: bool) -> Box<Future<Item=(), Error=String> + Send> {
        let fee_f64 = dstr(self.tx_fee as i64);
        let arc = self.clone();
        Box::new(self.my_balance().and_then(move |balance| {
            let required = if maker {
                amount + fee_f64
            } else {
                amount + amount / 777.0 + 2.0 * fee_f64
            };
            if balance < required {
                return ERR!("{} balance {} is too low, required {:.6}", arc.ticker(), balance, required);
            }
            Ok(())
        }))
    }
}

pub fn random_compressed_key_pair(prefix: u8) -> Result<KeyPair, String> {
    let random_key = try_s!(Random::new(prefix).generate());

    Ok(try_s!(KeyPair::from_private(Private {
        prefix,
        secret: random_key.private().secret.clone(),
        compressed: true,
    })))
}

fn key_pair_from_seed(seed: &[u8], prefix: u8) -> KeyPair {
    let mut hasher = Sha256::new();
    hasher.input(seed);
    let mut hash = hasher.result();
    hash[0] &= 248;
    hash[31] &= 127;
    hash[31] |= 64;
    let private = Private {
        prefix,
        secret: H256::from(hash.as_slice()),
        compressed: true,
    };

    KeyPair::from_private(private).unwrap()
}

pub enum UtxoInitMode {
    Native,
    Electrum(Vec<String>),
}

pub fn utxo_coin_from_iguana_info(info: *mut lp::iguana_info, mode: UtxoInitMode) -> Result<MmCoinEnum, String> {
    let info = unsafe { *info };
    let private = Private {
        prefix: info.wiftype,
        secret: H256::from(unsafe { lp::G.LP_privkey.bytes }),
        compressed: true,
    };

    let key_pair = try_s!(KeyPair::from_private(private));
    let my_address = Address {
        prefix: info.pubtype,
        t_addr_prefix: info.taddr,
        hash: key_pair.public().address_hash(),
    };

    let ticker = try_s! (unsafe {CStr::from_ptr (info.symbol.as_ptr())} .to_str()) .into();

    let rpc_client = match mode {
        UtxoInitMode::Native => {
            let auth_str = unsafe { try_s!(CStr::from_ptr(info.userpass.as_ptr()).to_str()) };
            let uri = unsafe { try_s!(CStr::from_ptr(info.serverport.as_ptr()).to_str()) };
            UtxoRpcClientEnum::Native(NativeClient {
                uri: format!("http://{}", uri),
                auth: format!("Basic {}", base64_encode(auth_str, URL_SAFE)),
            })
        },
        UtxoInitMode::Electrum(urls) => {
            let mut client = ElectrumClientImpl::new();
            for url in urls.iter() {
                try_s!(client.add_server(url));
            }
            try_s!(client.blockchain_headers_subscribe().wait());

            let client = Arc::new(client);
            // ping the electrum servers every minute to prevent them from disconnecting us.
            // according to docs server can do it if there are no messages in ~10 minutes.
            // https://electrumx.readthedocs.io/en/latest/protocol-methods.html?highlight=keep#server-ping
            // weak reference will allow to stop the thread if client is dropped
            let weak_client = Arc::downgrade(&client);
            try_s!(thread::Builder::new().name(format!("electrum_ping_{}", ticker)).spawn(move || {
                loop {
                    if let Some(client) = weak_client.upgrade() {
                        if let Err(e) = client.server_ping().wait() {
                            log!("Electrum servers " [urls] " ping error " [e]);
                        }
                        // the simplest way to retrigger subscription in case of reconnecting is
                        // just running subscribe requests in loop. It doesn't cause Electrum server
                        // to double the subscription messages.
                        if let Err(e) = client.blockchain_headers_subscribe().wait() {
                            log!("Electrum servers " [urls] " resubscribe error " [e]);
                        }
                    } else {
                        break;
                    }
                    thread::sleep(Duration::from_secs(30));
                }
            }));
            UtxoRpcClientEnum::Electrum(ElectrumClient(client))
        }
    };
    let tx_version = if info.isassetchain == 1 || ticker == "KMD" || ticker == "BEER" || ticker == "PIZZA" {
        4
    } else {
        info.txversion
    };
    // At least for now only ZEC and forks rely on tx version so we can use it to detect overwintered
    // TODO Consider refactoring, overwintered flag should be explicitly set in coins config
    let overwintered = tx_version >= 3;

    let coin = UtxoCoinImpl {
        ticker,
        decimals: 8,
        rpc_client,
        key_pair,
        is_pos: false,
        notarized: false,
        overwintered,
        pub_addr_prefix: info.pubtype,
        p2sh_addr_prefix: info.p2shtype,
        pub_t_addr_prefix: info.taddr,
        p2sh_t_addr_prefix: info.taddr,
        rpc_password: "".to_owned(),
        rpc_port: 0,
        rpc_user: "".to_owned(),
        segwit: false,
        wif_prefix: info.wiftype,
        tx_version,
        my_address: my_address.clone(),
        asset_chain: info.isassetchain == 1,
        tx_fee: info.txfee,
    };
    Ok(UtxoCoin(Arc::new(coin)).into())
}

#[test]
fn test_extract_secret() {
    let bytes = hex::decode("0100000001de7aa8d29524906b2b54ee2e0281f3607f75662cbc9080df81d1047b78e21dbc00000000d7473044022079b6c50820040b1fbbe9251ced32ab334d33830f6f8d0bf0a40c7f1336b67d5b0220142ccf723ddabb34e542ed65c395abc1fbf5b6c3e730396f15d25c49b668a1a401209da937e5609680cb30bff4a7661364ca1d1851c2506fa80c443f00a3d3bf7365004c6b6304f62b0e5cb175210270e75970bb20029b3879ec76c4acd320a8d0589e003636264d01a7d566504bfbac6782012088a9142fb610d856c19fd57f2d0cffe8dff689074b3d8a882103f368228456c940ac113e53dad5c104cf209f2f102a409207269383b6ab9b03deac68ffffffff01d0dc9800000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88ac40280e5c").unwrap();
    let tx: UtxoTransaction = deserialize(bytes.as_slice()).unwrap();
    let extended = ExtendedUtxoTx {
        transaction: tx,
        redeem_script: vec![].into()
    };

    let secret = extended.extract_secret().unwrap();
    let expected_secret = hex::decode("9da937e5609680cb30bff4a7661364ca1d1851c2506fa80c443f00a3d3bf7365").unwrap();
    assert_eq!(expected_secret, secret);
}

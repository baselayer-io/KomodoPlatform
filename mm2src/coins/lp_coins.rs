
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
//  coins.rs
//  marketmaker
//

#![feature(non_ascii_idents)]

#[macro_use] extern crate common;
#[macro_use] extern crate fomat_macros;
#[macro_use] extern crate gstuff;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate serde_json;
#[macro_use] extern crate unwrap;

use common::{bitcoin_ctx, bits256, lp, rpc_response, rpc_err_response, HyRes};
use common::mm_ctx::{from_ctx, MmArc};
use futures::{Future};
use gstuff::now_ms;
use hashbrown::hash_map::{HashMap, RawEntryMut};
use libc::{c_char, c_void};
use serde_json::{self as json, Value as Json};
use std::borrow::Cow;
use std::ffi::{CString};
use std::fmt::Debug;
use std::mem::zeroed;
use std::ops::Deref;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};

#[doc(hidden)]
pub mod coins_tests;
pub mod eth;
use self::eth::{eth_coin_from_iguana_info, EthCoin, SignedEthTransaction};
pub mod utxo;
use self::utxo::{utxo_coin_from_iguana_info, ExtendedUtxoTx, UtxoCoin, UtxoInitMode};

pub trait Transaction: Debug + 'static {
    fn to_raw_bytes(&self) -> Vec<u8>;
    fn extract_secret(&self) -> Result<Vec<u8>, String>;
    /// String representation of tx hash for displaying purpose
    fn tx_hash(&self) -> String;
}

#[derive(Clone, Debug)]
pub enum TransactionEnum {
    ExtendedUtxoTx (ExtendedUtxoTx),
    SignedEthTransaction (SignedEthTransaction)
}
ifrom! (TransactionEnum, ExtendedUtxoTx);
ifrom! (TransactionEnum, SignedEthTransaction);

// NB: When stable and groked by IDEs, `enum_dispatch` can be used instead of `Deref` to speed things up.
impl Deref for TransactionEnum {
    type Target = Transaction;
    fn deref (&self) -> &dyn Transaction {
        match self {
            &TransactionEnum::ExtendedUtxoTx (ref t) => t,
            &TransactionEnum::SignedEthTransaction (ref t) => t,
}   }   }

pub type TransactionFut = Box<dyn Future<Item=TransactionEnum, Error=String>>;

/// Swap operations (mostly based on the Hash/Time locked transactions implemented by coin wallets).
pub trait SwapOps {
    fn send_taker_fee(&self, fee_addr: &[u8], amount: u64) -> TransactionFut;

    fn send_maker_payment(
        &self,
        time_lock: u32,
        pub_a0: &[u8],
        pub_b0: &[u8],
        taker_addr: &[u8],
        secret_hash: &[u8],
        amount: u64
    ) -> TransactionFut;

    fn send_taker_payment(
        &self,
        time_lock: u32,
        pub_a0: &[u8],
        pub_b0: &[u8],
        maker_addr: &[u8],
        secret_hash: &[u8],
        amount: u64,
    ) -> TransactionFut;

    fn send_maker_spends_taker_payment(
        &self,
        taker_payment_tx: TransactionEnum,
        b_priv_0: &[u8],
        secret: &[u8],
    ) -> TransactionFut;

    fn send_taker_spends_maker_payment(
        &self,
        maker_payment_tx: TransactionEnum,
        a_priv_0: &[u8],
        secret: &[u8],
    ) -> TransactionFut;

    fn send_taker_refunds_payment(
        &self,
        taker_payment_tx: TransactionEnum,
        a_priv_0: &[u8],
    ) -> TransactionFut;

    fn send_maker_refunds_payment(
        &self,
        maker_payment_tx: TransactionEnum,
        b_priv_0: &[u8],
    ) -> TransactionFut;

    fn validate_fee(
        &self,
        fee_tx: TransactionEnum,
        fee_addr: &[u8],
        amount: u64
    ) -> Result<(), String>;

    fn validate_maker_payment(
        &self,
        payment_tx: TransactionEnum,
        time_lock: u32,
        pub_a0: &[u8],
        pub_b0: &[u8],
        other_addr: &[u8],
        priv_bn_hash: &[u8],
        amount: u64,
    ) -> Result<(), String>;

    fn validate_taker_payment(
        &self,
        payment_tx: TransactionEnum,
        time_lock: u32,
        pub_a0: &[u8],
        pub_b0: &[u8],
        other_addr: &[u8],
        priv_bn_hash: &[u8],
        amount: u64,
    ) -> Result<(), String>;
}

/// Operations that coins have independently from the MarketMaker.
/// That is, things implemented by the coin wallets or public coin services.
pub trait MarketCoinOps {
    fn my_address(&self) -> Cow<str>;

    fn my_balance(&self) -> Box<Future<Item=f64, Error=String> + Send>;

    fn send_raw_tx(&self, tx: TransactionEnum) -> TransactionFut;

    fn wait_for_confirmations(
        &self,
        tx: TransactionEnum,
        confirmations: u32,
        wait_until: u64,
    ) -> Result<(), String>;

    fn wait_for_tx_spend(&self, transaction: TransactionEnum, wait_until: u64) -> Result<TransactionEnum, String>;

    fn tx_from_raw_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, String>;

    fn current_block(&self) -> Box<Future<Item=u64, Error=String> + Send>;
}

/// Compatibility layer on top of `lp::iguana_info`.  
/// Some of the coin information is kept in the old C `iguana_info` struct,
/// we're going to port it eventually and consequently this trait should go away.
pub trait IguanaInfo {
    fn ticker<'a> (&'a self) -> &'a str;

    fn iguana_info (&self) -> &'static mut lp::iguana_info {
        let ticker = unwrap! (CString::new (self.ticker()));
        let ii = common::for_c::LP_coinsearch (ticker.as_ptr());
        unsafe {&mut *ii}
    }
}

/// NB: Implementations are expected to follow the pImpl idiom, providing cheap reference-counted cloning and garbage collection.
pub trait MmCoin: SwapOps + MarketCoinOps + IguanaInfo + Debug + 'static {
    // `MmCoin` is an extension fulcrum for something that doesn't fit the `MarketCoinOps`. Practical examples:
    // name (might be required for some APIs, CoinMarketCap for instance);
    // coin statistics that we might want to share with UI;
    // state serialization, to get full rewind and debugging information about the coins participating in a SWAP operation.
    // status/availability check: https://github.com/artemii235/SuperNET/issues/156#issuecomment-446501816

    fn is_asset_chain(&self) -> bool;

    fn check_i_have_enough_to_trade(&self, amount: f64, maker: bool) -> Box<Future<Item=(), Error=String> + Send>;
}

#[derive(Clone, Debug)]
pub enum MmCoinEnum {
    UtxoCoin (UtxoCoin),
    EthCoin (EthCoin)
}

impl From<UtxoCoin> for MmCoinEnum {
    fn from (c: UtxoCoin) -> MmCoinEnum {
        MmCoinEnum::UtxoCoin (c)
}   }

impl From<EthCoin> for MmCoinEnum {
    fn from (c: EthCoin) -> MmCoinEnum {
        MmCoinEnum::EthCoin (c)
}   }

// NB: When stable and groked by IDEs, `enum_dispatch` can be used instead of `Deref` to speed things up.
impl Deref for MmCoinEnum {
    type Target = MmCoin;
    fn deref (&self) -> &dyn MmCoin {
        match self {
            &MmCoinEnum::UtxoCoin (ref c) => c,
            &MmCoinEnum::EthCoin (ref c) => c
}   }   }

struct CoinsContext {
    /// A map from a currency ticker symbol to the corresponding coin.
    /// Similar to `LP_coins`.
    coins: Mutex<HashMap<String, MmCoinEnum>>
}
impl CoinsContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    fn from_ctx (ctx: &MmArc) -> Result<Arc<CoinsContext>, String> {
        Ok (try_s! (from_ctx (&ctx.coins_ctx, move || {
            Ok (CoinsContext {
                coins: Mutex::new (HashMap::new())
            })
        })))
    }
}

/*
char *portstrs[][3] = { { "BTC", "8332" }, { "KMD", "7771" } };

int32_t LP_is_slowcoin(char *symbol)
{
    if ( strcmp(symbol,"BTC") == 0 )
        return(2);
    else if ( strcmp(symbol,"BCH") == 0 )
        return(1);
    else if ( strcmp(symbol,"BTG") == 0 )
        return(1);
    else if ( strcmp(symbol,"SBTC") == 0 )
        return(1);
    else return(0);
}

uint16_t LP_rpcport(char *symbol)
{
    int32_t i;
    if ( symbol != 0 && symbol[0] != 0 )
    {
        for (i=0; i<sizeof(portstrs)/sizeof(*portstrs); i++)
            if ( strcmp(portstrs[i][0],symbol) == 0 )
                return(atoi(portstrs[i][1]));
    }
    return(0);
}

uint16_t LP_busport(uint16_t rpcport)
{
    if ( rpcport == 8332 )
        return(8334); // BTC
    else if ( rpcport < (1 << 15) )
        return(65535 - rpcport);
    else return(rpcport+1);
}

char *parse_conf_line(char *line,char *field)
{
    line += strlen(field);
    for (; *line!='='&&*line!=0; line++)
        break;
    if ( *line == 0 )
        return(0);
    if ( *line == '=' )
        line++;
    while ( line[strlen(line)-1] == '\r' || line[strlen(line)-1] == '\n' || line[strlen(line)-1] == ' ' )
        line[strlen(line)-1] = 0;
    //printf("LINE.(%s)\n",line);
    _stripwhite(line,0);
    return(clonestr(line));
}

uint16_t LP_userpassfp(char *symbol,char *username,char *password,FILE *fp)
{
    char *rpcuser,*rpcpassword,*str,line[8192]; uint16_t port = 0;
    rpcuser = rpcpassword = 0;
    username[0] = password[0] = 0;
    while ( fgets(line,sizeof(line),fp) != 0 )
    {
        if ( line[0] == '#' )
            continue;
        //printf("line.(%s) %p %p\n",line,strstr(line,(char *)"rpcuser"),strstr(line,(char *)"rpcpassword"));
        if ( (str= strstr(line,(char *)"rpcuser")) != 0 )
            rpcuser = parse_conf_line(str,(char *)"rpcuser");
        else if ( (str= strstr(line,(char *)"rpcpassword")) != 0 )
            rpcpassword = parse_conf_line(str,(char *)"rpcpassword");
        else if ( (str= strstr(line,(char *)"rpcport")) != 0 )
        {
            str = parse_conf_line(str,(char *)"rpcport");
            if ( str != 0 )
            {
                port = atoi(str);
                printf("found RPCPORT.%u\n",port);
                free(str);
            }
        }
    }
    if ( rpcuser != 0 && rpcpassword != 0 )
    {
        strcpy(username,rpcuser);
        strcpy(password,rpcpassword);
    }
    //printf("%s rpcuser.(%s) rpcpassword.(%s)\n",symbol,rpcuser,rpcpassword);
    if ( rpcuser != 0 )
        free(rpcuser);
    if ( rpcpassword != 0 )
        free(rpcpassword);
    return(port);
}

void LP_statefname(char *fname,char *symbol,char *assetname,char *str,char *name,char *confpath)
{
    if ( confpath != 0 && confpath[0] != 0 )
    {
		#if defined(NATIVE_WINDOWS)
		// need to do something with "confpath":"`${process.env.HOME}`/.muecore/mue.conf" under Windows
		char *ht = "`${process.env.USERHOME}`", *ht_start, *p_ht;
		char ht_symbol[2];
		
		ht_start = strstr(confpath, ht);

		if (ht_start) {
			ht_start = ht_start + strlen(ht);
			sprintf(fname, "%s\\", LP_getdatadir());
			p_ht = ht_start;
			if (p_ht[0] == '/' && p_ht[1] == '.') {
				p_ht += 2;
				//printf("%s\n", p_ht);
				while (p_ht[0] != '\0') {
					if (p_ht[0] == '/') strcat(fname, "\\"); else
					{
						ht_symbol[0] = p_ht[0]; ht_symbol[1] = '\0';
						strcat(fname, ht_symbol);
					}
					p_ht++;
				}
				//printf("%s\n", fname);
			}
		} else strcpy(fname, confpath);
		#else
		strcpy(fname,confpath);
		#endif	
        return;
    }
    sprintf(fname,"%s",LP_getdatadir());
#ifdef _WIN32
    strcat(fname,"\\");
#else
    strcat(fname,"/");
#endif
    if ( strcmp(symbol,"BTC") == 0 )
    {
#if defined(__APPLE__) || defined(NATIVE_WINDOWS)
        strcat(fname,"Bitcoin");
#else
        strcat(fname,".bitcoin");
#endif
    }
    else if ( name != 0 )
    {
        char name2[64];
#if defined(__APPLE__) || defined(NATIVE_WINDOWS)
        int32_t len;
        strcpy(name2,name);
        name2[0] = toupper(name2[0]);
        len = (int32_t)strlen(name2);
        if ( strcmp(&name2[len-4],"coin") == 0 )
            name2[len - 4] = 'C';
#else
        name2[0] = '.';
        strcpy(name2+1,name);
#endif
       strcat(fname,name2);
    }
    else
    {
#if defined(__APPLE__) || defined(NATIVE_WINDOWS)
        strcat(fname,"Komodo");
#else
        strcat(fname,".komodo");
#endif
        if ( strcmp(symbol,"KMD") != 0 )
        {
#ifdef _WIN32
            strcat(fname,"\\");
#else
            strcat(fname,"/");
#endif
            strcat(fname,assetname);
        }
    }
#ifdef _WIN32
    strcat(fname,"\\");
#else
    strcat(fname,"/");
#endif
    strcat(fname,str);
}

uint16_t LP_userpass(char *userpass,char *symbol,char *assetname,char *confroot,char *name,char *confpath,uint16_t origport)
{
    FILE *fp; char fname[512],username[512],password[512],confname[512]; uint16_t port = 0;
    userpass[0] = 0;
    sprintf(confname,"%s.conf",confroot);
    if ( 0 )
        printf("%s (%s) %s confname.(%s) confroot.(%s)\n",symbol,assetname,name,confname,confroot);
#if defined(__APPLE__) || defined(NATIVE_WINDOWS)
    int32_t len;
    confname[0] = toupper(confname[0]);
    len = (int32_t)strlen(confname);
    if ( strcmp(&confname[len-4],"coin") == 0 )
        confname[len - 4] = 'C';
#endif
    LP_statefname(fname,symbol,assetname,confname,name,confpath);
    if ( (fp= fopen(fname,"rb")) != 0 )
    {
        if ( (port= LP_userpassfp(symbol,username,password,fp)) == 0 )
            port = origport;
        sprintf(userpass,"%s:%s",username,password);
        fclose(fp);
        if ( 0 )
            printf("LP_statefname.(%s) <- %s %s %s (%s) (%s)\n",fname,name,symbol,assetname,userpass,confpath);
        return(port);
    } else printf("cant open.(%s)\n",fname);
    return(origport);
}

cJSON *LP_coinjson(struct iguana_info *coin,int32_t showwif)
{
    struct electrum_info *ep; bits256 zero; int32_t notarized; uint64_t balance; char wifstr[128],ipaddr[72]; uint8_t tmptype; bits256 checkkey; cJSON *item = cJSON_CreateObject();
    jaddstr(item,"coin",coin->symbol);
    if ( showwif != 0 )
    {
        bitcoin_priv2wif(coin->symbol,coin->wiftaddr,wifstr,G.LP_privkey,coin->wiftype);
        bitcoin_wif2priv(coin->symbol,coin->wiftaddr,&tmptype,&checkkey,wifstr);
        if ( bits256_cmp(G.LP_privkey,checkkey) == 0 )
            jaddstr(item,"wif",wifstr);
        else jaddstr(item,"wif","error creating wif");
    }
    jadd(item,"installed",coin->userpass[0] == 0 ? jfalse() : jtrue());
    if ( coin->userpass[0] != 0 )
    {
        jaddnum(item,"height",LP_getheight(&notarized,coin));
        if ( notarized > 0 )
            jaddnum(item,"notarized",notarized);
        if ( coin->electrum != 0 )
            balance = LP_unspents_load(coin->symbol,coin->smartaddr);
        else balance = LP_RTsmartbalance(coin);
        jaddnum(item,"balance",dstr(balance));
        jaddnum(item,"KMDvalue",dstr(LP_KMDvalue(coin,balance)));
    }
#ifndef NOTETOMIC
    else if (coin->etomic[0] != 0) {
        int error = 0;
        if (coin->inactive == 0) {
            balance = LP_etomic_get_balance(coin, coin->smartaddr, &error);
        } else {
            balance = 0;
        }
        jaddnum(item,"height",-1);
        jaddnum(item,"balance",dstr(balance));
    }
#endif
    else
    {
        jaddnum(item,"height",-1);
        jaddnum(item,"balance",0);
    }
    if ( coin->inactive != 0 )
    {
        jaddstr(item,"status","inactive");
    }
    else jaddstr(item,"status","active");
    if ( coin->isPoS != 0 )
        jaddstr(item,"type","PoS");
    if ( (ep= coin->electrum) != 0 )
    {
        sprintf(ipaddr,"%s:%u",ep->ipaddr,ep->port);
        jaddstr(item,"electrum",ipaddr);
    }
    jaddstr(item,"smartaddress",coin->smartaddr);
    jaddstr(item,"rpc",coin->serverport);
    jaddnum(item,"pubtype",coin->pubtype);
    jaddnum(item,"p2shtype",coin->p2shtype);
    jaddnum(item,"wiftype",coin->wiftype);
    jaddnum(item,"txfee",strcmp(coin->symbol,"BTC") != 0 ? coin->txfee : LP_txfeecalc(coin,0,0));
    if ( strcmp(coin->symbol,"KMD") == 0 )
    {
        memset(zero.bytes,0,sizeof(zero));
        if ( strcmp(coin->smartaddr,coin->instantdex_address) != 0 )
        {
            LP_instantdex_depositadd(coin->smartaddr,zero);
            strcpy(coin->instantdex_address,coin->smartaddr);
        }
        jaddnum(item,"zcredits",dstr(LP_myzcredits()));
        jadd(item,"zdebits",LP_myzdebits());
    }
    return(item);
}

struct iguana_info *LP_conflicts_find(struct iguana_info *refcoin)
{
    struct iguana_info *coin=0,*tmp; int32_t n;
    if ( refcoin != 0 && (n= (int32_t)strlen(refcoin->serverport)) > 3 && strcmp(":80",&refcoin->serverport[n-3]) != 0 )
    {
        HASH_ITER(hh,LP_coins,coin,tmp)
        {
            if ( coin->inactive != 0 || coin->electrum != 0 || coin == refcoin )
                continue;
            if ( strcmp(coin->serverport,refcoin->serverport) == 0 )
                break;
        }
    }
    return(coin);
}

cJSON *LP_coinsjson(int32_t showwif)
{
    struct iguana_info *coin,*tmp; cJSON *array = cJSON_CreateArray();
    HASH_ITER(hh,LP_coins,coin,tmp)
    {
        jaddi(array,LP_coinjson(coin,showwif));
    }
    return(array);
}

char *LP_getcoin(char *symbol)
{
    int32_t numenabled,numdisabled; struct iguana_info *coin,*tmp; cJSON *item=0,*retjson;
    retjson = cJSON_CreateObject();
    if ( symbol != 0 && symbol[0] != 0 )
    {
        numenabled = numdisabled = 0;
        HASH_ITER(hh,LP_coins,coin,tmp)
        {
            if ( strcmp(symbol,coin->symbol) == 0 )
                item = LP_coinjson(coin,LP_showwif);
            if ( coin->inactive == 0 )
                numenabled++;
            else numdisabled++;
        }
        jaddstr(retjson,"result","success");
        jaddnum(retjson,"enabled",numenabled);
        jaddnum(retjson,"disabled",numdisabled);
        if ( item == 0 )
            item = cJSON_CreateObject();
        jadd(retjson,"coin",item);
    }
    return(jprint(retjson,1));
}

struct iguana_info *LP_coinsearch(char *symbol)
{
    struct iguana_info *coin = 0;
    if ( symbol != 0 && symbol[0] != 0 )
    {
        portable_mutex_lock(&LP_coinmutex);
        HASH_FIND(hh,LP_coins,symbol,strlen(symbol),coin);
        portable_mutex_unlock(&LP_coinmutex);
    }
    return(coin);
}

struct iguana_info *LP_coinadd(struct iguana_info *cdata)
{
    struct iguana_info *coin = calloc(1,sizeof(*coin));
    *coin = *cdata;
    portable_mutex_init(&coin->txmutex);
    portable_mutex_init(&coin->addrmutex);
    portable_mutex_init(&coin->addressutxo_mutex);
    portable_mutex_lock(&LP_coinmutex);
    HASH_ADD_KEYPTR(hh,LP_coins,coin->symbol,strlen(coin->symbol),coin);
    portable_mutex_unlock(&LP_coinmutex);
    strcpy(coin->validateaddress,"validateaddress");
    strcpy(coin->getinfostr,"getinfo");
    strcpy(coin->estimatefeestr,"estimatefee");
    return(coin);
}
*/

/// Adds a new currency into the list of currencies configured.
///
/// Returns an error if the currency already exists. Initializing the same currency twice is a bad habit
/// (might lead to misleading and confusing information during debugging and maintenance, see DRY)
/// and should be fixed on the call site.
///
/// NB: As of now only a part of coin information has been ported to `MmCoinEnum`, as much as necessary to fix the SWAP in #233.
///     We plan to port the rest of it later. Until then, it is accessible through the old `iguana_info` struct.
///
/// * `req` - Payload of the corresponding "enable" or "electrum" RPC request.
fn lp_coininit (ctx: &MmArc, ticker: &str, req: &Json) -> Result<MmCoinEnum, String> {
    let cctx = try_s! (CoinsContext::from_ctx (ctx));
    let mut coins = try_s! (cctx.coins.lock());
    let ve = match coins.raw_entry_mut().from_key (ticker) {
        RawEntryMut::Occupied (_oe) => return ERR! ("Coin {} already initialized", ticker),
        RawEntryMut::Vacant (ve) => ve
    };

    let coins_en = if let Some (coins) = ctx.conf["coins"].as_array() {
        coins.iter().find (|coin| coin["coin"].as_str() == Some (ticker)) .unwrap_or (&Json::Null)
    } else {&Json::Null};

    if coins_en.is_null() {
        ctx.log.log ("😅", &[&("coin" as &str), &ticker, &("no-conf" as &str)],
            &fomat! ("Warning, coin " (ticker) " is used without a corresponding configuration."));
    }

    let c_ticker = try_s! (CString::new (ticker));
    let rpcport = match coins_en["rpcport"].as_u64() {
        Some (port) if port > 0 && port < u16::max_value() as u64 => port as u16,
        // NB: 0 for anything that's not "BTC" or "KMD".
        _ => unsafe {lp::LP_rpcport (c_ticker.as_ptr() as *mut c_char)}
    };

    let _estimatedrate = coins_en["estimatedrate"].as_f64().unwrap_or (20.);

    // NB: Unlike the previous C implementation we're never *moving* the `iguana_info` instance, it's effectively pinned.
    let mut ii: Box<lp::iguana_info> = Box::new (unsafe {zeroed()});
    // NB: Read values from local variables and not from `ii`
    //     (that is, to read A: let A = ...; ii.A = A; ii.B = ... A ...),
    //     allowing the compiler to verify the dependencies.
    try_s! (safecopy! (ii.symbol, "{}", ticker));
    ii.txversion = coins_en["txversion"].as_i64().unwrap_or (if ticker == "PART" {160} else {1}) as i32;
    ii.updaterate = (now_ms() / 1000) as u32;
    ii.isPoS = coins_en["isPoS"].as_i64().unwrap_or (0) as u8;
    let taddr = coins_en["taddr"].as_u64().unwrap_or (0) as u8;
    ii.taddr = taddr;
    ii.wiftaddr = 0;
    ii.longestchain = 1;
    ii.txfee = match coins_en["txfee"].as_u64() {
        None | Some (0) => if ticker == "BTC" {0} else {lp::LP_MIN_TXFEE as u64},
        Some (fee) => fee
    };
    ii.pubtype = coins_en["pubtype"].as_u64().unwrap_or (if ticker == "BTC" {0} else {60}) as u8;
    ii.p2shtype = coins_en["p2shtype"].as_u64().unwrap_or (if ticker == "BTC" {5} else {85}) as u8;
    ii.wiftype = coins_en["wiftype"].as_u64().unwrap_or (if ticker == "BTC" {128} else {188}) as u8;
    let inactive = loop {
        if ticker != "KMD" {
            if let Some (active) = coins_en["active"].as_i64() {
                if active != 0 {break 0}
        }   }
        break now_ms() / 1000
    };
    ii.inactive = inactive as u32;
    ii.ctx = unsafe {bitcoin_ctx() as *mut c_void};
    ii.noimportprivkey_flag = match ticker {"XVG" | "CLOAK" | "PPC" | "BCC" | "ORB" => 1, _ => 0};
    if rpcport != 0 {try_s! (safecopy! (ii.serverport, "127.0.0.1:{}", rpcport))}
    unsafe {lp::LP_coin_curl_init (&mut *ii)};
    ii.decimals = coins_en["decimals"].as_u64().unwrap_or (0) as u8;

    let asset = coins_en["asset"].as_str();
    let isassetchain = if let Some (asset) = asset {
        if asset != "BEER" && asset != "PIZZA" {
            ii.isassetchain = 1; 1
        } else {0}
    } else {0};

    ii.zcash = match ticker {
        "KMD" if isassetchain != 0 || taddr != 0 => lp::LP_IS_ZCASHPROTOCOL,
        "BCH" => lp::LP_IS_BITCOINCASH,
        "BTG" => lp::LP_IS_BITCOINGOLD,
        "CMM" => lp::LP_IS_BITCOINCASH,
        _ => 0
    } as u8;

    // NB: `LP_privkeycalc` initializes `lp::G.LP_privkey`, required to instantiate `UtxoCoin`.

    // NB: `LP_privkeycalc` triggers a chain of functions that ends up calling `LP_coinfind`, which tries to auto-add a coin.
    //     In order for this extra instance not to be added we have to make haste and add one ourselves.
    // Copies the `iguana_info` generated by `lp_coininit` into the old global `LP_coins`.
    let ii = Box::leak (ii);
    common::for_c::LP_coinadd (ii);

    let rpcport = unsafe {
        // TODO
        // The name/name2 logic here is a best-effort attempt to preserve the one in the former C code, but I think we should later simplify it.
        // These names are used to construct the file path, but that construction seems broken, at least on Windows (it's using a fixed USERHOME).
        // Maybe we should reuse the `komodo_conf_path` which provides the `c_confpath` value for the tests.
        let name = match ticker {
            "BTC" => "bitcoin",  // These were hardcoded before, though now for CMC we need the explicit "name" in the `coins` configuration as well.
            "KMD" => "komodo",
            _ => if let Some (asset) = asset {asset} else {coins_en["name"].as_str().unwrap_or ("")}
        };
        let name2 = if ticker == "KMD" || asset.is_some() {""} else {name};

        let c_asset = try_s! (CString::new (asset.unwrap_or ("")));
        let c_name = try_s! (CString::new (name));
        let c_name2 = try_s! (CString::new (name2));
        let c_name2_ptr = if name2 == "" {
            null_mut()
        } else {
            c_name2.as_ptr() as *mut c_char
        };

        let confpath = coins_en["confpath"].as_str().unwrap_or ("");
        let c_confpath = try_s! (CString::new (confpath));
        let confpath_ptr = if confpath == "" {
            null_mut()
        } else {
            c_confpath.as_ptr() as *mut c_char
        };

        lp::LP_userpass (
            ii.userpass.as_mut_ptr(),            // userpass
            c_ticker.as_ptr() as *mut c_char,    // symbol
            c_asset.as_ptr() as *mut c_char,     // assetname
            c_name.as_ptr() as *mut c_char,      // confroot
            c_name2_ptr,                         // name
            confpath_ptr,                        // confpath
            rpcport                              // origport
        )
    };
    if rpcport == 0 {log! ("Warning, coin " (ticker) " doesn't have the 'rpcport' configured")}

    // TODO: Move the private key into `MmCtx`. Initialize it before `lp_coininit`.
    let passphrase = try_s! (ctx.conf["passphrase"].as_str().ok_or ("!passphrase"));
    let c_passphrase = try_s! (CString::new (&passphrase[..]));
    unsafe {
        let mut pubkey33: [u8; 33] = zeroed();
        let mut pubkey: bits256 = zeroed();
        let pk = lp::LP_privkeycalc (
            ctx.btc_ctx() as *mut c_void,          // void *ctx
            pubkey33.as_mut_ptr(),                 // uint8_t *pubkey33
            &mut pubkey,                           // bits256 *pubkeyp
            ii,                                    // struct iguana_info *coin
            c_passphrase.as_ptr() as *mut c_char,  // char *passphrase
            b"\0".as_ptr() as *mut c_char          // char *wifstr
        );
        if !pk.nonz() {return ERR! ("!LP_privkeycalc")}
        if !lp::G.LP_privkey.nonz() {return ERR! ("Error initializing the global private key (G.LP_privkey)")}
    }

    let c_ticker = try_s! (CString::new (ticker));
    let c_ticker = c_ticker.as_ptr() as *mut c_char;

    unsafe {lp::LP_priceinfoadd (c_ticker)};

    if !coins_en["etomic"].is_null() {
        let etomic = match coins_en["etomic"].as_str() {Some (s) => s, None => return ERR! ("Field 'etomic' is not a string")};
        try_s! (safecopy! (ii.etomic, "{}", etomic));
    }

    let method = req["method"].as_str();
    let utxo_mode = if method == Some ("electrum") {
        let urls: Vec<String> = try_s!(json::from_value(req["urls"].clone()));
        UtxoInitMode::Electrum(urls)
    } else if method == Some ("enable") {
        if unsafe {!lp::LP_conflicts_find (ii) .is_null()} {return ERR! ("coin port conflicts with existing coin")}
        UtxoInitMode::Native
    } else {
        return ERR! ("lp_coininit unknown method {:?}", method);
    };

    // TODO: Should pick the correct coin implementation somehow.
    //       Consider changing the config format to set the coin type `explicitly`.
    let coin: MmCoinEnum = if coins_en["etomic"].is_null() {
        try_s! (utxo_coin_from_iguana_info (ii, utxo_mode)) .into()
    } else {
        try_s! (eth_coin_from_iguana_info(ii)) .into()
    };

    // TODO we should return the error here instead of using the default 0 value because coin
    //      is not usable for sure in case of error on getting the block count.
    //      We can't put try_s! here due to default BTC and KMD initialization which we should get rid of.
    let block_count = coin.current_block().wait().unwrap_or(0);
    // TODO, #156: Warn the user when we know that the wallet is under-initialized.
    log! ([=ticker] if !coins_en["etomic"].is_null() {", etomic"} ", " [=method] ", " [=block_count]);
    if block_count == 0 {
        ii.inactive = (now_ms() / 1000) as u32
    } else {
        ii.inactive = 0;
        unsafe {lp::LP_unspents_load (ii.symbol.as_mut_ptr(), ii.smartaddr.as_mut_ptr())};
        // AG: I wonder why the special treatment.
        //     We invoke `LP_importaddress` during the SWAP, so maybe we can remove the "KMD"-only invocation here.
        if ticker == "KMD" {
            unsafe {lp::LP_importaddress (b"KMD\0".as_ptr() as *mut c_char, lp::BOTS_BONDADDRESS.as_ptr() as *mut c_char)};
        }
    }

    ve.insert (ticker.into(), coin.clone());
    Ok (coin)
}

/// Enable a coin in the local wallet mode.
pub fn enable (ctx: MmArc, req: Json) -> HyRes {
    let ticker = try_h! (req["coin"].as_str().ok_or ("No 'coin' field"));
    let coin: MmCoinEnum = try_h! (lp_coininit (&ctx, ticker, &req));
    Box::new(coin.my_balance().and_then(move |balance|
        rpc_response(200, json!({
            "result": "success",
            "address": coin.my_address(),
            "balance": balance
        }).to_string())
    ))
}

/// Enable a coin in the Electrum mode.
pub fn electrum (ctx: MmArc, req: Json) -> HyRes {
    let ticker = try_h! (req["coin"].as_str().ok_or ("No 'coin' field"));
    let coin: MmCoinEnum = try_h! (lp_coininit (&ctx, ticker, &req));
    Box::new(coin.my_balance().and_then(move |balance|
        rpc_response(200, json!({
            "result": "success",
            "address": coin.my_address(),
            "balance": balance
        }).to_string())
    ))
}

/*
int32_t LP_isdisabled(char *base,char *rel)
{
    struct iguana_info *coin;
    if ( base != 0 && (coin= LP_coinsearch(base)) != 0 && coin->inactive != 0 )
        return(1);
    else if ( rel != 0 && (coin= LP_coinsearch(rel)) != 0 && coin->inactive != 0 )
        return(1);
    else return(0);
}
*/

/// NB: As of now only a part of coin information has been ported to `MmCoinEnum`.
///     We plan to port the rest of it later (cf. `lp_coininit`).
///     Use the `iguana_info()` interface to access the C version meanwhile.
///
/// NB: Returns only the enabled (aka active) coins.
pub fn lp_coinfind (ctx: &MmArc, ticker: &str) -> Result<Option<MmCoinEnum>, String> {
    let cctx = try_s! (CoinsContext::from_ctx (ctx));
    let coins = try_s! (cctx.coins.lock());
    Ok (coins.get (ticker) .map (|coin| coin.clone()))
}

/*
void LP_otheraddress(char *destcoin,char *otheraddr,char *srccoin,char *coinaddr)
{
    uint8_t addrtype,rmd160[20]; struct iguana_info *src,*dest;
    if ( (src= LP_coinfind(srccoin)) != 0 && (dest= LP_coinfind(destcoin)) != 0 )
    {
        bitcoin_addr2rmd160(srccoin,src->taddr,&addrtype,rmd160,coinaddr);
        bitcoin_address(destcoin,otheraddr,dest->taddr,dest->pubtype,rmd160,20);
    } else printf("couldnt find %s or %s\n",srccoin,destcoin);
}
*/

pub fn lp_initcoins (ctx: &MmArc) -> Result<(), String> {
    // The function lived in the "LP_nativeDEX.c" originally, but I suspect that we can have it encapsulated in the `coins` proper.

    // NB: Most of the coins are initialized in `fn enable`. (Only knowing about the enabled coins
    //     is not only faster but also safer, ensuring that every coin we know of is valid and operable).
    //     But we currently need to initialize at least one coin here, in `fn lp_initcoins`,
    //     in order to set the global key pair with `LP_privkeycalc`.
    //     We also need the "KMD" initialized [and enabled] since it is currently used in `LP_pricepings` [etc].
    let default_coins = ["BTC", "KMD"];

    for &ticker in default_coins.iter() {
        try_s! (lp_coininit (ctx, ticker, &json!({"method":"enable"})));
    }

    Ok(())
}

/// Get my_balance of a coin
pub fn my_balance (ctx: MmArc, req: Json) -> HyRes {
    let ticker = try_h! (req["coin"].as_str().ok_or ("No 'coin' field")).to_owned();
    let coin = match lp_coinfind (&ctx, &ticker) {
        Ok (Some (t)) => t,
        Ok (None) => return rpc_err_response (500, &fomat! ("No such coin: " (ticker))),
        Err (err) => return rpc_err_response (500, &fomat! ("!lp_coinfind(" (ticker) "): " (err)))
    };
    Box::new(coin.my_balance().and_then(move |balance| rpc_response(200, json!({
        "coin": ticker,
        "balance": balance,
        "address": coin.my_address(),
    }).to_string())))
}

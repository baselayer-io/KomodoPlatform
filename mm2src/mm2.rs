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
//  mm2.rs
//  marketmaker
//
//  Copyright © 2017-2018 SuperNET. All rights reserved.
//

#![feature(non_ascii_idents)]

#[macro_use] extern crate common;
#[allow(unused_imports)]
#[macro_use] extern crate duct;
#[macro_use] extern crate fomat_macros;
#[macro_use] extern crate gstuff;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate serde_json;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate serialization_derive;
#[macro_use] extern crate unwrap;

use common::{bitcoin_priv2wif, lp, os, BitcoinCtx, CJSON, MM_VERSION};
use common::lp::{_bits256 as bits256};
use common::mm_ctx::MmCtx;

// Re-export preserves the functions that are temporarily accessed from C during the gradual port.
#[cfg(feature = "etomic")]
pub use etomicrs::*;

use gstuff::now_ms;

use libc::{c_char, c_int, c_void};

use rand::random;

use serde_json::{self as json, Value as Json};

use std::env;
use std::ffi::{CStr, CString, OsString};
use std::io::{self, Write};
use std::mem::{zeroed};
use std::net::{SocketAddr, Ipv4Addr};
use std::process::exit;
use std::ptr::{null};
use std::str::from_utf8_unchecked;
use std::slice::from_raw_parts;
use std::thread::sleep;
use std::time::Duration;
use std::str;

pub mod crash_reports;
use self::crash_reports::init_crash_reports;

mod lp_native_dex;
use self::lp_native_dex::{lp_init};

pub mod lp_network;
pub use self::lp_network::lp_queue_command;

pub mod lp_ordermatch;
pub mod lp_swap;
pub mod rpc;

#[cfg(test)]
mod mm2_tests;
/*
#include "LP_nativeDEX.c"

void LP_ports(uint16_t *pullportp,uint16_t *pubportp,uint16_t *busportp,uint16_t netid)
{
    int32_t netmod,netdiv; uint16_t otherports;
    *pullportp = *pubportp = *busportp = 0;
    if ( netid < 0 )
        netid = 0;
    else if ( netid > (65535-40-LP_RPCPORT)/4 )
    {
        printf("netid.%d overflow vs max netid.%d 14420?\n",netid,(65535-40-LP_RPCPORT)/4);
        exit(-1);
    }
    if ( netid != 0 )
    {
        netmod = (netid % 10);
        netdiv = (netid / 10);
        otherports = (netdiv * 40) + (LP_RPCPORT + netmod);
    } else otherports = LP_RPCPORT;
    *pullportp = otherports + 10;
    *pubportp = otherports + 20;
    *busportp = otherports + 30;
    printf("RPCport.%d remoteport.%d, nanoports %d %d %d\n",RPC_port,RPC_port-1,*pullportp,*pubportp,*busportp);
}
*/
fn lp_main (c_conf: CJSON, conf: Json) -> Result<(), String> {
    // Redirects the C stdout to the log.
    let c_log_path_buf: CString;
    let c_log_path = if conf["log"].is_null() {null()} else {
        let log = try_s! (conf["log"].as_str().ok_or ("log is not a string"));
        c_log_path_buf = try_s! (CString::new (log));
        c_log_path_buf.as_ptr()
    };
    unsafe {lp::unbuffered_output_support (c_log_path)};

    let (mut pullport, mut pubport, mut busport) = (0, 0, 0);
    if conf["passphrase"].is_string() {
        let profitmargin = conf["profitmargin"].as_f64();
        unsafe {lp::LP_profitratio += profitmargin.unwrap_or (0.)};
        let port = conf["rpcport"].as_u64().unwrap_or (lp::LP_RPCPORT as u64);
        if port < 1000 {return ERR! ("port < 1000")}
        if port > u16::max_value() as u64 {return ERR! ("port > u16")}
        let netid = conf["netid"].as_u64().unwrap_or (0) as u16;
        unsafe {lp::LP_ports (&mut pullport, &mut pubport, &mut busport, netid)};
        try_s! (lp_init (port as u16, pullport, pubport, conf, c_conf));
        Ok(())
    } else {ERR! ("!passphrase")}
}

fn help() {
    pintln! (
        "Command-line options.\n"
        "The first command-line argument is special and designates the mode.\n"
        "\n"
        "  help                  ..  Display this message.\n"
        "  btc2kmd {WIF or BTC}  ..  Convert a BTC WIF into a KMD WIF.\n"
        "  events                ..  Listen to a feed coming from a separate MM daemon and print it to stdout.\n"
        "  vanity {substring}    ..  Tries to find an address with the given substring.\n"
        "  nxt                   ..  Query the local NXT client (port 7876) regarding the SuperNET account in NXT.\n"
        "  {JSON configuration}  ..  Run the MarketMaker daemon.\n"
        "\n"
        "Some (but not all) of the JSON configuration parameters (* - required):\n"
        "\n"
        "  alice_contract ..  0x prefixed Alice ETH contract address.\n"
        "                     Default is 0x9bc5418ceded51db08467fc4b62f32c5d9ebda55 (Mainnet).\n"
        "                     Set 0xe1d4236c5774d35dc47dcc2e5e0ccfc463a3289c for Ropsten testnet\n"
        "  bob_contract   ..  0x prefixed Bob ETH contract address.\n"
        "                     Default is 0x2896Db79fAF20ABC8776fc27D15719cf59b8138B (Mainnet).\n"
        "                     Set 0x105aFE60fDC8B5c021092b09E8a042135A4A976E for Ropsten testnet\n"
        "  canbind        ..  If > 1000 and < 65536, initializes the `LP_fixed_pairport`.\n"
        // I'm not quite sure what the "client mode" is, should be clarified as soon as we learn it.
        // Does it turn Electrum-only mode on?
        // Does it turn us into the Alice (Buyer)?
        // Prevents us from fully joining the peer-to-peer network (by affecting `LP_canbind`)?
        // Anything else?
        // In MM2 we want to make the peers equal, allowing everyone to swap with everyone,
        // the client-server model doesn't make a lot of sense then.
        // cf. `IAMLP`, `MmCtx::am_client`.
        // ⇒ See also the "Order Matching with Full-Relay and Non-Relay Nodes" chapter in the Komodo Whitepaper.
        "  client         ..  '1' to use the client mode.\n"
        // We don't want to break the existing RPC API,
        // so the "refrel=coinmarketcap" designator will act as autoselect,
        // using the CoinGecko behind the scenes unless the "cmc_key" is given.
        // In the future, when MM2 is more widely used and thus we're working more tighly with the GUIs (BarterDEX, HyperDEX, dICO),
        // we might add the "refrel=cmc" and "refrel=coingecko" RPC options.
        "  cmc_key        ..  CoinMarketCap Professional API Key. Switches from CoinGecko to CoinMarketCap.\n"
        "                     The Key can be obtained from 'https://pro.coinmarketcap.com/account'.\n"
        "                     NB: The 'coins' command-line configuration must have the lowercased coin names in the 'name' field,\n"
      r#"                     {"coins": [{"name": "dash", "coin": "DASH", ...}, ...], ...}."# "\n"
        // cf. https://github.com/atomiclabs/hyperdex/blob/1d4ed3234b482e769124725c7e979eef5cd72d24/app/marketmaker/supported-currencies.js#L12
        "  coins          ..  Information about the currencies: their ticker symbols, names, ports, addresses, etc.\n"
        "                     If not present on the command line then we try the (stringified) coins.json and exchanges/coins.json.\n"
        "  dbdir          ..  MM database path. 'DB' by default.\n"
        "  ethnode        ..  HTTP url of ethereum node. Parity ONLY. Default is http://195.201.0.6:8555 (Mainnet).\n"
        "                     Set http://195.201.0.6:8545 for Ropsten testnet.\n"
        "  log            ..  File path. Redirect (as of now only a part of) the log there.\n"
        "  myipaddr       ..  IP address to bind to for P2P networking.\n"
        "  netid          ..  Subnetwork. Affects ports and keys.\n"
        "  passphrase *   ..  Wallet seed.\n"
        "  profitmargin   ..  Adds to `LP_profitratio`.\n"
        // cf. https://github.com/atomiclabs/hyperdex/pull/563/commits/6d17c0c994693b768e30130855c679a7849a2b27
        "  rpccors        ..  Access-Control-Allow-Origin header value to be used in all the RPC responses.\n"
        "                     Default is currently 'http://localhost:3000'\n"
        "  rpcip          ..  IP address to bind to for RPC server. Overrides the 127.0.0.1 default\n"
        "  rpcport        ..  If > 1000 overrides the 7783 default.\n"
        "  userhome       ..  System home directory of a user ('/root' by default).\n"
        "  wif            ..  `1` to add WIFs to the information we provide about a coin.\n"
        "\n"
        // Generated from https://github.com/KomodoPlatform/Documentation (PR to dev branch).
        // SHossain: "this would be the URL we would recommend and it will be maintained
        //            Please let @gcharang or me know if anything needs updating there".
        "See also the online documentation at https://docs.komodoplatform.com/barterDEX/barterDEX-API.html."
    )
}

fn main() {
    init_crash_reports();
    unsafe {os::OS_init()};
    log!({"BarterDEX MarketMaker {}", MM_VERSION});

    // Temporarily simulate `argv[]` for the C version of the main method.
    let args: Vec<String> = env::args().map (|mut arg| {arg.push ('\0'); arg}) .collect();
    let mut args: Vec<*const c_char> = args.iter().map (|s| s.as_ptr() as *const c_char) .collect();
    args.push (null());

    let args_os: Vec<OsString> = env::args_os().collect();

    // NB: The first argument is special, being used as the mode switcher.
    // The other arguments might be used to pass the data to the various MM modes,
    // we're not checking them for the mode switches in order not to risk [untrusted] data being mistaken for a mode switch.
    let first_arg = args_os.get (1) .and_then (|arg| arg.to_str());

    if first_arg == Some ("btc2kmd") && args_os.get (2) .is_some() {
        match btc2kmd (unwrap! (args_os[2].to_str(), "Bad argument encoding")) {
            Ok (output) => log! ((output)),
            Err (err) => log! ({"btc2kmd error] {}", err})
        }
        return
    }

    if let Err (err) = events (&args_os) {log! ({"events error] {}", err}); return}

    let second_arg = args_os.get (2) .and_then (|arg| arg.to_str());
    if first_arg == Some ("vanity") && second_arg.is_some() {vanity (unwrap! (second_arg)); return}

    if first_arg == Some ("--help") || first_arg == Some ("-h") || first_arg == Some ("help") {help(); return}
    if cfg! (windows) && first_arg == Some ("/?") {help(); return}

    if first_arg == Some ("nxt") {
        unsafe {lp::LP_NXT_redeems()};
        sleep (Duration::from_secs (3));
        return
    }

    if let Some (conf) = first_arg {
        if let Err (err) = run_lp_main (conf) {
            log! ((err));
            exit (1);
        }
    }
}

// TODO: `btc2kmd` is *pure*, it doesn't use shared state,
// though some of the underlying functions (`LP_convaddress`) do (the hash of cryptocurrencies is shared).
// Should mark it as shallowly pure.

/// Implements the "btc2kmd" command line utility.
fn btc2kmd (wif_or_btc: &str) -> Result<String, String> {
    extern "C" {
        fn LP_wifstr_valid (symbol: *const u8, wifstr: *const u8) -> i32;
        fn LP_convaddress (symbol: *const u8, address: *const u8, dest: *const u8) -> *const c_char;
        fn bitcoin_wif2priv (symbol: *const u8, wiftaddr: u8, addrtypep: *mut u8, privkeyp: *mut bits256, wifstr: *const c_char) -> i32;
    }

    let wif_or_btc_z = format! ("{}\0", wif_or_btc);
    /* (this line helps the IDE diff to match the old and new code)
    if ( strstr(argv[0],"btc2kmd") != 0 && argv[1] != 0 )
    */
    let mut privkey: bits256 = unsafe {zeroed()};
    let mut checkkey: bits256 = unsafe {zeroed()};
    let mut tmptype = 0;
    let mut kmdwif: [c_char; 64] = unsafe {zeroed()};
    if unsafe {LP_wifstr_valid (b"BTC\0".as_ptr(), wif_or_btc_z.as_ptr())} > 0 {
        let rc = unsafe {bitcoin_wif2priv (b"BTC\0".as_ptr(), 0, &mut tmptype, &mut privkey, wif_or_btc_z.as_ptr() as *const i8)};
        if rc < 0 {return ERR! ("!bitcoin_wif2priv")}
        let rc = unsafe {bitcoin_priv2wif (b"KMD\0".as_ptr(), 0, kmdwif.as_mut_ptr(), privkey, 188)};
        if rc < 0 {return ERR! ("!bitcoin_priv2wif")}
        let rc = unsafe {bitcoin_wif2priv (b"KMD\0".as_ptr(), 0, &mut tmptype, &mut checkkey, kmdwif.as_ptr())};
        if rc < 0 {return ERR! ("!bitcoin_wif2priv")}
        let kmdwif = try_s! (unsafe {CStr::from_ptr (kmdwif.as_ptr())} .to_str());
        if privkey == checkkey {
            Ok (format! ("BTC {} -> KMD {}: privkey {}", wif_or_btc, kmdwif, privkey))
        } else {
            Err (format! ("ERROR BTC {} {} != KMD {} {}", wif_or_btc, privkey, kmdwif, checkkey))
        }
    } else {
        let retstr = unsafe {LP_convaddress(b"BTC\0".as_ptr(), wif_or_btc_z.as_ptr(), b"KMD\0".as_ptr())};
        if retstr == null() {return ERR! ("LP_convaddress")}
        Ok (unwrap! (unsafe {CStr::from_ptr (retstr)} .to_str()) .into())
    }
}

/// Implements the `mm2 events` mode.  
/// If the command-line arguments match the events mode and everything else works then this function will never return.
fn events (args_os: &[OsString]) -> Result<(), String> {
    use common::nn::*;

    /*
    else if ( argv[1] != 0 && strcmp(argv[1],"events") == 0 )
    */
    if args_os.get (1) .and_then (|arg| arg.to_str()) .unwrap_or ("") == "events" {
        let ipc_endpoint = unsafe {nn_socket (AF_SP as c_int, NN_PAIR as c_int)};
        if ipc_endpoint < 0 {return ERR! ("!nn_socket")}
        let rc = unsafe {nn_connect (ipc_endpoint, "ws://127.0.0.1:5555\0".as_ptr() as *const c_char)};
        if rc < 0 {return ERR! ("!nn_connect")}
        loop {
            let mut buf: [u8; 1000000] = unsafe {zeroed()};
            let len = unsafe {nn_recv (ipc_endpoint, buf.as_mut_ptr() as *mut c_void, buf.len() - 1, 0)};
            if len >= 0 {
                let len = len as usize;
                assert! (len < buf.len());
                let stdout = io::stdout();
                let mut stdout = stdout.lock();
                try_s! (stdout.write_all (&buf[0..len]));
            }
        }
    }
    Ok(())
}

fn vanity (substring: &str) {
    extern "C" {
        fn bitcoin_priv2pub (
            ctx: *mut BitcoinCtx, symbol: *const u8, pubkey33: *mut u8, coinaddr: *mut u8,
            privkey: bits256, taddr: u8, addrtype: u8);
    }
    /*
    else if ( argv[1] != 0 && strcmp(argv[1],"vanity") == 0 && argv[2] != 0 )
    */
    let mut pubkey33: [u8; 33] = unsafe {zeroed()};
    let mut coinaddr: [u8; 64] = unsafe {zeroed()};
    let mut wifstr: [c_char; 128] = unsafe {zeroed()};
    let mut privkey: bits256 = unsafe {zeroed()};
    unsafe {lp::LP_mutex_init()};
    let ctx = MmCtx::new (json! ({}), SocketAddr::new (Ipv4Addr::new (127, 0, 0, 1) .into(), 123));
    unwrap! (coins::lp_initcoins (&ctx));
    let timestamp = now_ms() / 1000;
    log! ({"start vanitygen ({}).{} t.{}", substring, substring.len(), timestamp});
    for i in 0..1000000000 {
        privkey.bytes = random();
        unsafe {bitcoin_priv2pub (ctx.btc_ctx(), "KMD\0".as_ptr(), pubkey33.as_mut_ptr(), coinaddr.as_mut_ptr(), privkey, 0, 60)};
        let coinaddr = unsafe {from_utf8_unchecked (from_raw_parts (coinaddr.as_ptr(), 34))};
        if &coinaddr[1 .. substring.len()] == &substring[0 .. substring.len() - 1] {  // Print on near match.
            unsafe {bitcoin_priv2wif ("KMD\0".as_ptr(), 0, wifstr.as_mut_ptr(), privkey, 188)};
            let wifstr = unwrap! (unsafe {CStr::from_ptr (wifstr.as_ptr())} .to_str());
            log! ({"i.{} {} -> {} wif.{}", i, privkey, coinaddr, wifstr});
            if coinaddr.as_bytes()[substring.len()] == substring.as_bytes()[substring.len() - 1] {break}  // Stop on full match.
        }
    }
    log! ({"done vanitygen.({}) done {} elapsed {}\n", substring, now_ms() / 1000, now_ms() / 1000 - timestamp});
}

/// Parses the `first_argument` as JSON and starts LP_main.
fn run_lp_main (conf: &str) -> Result<(), String> {
    let c_conf = match CJSON::from_str (conf) {
        Ok (json) => json,
        Err (err) => return ERR! ("couldnt parse.({}).{}", conf, err)
    };
    let conf: Json = match json::from_str(conf) {
        Ok (json) => json,
        Err (err) => return ERR! ("couldnt parse.({}).{}", conf, err)
    };

    if conf["docker"] == 1 {
        unsafe {lp::DOCKERFLAG = 1}
    } else if conf["docker"].is_string() {
        let ip_port = unwrap! (CString::new (unwrap! (conf["docker"].as_str())));
        unsafe {lp::DOCKERFLAG = os::calc_ipbits (ip_port.as_ptr() as *mut c_char) as u32}
    }

    try_s! (lp_main (c_conf, conf));
    Ok(())
}

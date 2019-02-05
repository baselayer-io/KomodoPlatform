use common::identity;
use common::for_tests::{enable_electrum, from_env_file, mm_dump, mm_spat, LocalStart, MarketMakerIt, RaiiDump, RaiiKill};
use dirs;
use gstuff::{now_float, slurp};
use hyper::StatusCode;
use hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN;
use libc::c_char;
use peers;
use serde_json::{self as json, Value as Json};
use std::borrow::Cow;
use std::env::{self, var};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::str::{from_utf8_unchecked};
use std::thread::{self, sleep};
use std::time::Duration;

/// Asks MM to enable the given currency in native mode.  
/// Returns the RPC reply containing the corresponding wallet address.
fn enable_native(mm: &MarketMakerIt, coin: &str) -> String {
    let native = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "method": "enable",
        "coin": coin,
    })));
    assert_eq! (native.0, StatusCode::OK, "'enable' failed: {}", native.1);
    native.1
}

/// Enables BEER, PIZZA, ETOMIC and ETH.
/// Returns the RPC replies containing the corresponding wallet addresses.
fn enable_coins(mm: &MarketMakerIt) -> Vec<(&'static str, String)> {
    let mut replies = Vec::new();
    replies.push (("BEER", enable_native (mm, "BEER")));
    replies.push (("PIZZA", enable_native (mm, "PIZZA")));
    replies.push (("ETOMIC", enable_native(mm, "ETOMIC")));
    replies.push (("ETH", enable_native (mm, "ETH")));
    replies
}

fn enable_coins_electrum(mm: &MarketMakerIt) -> Vec<(&'static str, String)> {
    let mut replies = Vec::new();
    replies.push (("BEER", enable_electrum (mm, "BEER", vec!["electrum1.cipig.net:10022","electrum2.cipig.net:10022","electrum3.cipig.net:10022"])));
    replies.push (("PIZZA", enable_electrum (mm, "PIZZA", vec!["electrum1.cipig.net:10024","electrum2.cipig.net:10024","electrum3.cipig.net:10024"])));
    replies.push (("ETOMIC", enable_electrum (mm, "ETOMIC", vec!["electrum1.cipig.net:10025","electrum2.cipig.net:10025","electrum3.cipig.net:10025"])));
    replies
}

#[test]
fn test_autoprice_coingecko() {portfolio::portfolio_tests::test_autoprice_coingecko (local_start())}

#[test]
fn test_autoprice_coinmarketcap() {portfolio::portfolio_tests::test_autoprice_coinmarketcap (local_start())}

#[test]
fn test_fundvalue() {portfolio::portfolio_tests::test_fundvalue (local_start())}

/// Integration test for RPC server.
/// Check that MM doesn't crash in case of invalid RPC requests
#[test]
fn test_rpc() {
    let (_, mut mm, _dump_log, _dump_dashboard) = mm_spat (local_start(), &identity);
    unwrap! (mm.wait_for_log (19., &|log| log.contains (">>>>>>>>> DEX stats ")));

    let no_method = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "coin": "BEER",
        "ipaddr": "electrum1.cipig.net",
        "port": 10022
    })));
    assert! (no_method.0.is_server_error());
    assert_eq!((no_method.2)[ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:4000");

    let not_json = unwrap! (mm.rpc_str("It's just a string"));
    assert! (not_json.0.is_server_error());
    assert_eq!((not_json.2)[ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:4000");

    let unknown_method = unwrap! (mm.rpc (json! ({
        "method": "unknown_method",
    })));

    assert! (unknown_method.0.is_server_error());
    assert_eq!((unknown_method.2)[ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:4000");

    let mpnet = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "method": "mpnet",
        "onoff": 1,
    })));
    assert_eq!(mpnet.0, StatusCode::OK);
    assert_eq!((mpnet.2)[ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:4000");

    unwrap! (mm.wait_for_log (1., &|log| log.contains ("MPNET onoff")));

    let version = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "method": "version",
    })));
    assert_eq!(version.0, StatusCode::OK);
    assert_eq!((version.2)[ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:4000");

    let help = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "method": "help",
    })));
    assert_eq!(help.0, StatusCode::OK);
    assert_eq!((help.2)[ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:4000");

    unwrap! (mm.stop());
    unwrap! (mm.wait_for_log (9., &|log| log.contains ("on_stop] firing shutdown_tx!")));
    // TODO (workaround libtorrent hanging in delete) // unwrap! (mm.wait_for_log (9., &|log| log.contains ("LogState] Bye!")));
}

use super::{btc2kmd, events, lp_main, CJSON};

/// Integration (?) test for the "btc2kmd" command line invocation.
/// The argument is the WIF example from https://en.bitcoin.it/wiki/Wallet_import_format.
#[test]
fn test_btc2kmd() {
    let output = unwrap! (btc2kmd ("5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"));
    assert_eq! (output, "BTC 5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ \
    -> KMD UpRBUQtkA5WqFnSztd7sCYyyhtd4aq6AggQ9sXFh2fXeSnLHtd3Z: \
    privkey 0c28fca386c7a227600b2fe50b7cae11ec86d3bf1fbe471be89827e19d72aa1d");
}

/// This is not a separate test but a helper used by `MarketMakerIt` to run the MarketMaker from the test binary.
#[test]
fn test_mm_start() {
    if let Ok (conf) = var ("_MM2_TEST_CONF") {
        log! ("test_mm_start] Starting the MarketMaker...");
        let conf: Json = unwrap! (json::from_str (&conf));
        let c_json = unwrap! (CString::new (unwrap! (json::to_string (&conf))));
        let c_conf = unwrap! (CJSON::from_zero_terminated (c_json.as_ptr() as *const c_char));
        unwrap! (lp_main (c_conf, conf))
    }
}

#[cfg(windows)]
fn chdir (dir: &Path) {
    use winapi::um::processenv::SetCurrentDirectoryA;

    let dir = unwrap! (dir.to_str());
    let dir = unwrap! (CString::new (dir));
    // https://docs.microsoft.com/en-us/windows/desktop/api/WinBase/nf-winbase-setcurrentdirectory
    let rc = unsafe {SetCurrentDirectoryA (dir.as_ptr())};
    assert_ne! (rc, 0);
}

#[cfg(not(windows))]
fn chdir (dir: &Path) {
    unwrap! (nix::unistd::chdir (dir))
}

/// Typically used when the `LOCAL_THREAD_MM` env is set, helping debug the tested MM.  
/// NB: Accessing `lp_main` this function have to reside in the mm2 binary crate. We pass a pointer to it to subcrates.
fn local_start_impl (folder: PathBuf, log_path: PathBuf, mut conf: Json) {
    unwrap! (thread::Builder::new().name ("MM".into()) .spawn (move || {
        if conf["log"].is_null() {
            conf["log"] = unwrap! (log_path.to_str()) .into();
        } else {
            let path = Path::new (unwrap! (conf["log"].as_str(), "log is not a string"));
            assert_eq! (log_path, path);
        }

        log! ({"local_start] MM in a thread, log {:?}.", log_path});

        chdir (&folder);

        let c_json = unwrap! (CString::new (unwrap! (json::to_string (&conf))));
        let c_conf = unwrap! (CJSON::from_zero_terminated (c_json.as_ptr() as *const c_char));
        unwrap! (lp_main (c_conf, conf))
    }));
}

fn local_start() -> LocalStart {local_start_impl}

/// Integration test for the "mm2 events" mode.
/// Starts MM in background and verifies that "mm2 events" produces a non-empty feed of events.
#[test]
fn test_events() {
    let executable = unwrap! (env::args().next());
    let executable = unwrap! (Path::new (&executable) .canonicalize());
    match var ("_MM2_TEST_EVENTS_MODE") {
        Ok (ref mode) if mode == "MM_EVENTS" => {
            log! ("test_events] Starting the `mm2 events`...");
            unwrap! (events (&["_test".into(), "events".into()]));
        },
        _ => {
            let conf = json! ({"gui": "nogui", "passphrase": "123", "coins": "BTC,KMD"});
            log! ("Starting the MarketMaker, similar to\n  gdb --args target/debug/mm2 '" (unwrap! (json::to_string (&conf))) "'\n  run");
            let mut mm = unwrap! (MarketMakerIt::start (
                conf,
                "5bfaeae675f043461416861c3558146bf7623526891d890dc96bc5e0e5dbc337".into(),
                match var ("LOCAL_THREAD_MM") {Ok (ref e) if e == "1" => Some (local_start()), _ => None}));
            let (_dump_log, _dump_dashboard) = mm_dump (&mm.log_path);

            let expected_bind = format! (">>>>>>>>> DEX stats {}:7783", mm.ip);
            unwrap! (mm.wait_for_log (22., &|log| log.contains (&expected_bind)));

            log! ("Asking for a websocket endpoint with a stream of events…");
            let (status, body, _headers) = unwrap! (mm.rpc (json! ({"userpass": mm.userpass, "method": "getendpoint"})));
            log! ({"test_events] getendpoint response: {:?}, {}", status, body});
            assert_eq! (status, StatusCode::OK);
            //let expected_endpoint = format! ("\"endpoint\":\"ws://{}:5555\"", mm.ip);
            assert! (body.contains ("\"endpoint\":\"ws://127.0.0.1:5555\""), "{}", body);

            log! ("Starting 'mm2 events', " [executable] "…");
            let mm_events_output = mm.folder.join ("mm2_events.log");
            let mut mm_events = RaiiKill::from_handle (unwrap! (cmd! (executable, "test_events", "--nocapture")
                .dir (env::temp_dir())
                .env ("_MM2_TEST_EVENTS_MODE", "MM_EVENTS")
                .env ("MM2_UNBUFFERED_OUTPUT", "1")
                .stderr_to_stdout().stdout (&mm_events_output) .start()));

            log! ("Waiting for 'mm2 events' to print some interesting data, " [mm_events_output] "…");
            let mm_events_output = RaiiDump {log_path: mm_events_output};
            let start = now_float();
            loop {
                if let Some (ref mut pc) = mm.pc {if !pc.running() {panic! ("MM process terminated prematurely.")}}
                if !mm_events.running() {panic! ("`mm2 events` terminated prematurely.")}
                let log = slurp (&mm_events_output.log_path);
                let log = unsafe {from_utf8_unchecked (&log)};
                if log.contains ("\"base\":\"KMD\"") && log.contains ("\"price64\":\"") {break}  // Gotcha!
                if now_float() - start > 123. {panic! ("Timeout expired waiting for data on event stream")}
                sleep (Duration::from_secs (1));
            }

            log! ("Stopping MMs…");
            unwrap! (mm.stop());
            sleep (Duration::from_millis (100));
        }
    }
}

/// Invokes the RPC "notify" method, adding a node to the peer-to-peer ring.
#[test]
fn test_notify() {
    let (_passphrase, mut mm, _dump_log, _dump_dashboard) = mm_spat (local_start(), &identity);
    unwrap! (mm.wait_for_log (19., &|log| log.contains (">>>>>>>>> DEX stats ")));

    let notify = unwrap! (mm.rpc (json! ({
        "method": "notify",
        "rmd160": "9562c4033b6ac1ea2378636a782ce5fdf7ee9a2d",
        "pub": "5eb48483573d44f1b24e33414273384c2f0ae15ecab7f700fb3042f904b09820",
        "pubsecp": "0342407c81e408d9d6cdec35576d7284b712ee4062cb908574b5bc6bb46406f8ad",
        "timestamp": 1541434098,
        "sig":  "1f1e2198d890eeb2fc0004d092ff1266c1be10ca16a0cbe169652c2dc1b3150e5918fd9c7fc5161a8f05f4384eb05fc92e4e9c1abb551795f447b0433954f29990",
        "isLP": "45.32.19.196",
        "session": 1540419658,
    })));
    assert_eq! (notify.0, StatusCode::OK, "notify reply: {:?}", notify);
    //unwrap! (mm.wait_for_log (9., &|log| log.contains ("lp_notify_recv] hailed by peer: 45.32.19.196")));
}

/// https://github.com/artemii235/SuperNET/issues/241
#[test]
fn alice_can_see_the_active_order_after_connection() {
    let coins = json!([
        {"coin":"BEER","asset":"BEER","rpcport":8923,"txversion":4},
        {"coin":"PIZZA","asset":"PIZZA","rpcport":11608,"txversion":4},
        {"coin":"ETOMIC","asset":"ETOMIC","rpcport":10271,"txversion":4},
        {"coin":"ETH","name":"ethereum","etomic":"0x0000000000000000000000000000000000000000","rpcport":80}
    ]);

    // start bob and immediately place the order
    let mut mm_bob = unwrap! (MarketMakerIt::start (
        json! ({
            "gui": "nogui",
            "netid": 9998,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "canbind": env::var ("BOB_TRADE_PORT") .ok().map (|s| unwrap! (s.parse::<i64>())),
            "passphrase": "bob passphrase",
            "coins": coins,
        }),
        "db4be27033b636c6644c356ded97b0ad08914fcb8a1e2a1efc915b833c2cbd19".into(),
        match var ("LOCAL_THREAD_MM") {Ok (ref e) if e == "bob" => Some (local_start()), _ => None}
    ));
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump (&mm_bob.log_path);
    log!({"Bob log path: {}", mm_bob.log_path.display()});
    unwrap! (mm_bob.wait_for_log (22., &|log| log.contains (">>>>>>>>> DEX stats ")));
    // Enable coins on Bob side. Print the replies in case we need the "address".
    log! ({"enable_coins (bob): {:?}", enable_coins_electrum (&mm_bob)});
    // issue sell request on Bob side by setting base/rel price
    log!("Issue bob sell request");
    let rc = unwrap! (mm_bob.rpc (json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "BEER",
        "rel": "PIZZA",
        "price": 0.9
    })));
    assert! (rc.0.is_success(), "!setprice: {}", rc.1);

    thread::sleep(Duration::from_secs(10));

    // Bob orderbook must show the new order
    log!("Get BEER/PIZZA orderbook on Bob side");
    let rc = unwrap! (mm_bob.rpc (json! ({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "BEER",
        "rel": "PIZZA",
    })));
    assert! (rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = unwrap!(json::from_str(&rc.1));
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert!(asks.len() > 0, "Bob BEER/PIZZA asks are empty");
    let vol = asks[0]["maxvolume"].as_f64().unwrap();
    assert_eq!(vol, 1.0);

    log!("Bob orderbook " [bob_orderbook]);
    let mut mm_alice = unwrap! (MarketMakerIt::start (
        json! ({
            "gui": "nogui",
            "netid": 9998,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var ("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var ("ALICE_TRADE_IP") .ok(),
            "passphrase": "alice passphrase",
            "coins": coins,
            "seednode": fomat!((mm_bob.ip)),
            "alice_contract":"0xe1d4236c5774d35dc47dcc2e5e0ccfc463a3289c",
            "bob_contract":"0x105aFE60fDC8B5c021092b09E8a042135A4A976E",
            "ethnode":"http://195.201.0.6:8545"
        }),
        "08bfc72a25d860a6399005abc27d1aea35318c137fbaad686ab8d59f5716b473".into(),
        match var ("LOCAL_THREAD_MM") {Ok (ref e) if e == "alice" => Some (local_start()), _ => None}
    ));

    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump (&mm_alice.log_path);
    log!({"Alice log path: {}", mm_alice.log_path.display()});

    unwrap! (mm_alice.wait_for_log (22., &|log| log.contains (">>>>>>>>> DEX stats ")));

    // Enable coins on Alice side. Print the replies in case we need the "address".
    log! ({"enable_coins (alice): {:?}", enable_coins_electrum (&mm_alice)});

    // wait until Alice recognize Bob node by importing it's pubkey
    unwrap! (mm_alice.wait_for_log (33., &|log| log.contains ("set pubkey for")));

    for _ in 0..2 {
        // Alice should be able to see the order no later than 10 seconds after recognizing the bob
        thread::sleep(Duration::from_secs(10));
        log!("Get BEER/PIZZA orderbook on Alice side");
        let rc = unwrap!(mm_alice.rpc (json! ({
            "userpass": mm_alice.userpass,
            "method": "orderbook",
            "base": "BEER",
            "rel": "PIZZA",
        })));
        assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

        let alice_orderbook: Json = unwrap!(json::from_str(&rc.1));
        let asks = alice_orderbook["asks"].as_array().unwrap();
        assert_eq!(asks.len(), 1, "Alice BEER/PIZZA orderbook must have exactly 1 ask");
        let vol = asks[0]["maxvolume"].as_f64().unwrap();
        assert_eq!(vol, 1.0);
    }
}

#[test]
fn test_status() {common::log::tests::test_status()}

#[test]
fn test_peers_dht() {peers::peers_tests::test_peers_dht()}

#[test]
fn test_peers_direct_send() {peers::peers_tests::test_peers_direct_send()}

#[test]
fn test_my_balance() {
    let coins = json!([
        {"coin":"BEER","asset":"BEER","rpcport":8923,"txversion":4},
    ]);

    let mut mm = unwrap! (MarketMakerIt::start (
        json! ({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "passphrase": "bob passphrase",
            "coins": coins,
            "alice_contract":"0xe1d4236c5774d35dc47dcc2e5e0ccfc463a3289c",
            "bob_contract":"0x105aFE60fDC8B5c021092b09E8a042135A4A976E",
            "ethnode":"http://195.201.0.6:8545"
        }),
        "db4be27033b636c6644c356ded97b0ad08914fcb8a1e2a1efc915b833c2cbd19".into(),
        match var ("LOCAL_THREAD_MM") {Ok (ref e) if e == "bob" => Some (local_start()), _ => None}
    ));
    let (_dump_log, _dump_dashboard) = mm_dump (&mm.log_path);
    log!({"log path: {}", mm.log_path.display()});
    unwrap! (mm.wait_for_log (22., &|log| log.contains (">>>>>>>>> DEX stats ")));
    // Enable BEER.
    let enable = enable_electrum(&mm, "BEER", vec!["electrum1.cipig.net:10022","electrum2.cipig.net:10022","electrum3.cipig.net:10022"]);
    let json: Json = unwrap!(json::from_str(&enable));
    let balance_on_enable = unwrap!(json["balance"].as_f64());
    assert_eq!(balance_on_enable, 1.0);

    let my_balance = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "method": "my_balance",
        "coin": "BEER",
    })));
    assert_eq! (my_balance.0, StatusCode::OK, "RPC «my_balance» failed with status «{}»", my_balance.0);
    let json: Json = unwrap!(json::from_str(&my_balance.1));
    let my_balance = unwrap!(json["balance"].as_f64());
    assert_eq!(my_balance, 1.0);
    let my_address = unwrap!(json["address"].as_str());
    assert_eq!(my_address, "RRnMcSeKiLrNdbp91qNVQwwXx5azD4S4CD");
}

#[test]
fn test_check_balance_on_order_post() {
    let coins = json!([
        {"coin":"BEER","asset":"BEER","rpcport":8923,"txversion":4},
        {"coin":"PIZZA","asset":"PIZZA","rpcport":11608,"txversion":4},
        {"coin":"ETOMIC","asset":"ETOMIC","rpcport":10271,"txversion":4},
        {"coin":"ETH","name":"ethereum","etomic":"0x0000000000000000000000000000000000000000","rpcport":80}
    ]);

    // start bob and immediately place the order
    let mut mm = unwrap! (MarketMakerIt::start (
        json! ({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "canbind": env::var ("BOB_TRADE_PORT") .ok().map (|s| unwrap! (s.parse::<i64>())),
            "passphrase": "bob passphrase",
            "coins": coins,
            "alice_contract":"0xe1d4236c5774d35dc47dcc2e5e0ccfc463a3289c",
            "bob_contract":"0x105aFE60fDC8B5c021092b09E8a042135A4A976E",
            "ethnode":"http://195.201.0.6:8545"
        }),
        "db4be27033b636c6644c356ded97b0ad08914fcb8a1e2a1efc915b833c2cbd19".into(),
        match var ("LOCAL_THREAD_MM") {Ok (ref e) if e == "bob" => Some (local_start()), _ => None}
    ));
    let (_dump_log, _dump_dashboard) = mm_dump (&mm.log_path);
    log!({"Log path: {}", mm.log_path.display()});
    unwrap! (mm.wait_for_log (22., &|log| log.contains (">>>>>>>>> DEX stats ")));
    // Enable coins. Print the replies in case we need the "address".
    log! ({"enable_coins (bob): {:?}", enable_coins_electrum (&mm)});
    // issue sell request by setting base/rel price
    let rc = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "PIZZA",
        "rel": "BEER",
        "price": 0.9
    })));
    // Expect error as PIZZA balance is 0
    assert! (rc.0.is_server_error(), "!setprice success but should be error: {}", rc.1);

    // issue buy request
    let rc = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "BEER",
        "rel": "PIZZA",
        "relvolume": 0.1,
        "price": 0.9
    })));
    // Expect error as PIZZA balance is 0
    assert! (rc.0.is_server_error(), "!buy success but should be error: {}", rc.1);

    // issue sell request
    let rc = unwrap! (mm.rpc (json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "PIZZA",
        "rel": "BEER",
        "basevolume": 0.1,
        "price": 0.9
    })));
    // Expect error as PIZZA balance is 0
    assert! (rc.0.is_server_error(), "!sell success but should be error: {}", rc.1);
}

#[cfg(windows)]
fn get_special_folder_path() -> PathBuf {
    use std::ffi::CStr;
    use std::mem::zeroed;
    use std::ptr::null_mut;
    use winapi::um::shlobj::SHGetSpecialFolderPathA;
    use winapi::shared::minwindef::MAX_PATH;
    use winapi::um::shlobj::CSIDL_APPDATA;

    let mut buf: [c_char; MAX_PATH + 1] = unsafe {zeroed()};
    // https://docs.microsoft.com/en-us/windows/desktop/api/shlobj_core/nf-shlobj_core-shgetspecialfolderpatha
    let rc = unsafe {SHGetSpecialFolderPathA (null_mut(), buf.as_mut_ptr(), CSIDL_APPDATA, 1)};
    if rc != 1 {panic! ("!SHGetSpecialFolderPathA")}
    Path::new (unwrap! (unsafe {CStr::from_ptr (buf.as_ptr())} .to_str())) .to_path_buf()
}

#[cfg(not(windows))]
fn get_special_folder_path() -> PathBuf {panic!("!windows")}

/// Determines komodod conf file location, emulating komodo/util.cpp/GetConfigFile.
fn komodo_conf_path (ac_name: Option<&'static str>) -> Result<PathBuf, String> {
    let confname: Cow<str> = if let Some (ac_name) = ac_name {
        format! ("{}.conf", ac_name).into()
    } else {
        "komodo.conf".into()
    };

    // komodo/util.cpp/GetDefaultDataDir

    let mut path = match dirs::home_dir() {
        Some (hd) => hd,
        None => Path::new ("/") .to_path_buf()
    };

    if cfg! (windows) {
        // >= Vista: c:\Users\$username\AppData\Roaming
        path = get_special_folder_path();
        path.push ("Komodo");
    } else if cfg! (target_os = "macos") {
        path.push ("Library");
        path.push ("Application Support");
        path.push ("Komodo");
    } else {
        path.push (".komodo");
    }

    if let Some (ac_name) = ac_name {path.push (ac_name)}
    Ok (path.join (&confname[..]))
}

fn trade_base_rel(base: &str, rel: &str) {
    // Keep BEER here for some time as coin maybe will be back
    let beer_cfp = unwrap! (komodo_conf_path (Some ("BEER")));
    let pizza_cfp = unwrap! (komodo_conf_path (Some ("PIZZA")));
    let etomic_cfp = unwrap! (komodo_conf_path (Some ("ETOMIC")));
    assert! (beer_cfp.exists(), "BEER config {:?} is not found", beer_cfp);
    assert! (pizza_cfp.exists(), "PIZZA config {:?} is not found", pizza_cfp);
    assert! (etomic_cfp.exists(), "ETOMIC config {:?} is not found", etomic_cfp);

    let (bob_file_passphrase, bob_file_userpass) = from_env_file (slurp (&".env.seed"));
    let (alice_file_passphrase, alice_file_userpass) = from_env_file (slurp (&".env.client"));

    let bob_passphrase = unwrap! (var ("BOB_PASSPHRASE") .ok().or (bob_file_passphrase), "No BOB_PASSPHRASE or .env.seed/PASSPHRASE");
    let bob_userpass = unwrap! (var ("BOB_USERPASS") .ok().or (bob_file_userpass), "No BOB_USERPASS or .env.seed/USERPASS");
    let alice_passphrase = unwrap! (var ("ALICE_PASSPHRASE") .ok().or (alice_file_passphrase), "No ALICE_PASSPHRASE or .env.client/PASSPHRASE");
    let alice_userpass = unwrap! (var ("ALICE_USERPASS") .ok().or (alice_file_userpass), "No ALICE_USERPASS or .env.client/USERPASS");

    let coins = json!([
        {"coin":"BEER","asset":"BEER","rpcport":8923,"confpath":unwrap!(beer_cfp.to_str())},
        {"coin":"PIZZA","asset":"PIZZA","rpcport":11608,"confpath":unwrap!(pizza_cfp.to_str())},
        {"coin":"ETOMIC","asset":"ETOMIC","rpcport":10271,"confpath":unwrap!(etomic_cfp.to_str())},
        {"coin":"ETH","name":"ethereum","etomic":"0x0000000000000000000000000000000000000000","rpcport":80}
    ]);

    let mut mm_bob = unwrap! (MarketMakerIt::start (
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "canbind": env::var ("BOB_TRADE_PORT") .ok().map (|s| unwrap! (s.parse::<i64>())),
            "passphrase": bob_passphrase,
            "coins": coins,
        }),
        bob_userpass,
        match var ("LOCAL_THREAD_MM") {Ok (ref e) if e == "bob" => Some (local_start()), _ => None}
    ));

    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump (&mm_bob.log_path);
    log!({"Bob log path: {}", mm_bob.log_path.display()});

    // Both Alice and Bob might try to bind on the "0.0.0.0:47773" DHT port in this test
    // (because the local "127.0.0.*:47773" addresses aren't that useful for DHT).
    // We want to give Bob a headstart in acquiring the port,
    // because Alice will then be able to directly reach it (thanks to "seednode").
    // Direct communication is not required in this test, but it's nice to have.
    // The port differs for another netid, should be 43804 for 9000
    unwrap! (mm_bob.wait_for_log (9., &|log| log.contains ("preferred port 43804 drill true")));

    let mut mm_alice = unwrap! (MarketMakerIt::start (
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var ("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var ("ALICE_TRADE_IP") .ok(),
            "passphrase": alice_passphrase,
            "coins": coins,
            // We're using the open (non-NAT) netid 9000 seed instead, 195.201.42.102 // "seednode": fomat!((mm_bob.ip))
        }),
        alice_userpass,
        match var ("LOCAL_THREAD_MM") {Ok (ref e) if e == "alice" => Some (local_start()), _ => None}
    ));

    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump (&mm_alice.log_path);
    log!({"Alice log path: {}", mm_alice.log_path.display()});

    // wait until both nodes RPC API is active
    unwrap! (mm_bob.wait_for_log (22., &|log| log.contains (">>>>>>>>> DEX stats ")));
    unwrap! (mm_alice.wait_for_log (22., &|log| log.contains (">>>>>>>>> DEX stats ")));

    // Enable coins on Bob side. Print the replies in case we need the "smartaddress".
    log! ({"enable_coins (bob): {:?}", enable_coins (&mm_bob)});
    // Enable coins on Alice side. Print the replies in case we need the "smartaddress".
    log! ({"enable_coins (alice): {:?}", enable_coins (&mm_alice)});

    // Both the Taker and the Maker should connect to the netid 9000 open (non-NAT) seed node.
    // NB: Long wayt as there might be delays in the seed node from us reusing the 127.0.0.* IPs with different keys.
    unwrap! (mm_bob.wait_for_log (999., &|log| log.contains ("set pubkey for ")));
    unwrap! (mm_alice.wait_for_log (99., &|log| log.contains ("set pubkey for ")));

    // issue sell request on Bob side by setting base/rel price
    log!("Issue bob sell request");
    let rc = unwrap! (mm_bob.rpc (json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": base,
        "rel": rel,
        "price": 0.9
    })));
    assert! (rc.0.is_success(), "!setprice: {}", rc.1);

    // issue base/rel buy request from Alice side
    thread::sleep(Duration::from_secs(2));
    log!("Issue alice buy request");
    let rc = unwrap! (mm_alice.rpc (json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": base,
        "rel": rel,
        "relvolume": 0.1,  // Should be close enough to the existing UTXOs.
        "price": 1
    })));
    assert! (rc.0.is_success(), "!buy: {}", rc.1);

    // ensure the swap started
    unwrap! (mm_alice.wait_for_log (99., &|log| log.contains ("Entering the taker_swap_loop")));
    unwrap! (mm_bob.wait_for_log (20., &|log| log.contains ("Entering the maker_swap_loop")));

    // wait for swap to complete on both sides
    unwrap! (mm_alice.wait_for_log (600., &|log| log.contains ("Swap finished successfully")));
    unwrap! (mm_bob.wait_for_log (600., &|log| log.contains ("Swap finished successfully")));

    unwrap! (mm_bob.stop());
    unwrap! (mm_alice.stop());
}

/// Integration test for PIZZA/BEER and BEER/PIZZA trade
/// This test is ignored because as of now it requires additional environment setup:
/// PIZZA and ETOMIC daemons must be running and fully synced for swaps to be successful
/// The trades can't be executed concurrently now for 2 reasons:
/// 1. Bob node starts listening 47772 port on all interfaces so no more Bobs can be started at once
/// 2. Current UTXO handling algo might result to conflicts between concurrently running nodes
/// 
/// Steps that are currently necessary to run this test:
/// 
/// Obtain the wallet binaries (komodod, komodo-cli) from the [Agama wallet](https://github.com/KomodoPlatform/Agama/releases/).
/// (Or use the Docker image artempikulin/komodod-etomic).
/// (Or compile them from [source](https://github.com/jl777/komodo/tree/dev))
/// 
/// Obtain ~/.zcash-params (c:/Users/$username/AppData/Roaming/ZcashParams on Windows).
/// 
/// Start the wallets
/// 
///     komodod -ac_name=PIZZA -ac_supply=100000000 -addnode=24.54.206.138 -addnode=78.47.196.146
/// 
/// and
/// 
///     komodod -ac_name=ETOMIC -ac_supply=100000000 -addnode=78.47.196.146
/// 
/// and (if you want to test BEER coin):
///
///     komodod -ac_name=BEER -ac_supply=100000000 -addnode=78.47.196.146 -addnode=43.245.162.106 -addnode=88.99.153.2 -addnode=94.130.173.120 -addnode=195.201.12.150 -addnode=23.152.0.28
///
/// Get rpcuser and rpcpassword from ETOMIC/ETOMIC.conf
/// (c:/Users/$username/AppData/Roaming/Komodo/ETOMIC/ETOMIC.conf on Windows)
/// and run
/// 
///     komodo-cli -ac_name=ETOMIC importaddress RKGn1jkeS7VNLfwY74esW7a8JFfLNj1Yoo
/// 
/// Share the wallet information with the test. On Windows:
/// 
///     set BOB_PASSPHRASE=...
///     set BOB_USERPASS=...
///     set ALICE_PASSPHRASE=...
///     set ALICE_USERPASS=...
/// 
/// And run the test:
/// 
///     cargo test trade -- --nocapture --ignored
#[test]
#[ignore]
fn trade_pizza_eth() {
    trade_base_rel("PIZZA", "ETH");
}

#[test]
#[ignore]
fn trade_eth_pizza() {
    trade_base_rel("ETH", "PIZZA");
}

#[test]
#[ignore]
fn trade_beer_eth() {
    trade_base_rel("BEER", "ETH");
}

#[test]
#[ignore]
fn trade_eth_beer() {
    trade_base_rel("ETH", "BEER");
}

#[test]
#[ignore]
fn trade_pizza_beer() {
    trade_base_rel("PIZZA", "BEER");
}

#[test]
#[ignore]
fn trade_beer_pizza() {
    trade_base_rel("BEER", "PIZZA");
}

#[test]
#[ignore]
fn trade_pizza_etomic() {
    trade_base_rel("PIZZA", "ETOMIC");
}

#[test]
#[ignore]
fn trade_etomic_pizza() {
    trade_base_rel("ETOMIC", "PIZZA");
}

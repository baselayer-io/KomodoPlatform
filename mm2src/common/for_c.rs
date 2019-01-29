use hashbrown::HashMap;
use libc::c_char;
use std::ffi::CStr;
use std::mem::size_of;
use std::net::IpAddr;
use std::ptr::null_mut;
use std::sync::Mutex;

use super::lp;

// Example of a function forward:
//lazy_static! {pub static ref PEERS_SEND_COMPAT: Mutex<Option<fn (u32, i32, *const u8, i32) -> i32>> = Mutex::new (None);}

#[no_mangle]
pub extern fn log_stacktrace (desc: *const c_char) {
    let desc = if desc.is_null() {
        ""
    } else {
        match unsafe {CStr::from_ptr (desc)} .to_str() {
            Ok (s) => s,
            Err (err) => {
                log! ({"log_stacktrace] Bad trace description: {}", err});
                ""
            }
        }
    };
    let mut trace = String::with_capacity (4096);
    super::stack_trace (&mut super::stack_trace_frame, &mut |l| trace.push_str (l));
    log! ({"Stacktrace. {}\n{}", desc, trace});
}

pub struct IISafe (pub *mut lp::iguana_info);
unsafe impl Send for IISafe {}
unsafe impl Sync for IISafe {}

lazy_static! {
    /// NB: This singleton is here to support the C code. When the corresponding C code is ported,
    ///     the `COINS` should be removed in favor of `CoinsContext::coins`.
    pub static ref COINS: Mutex<HashMap<String, IISafe>> = Mutex::new (HashMap::new());
}

#[no_mangle]
pub extern fn LP_coinsearch (ticker: *const c_char) -> *mut lp::iguana_info {
    let ticker = unwrap! (unsafe {CStr::from_ptr (ticker)} .to_str());
    let coins = unwrap! (COINS.lock());
    match coins.get (ticker) {
        Some (ii) => ii.0,
        None => null_mut()
    }
}


#[no_mangle]
pub extern fn LP_coinadd (ii: *mut lp::iguana_info) -> *mut lp::iguana_info {
    let ticker = unwrap! (unsafe {CStr::from_ptr ((*ii).symbol.as_ptr())} .to_str());
    let mut coins = unwrap! (COINS.lock());
    coins.insert (ticker.into(), IISafe (ii));
    // As of now we still need the `LP_coins` in order to iterate over the coins,
    // but the improvement is that we're not moving the instance anywhere, it's effectively pinned.
    unsafe {lp::LP_coinadd_ (ii, size_of::<lp::iguana_info>() as i32)};
    ii
}

#[no_mangle]
pub extern fn LP_get_coin_pointers (coins_buf: *mut *mut lp::iguana_info, coins_size: i32) {
    let coins = unwrap! (COINS.lock());
    assert! (coins_size > 0);
    // NB: Resulting buffer is either zero-terminated or full.
    if coins.len() < coins_size as usize {unsafe {*coins_buf.offset (coins.len() as isize) = null_mut()}}
    for ((_ticker, ii), idx) in coins.iter().zip (0..) {
        if idx >= coins_size as isize {break}
        unsafe {*coins_buf.offset (idx) = ii.0}
    }
}

#[no_mangle]
pub extern fn is_loopback_ip (ip: *mut c_char) -> u8 {
    let ip_str = match unsafe { CStr::from_ptr(ip).to_str() } {
        Ok(s) => s,
        Err(e) => {
            log!("Error creating CStr " [e]);
            return 0;
        }
    };

    let ip: IpAddr = match ip_str.parse() {
        Ok(ip) => ip,
        Err(e) => {
            log!("Error " [e] " parsing ip from str " (ip_str));
            return 0;
        }
    };

    ip.is_loopback() as u8
}

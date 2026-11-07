use std::ffi::{CStr, CString};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

// C-ABI declarations matching mahayana_app_host.h
unsafe extern "C" {
    fn telegram_app_host_create_test(app_data_dir: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn telegram_app_host_destroy(host: *mut std::ffi::c_void);
    fn telegram_app_host_dispatch(host: *mut std::ffi::c_void, request_json: *const std::ffi::c_char) -> *mut std::ffi::c_char;
    fn telegram_app_host_free_string(ptr: *mut std::ffi::c_char);
    fn telegram_bridge_register_event_callback(
        host: *mut std::ffi::c_void,
        callback: extern "C" fn(*const std::ffi::c_char, *const std::ffi::c_char, *mut std::ffi::c_void),
        user_context: *mut std::ffi::c_void,
    );
}

static EVENT_COUNTER: Mutex<usize> = Mutex::new(0);

extern "C" fn on_event(
    event_type: *const std::ffi::c_char,
    _payload: *const std::ffi::c_char,
    _user_context: *mut std::ffi::c_void,
) {
    let ev_str = unsafe { CStr::from_ptr(event_type).to_str().unwrap() };
    if ev_str == "newMessage" {
        let mut count = EVENT_COUNTER.lock().unwrap();
        *count += 1;
    }
}

fn main() {
    println!("================================================================================");
    println!(" NATIVE RUST CONCURRENCY STRESS: 100 OS Threads on telegram_app_host_dispatch");
    println!("================================================================================");

    let temp_dir = std::env::temp_dir().join(format!("fabushi_rust_stress_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);
    let c_dir = CString::new(temp_dir.to_str().unwrap()).unwrap();

    let host = unsafe { telegram_app_host_create_test(c_dir.as_ptr()) };
    assert!(!host.is_null(), "Failed to create test host");

    unsafe {
        telegram_bridge_register_event_callback(host, on_event, std::ptr::null_mut());
    }

    let thread_count = 100;
    let barrier = Arc::new(Barrier::new(thread_count));
    let host_ptr = host as usize; // Pass pointer safely across threads

    let results = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();

    for thread_idx in 0..thread_count {
        let b = Arc::clone(&barrier);
        let res_arc = Arc::clone(&results);

        let handle = thread::spawn(move || {
            let req = format!(
                r#"{{"command":"telegram.sendMessage","params":{{"chat_id":2020,"sender_id":88,"client_nonce":55555,"text":"Concurrent message {thread_idx}"}}}}"#
            );
            let c_req = CString::new(req).unwrap();

            // Wait for all 100 OS threads to reach the starting gate
            b.wait();

            let resp_ptr = unsafe {
                telegram_app_host_dispatch(host_ptr as *mut std::ffi::c_void, c_req.as_ptr())
            };
            assert!(!resp_ptr.is_null());

            let resp_str = unsafe {
                let s = CStr::from_ptr(resp_ptr).to_str().unwrap().to_string();
                telegram_app_host_free_string(resp_ptr);
                s
            };

            res_arc.lock().unwrap().push(resp_str);
        });

        handles.push(handle);
    }

    for h in handles {
        h.join().unwrap();
    }

    let responses = results.lock().unwrap().clone();
    assert_eq!(responses.len(), thread_count);

    let mut new_count = 0;
    let mut dup_count = 0;
    let mut message_ids = Vec::new();

    for r in &responses {
        let v: serde_json::Value = serde_json::from_str(r).unwrap();
        assert_eq!(v["ok"], true);
        let result_obj = &v["result"];
        let is_dup = result_obj["duplicate"].as_bool().unwrap_or(false);
        let msg_id = result_obj["id"].as_i64().unwrap();

        if is_dup {
            dup_count += 1;
        } else {
            new_count += 1;
        }
        message_ids.push(msg_id);
    }

    let unique_ids: std::collections::HashSet<i64> = message_ids.iter().cloned().collect();
    let events_emitted = *EVENT_COUNTER.lock().unwrap();

    println!("Results from 100 native OS threads:");
    println!("  - New message count: {new_count}");
    println!("  - Duplicate count: {dup_count}");
    println!("  - Unique message IDs: {} -> {:?}", unique_ids.len(), unique_ids);
    println!("  - 'newMessage' events emitted: {events_emitted}");

    // Query messages in chat 2020
    let query_req = CString::new(r#"{"command":"telegram.getMessages","params":{"chat_id":2020}}"#).unwrap();
    let query_ptr = unsafe {
        telegram_app_host_dispatch(host, query_req.as_ptr())
    };
    let query_str = unsafe {
        let s = CStr::from_ptr(query_ptr).to_str().unwrap().to_string();
        telegram_app_host_free_string(query_ptr);
        s
    };
    let qv: serde_json::Value = serde_json::from_str(&query_str).unwrap();
    let store_msg_count = qv["result"].as_array().map(|a| a.len()).unwrap_or(0);
    println!("  - Messages in local_store for chat 2020: {store_msg_count}");

    unsafe {
        telegram_app_host_destroy(host);
    }
    let _ = std::fs::remove_dir_all(&temp_dir);

    // Assertions
    let mut failed = false;
    if new_count != 1 {
        println!("FAIL: Expected exactly 1 new message, got {new_count}");
        failed = true;
    }
    if dup_count != 99 {
        println!("FAIL: Expected 99 duplicates, got {dup_count}");
        failed = true;
    }
    if unique_ids.len() != 1 {
        println!("FAIL: Expected exactly 1 unique message ID, got {}", unique_ids.len());
        failed = true;
    }
    if events_emitted != 1 {
        println!("FAIL: Expected exactly 1 newMessage event emitted, got {events_emitted}");
        failed = true;
    }
    if store_msg_count != 1 {
        println!("FAIL: Expected exactly 1 message in local_store, got {store_msg_count}");
        failed = true;
    }

    if failed {
        println!("\n>>> EMPIRICAL CHALLENGE FAILED: NATIVE CONCURRENCY RACE DETECTED! <<<");
        std::process::exit(1);
    } else {
        println!("\n✓ EMPIRICAL CHALLENGE PASSED: Exactly 1 message inserted, 1 emitted, 99 duplicates.");
    }
}

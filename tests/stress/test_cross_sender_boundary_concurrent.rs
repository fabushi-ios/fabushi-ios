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

fn dispatch_command(host_ptr: usize, req_str: &str) -> serde_json::Value {
    let c_req = CString::new(req_str).unwrap();
    let resp_ptr = unsafe {
        telegram_app_host_dispatch(host_ptr as *mut std::ffi::c_void, c_req.as_ptr())
    };
    assert!(!resp_ptr.is_null());
    let resp_str = unsafe {
        let s = CStr::from_ptr(resp_ptr).to_str().unwrap().to_string();
        telegram_app_host_free_string(resp_ptr);
        s
    };
    serde_json::from_str(&resp_str).expect("Valid JSON response")
}

fn main() {
    println!("================================================================================");
    println!(" EMPIRICAL STRESS: Cross-Sender Isolation & Boundary Nonces Concurrency");
    println!("================================================================================");

    let temp_dir = std::env::temp_dir().join(format!("fabushi_cross_sender_stress_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);
    let c_dir = CString::new(temp_dir.to_str().unwrap()).unwrap();

    let host = unsafe { telegram_app_host_create_test(c_dir.as_ptr()) };
    assert!(!host.is_null(), "Failed to create test host");

    unsafe {
        telegram_bridge_register_event_callback(host, on_event, std::ptr::null_mut());
    }

    let host_ptr = host as usize;

    // -------------------------------------------------------------------------
    // Phase 1: Cross-Sender Concurrent Collision (50 threads Sender 42, 50 threads Sender 999)
    // -------------------------------------------------------------------------
    println!("\n[PHASE 1] Cross-Sender Concurrent Collision (Same Nonce = 7777777)...");
    let initial_events = *EVENT_COUNTER.lock().unwrap();
    assert_eq!(initial_events, 0);

    let total_threads = 100;
    let barrier = Arc::new(Barrier::new(total_threads));
    let results = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();
    for thread_idx in 0..total_threads {
        let b = Arc::clone(&barrier);
        let res_arc = Arc::clone(&results);
        let sender_id = if thread_idx < 50 { 42u64 } else { 999u64 };

        let handle = thread::spawn(move || {
            let req = format!(
                r#"{{"command":"telegram.sendMessage","params":{{"chat_id":3030,"sender_id":{sender_id},"client_nonce":7777777,"text":"Cross-sender message from thread {thread_idx}"}}}}"#
            );
            b.wait();
            let v = dispatch_command(host_ptr, &req);
            res_arc.lock().unwrap().push((sender_id, v));
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().unwrap();
    }

    let phase1_results = results.lock().unwrap().clone();
    assert_eq!(phase1_results.len(), 100);

    let mut sender_42_ids = Vec::new();
    let mut sender_999_ids = Vec::new();
    let mut sender_42_new = 0;
    let mut sender_42_dup = 0;
    let mut sender_999_new = 0;
    let mut sender_999_dup = 0;

    for (s_id, resp) in &phase1_results {
        assert_eq!(resp["ok"], true);
        let result_obj = &resp["result"];
        let is_dup = result_obj["duplicate"].as_bool().unwrap_or(false);
        let msg_id = result_obj["id"].as_i64().unwrap();

        if *s_id == 42 {
            sender_42_ids.push(msg_id);
            if is_dup { sender_42_dup += 1; } else { sender_42_new += 1; }
        } else if *s_id == 999 {
            sender_999_ids.push(msg_id);
            if is_dup { sender_999_dup += 1; } else { sender_999_new += 1; }
        }
    }

    let unique_42: std::collections::HashSet<i64> = sender_42_ids.iter().cloned().collect();
    let unique_999: std::collections::HashSet<i64> = sender_999_ids.iter().cloned().collect();
    let id_42 = *unique_42.iter().next().unwrap();
    let id_999 = *unique_999.iter().next().unwrap();

    println!("  Sender 42: new={}, dup={}, unique_ids={:?}", sender_42_new, sender_42_dup, unique_42);
    println!("  Sender 999: new={}, dup={}, unique_ids={:?}", sender_999_new, sender_999_dup, unique_999);
    println!("  ID 42 = {id_42}, ID 999 = {id_999}");

    assert_eq!(sender_42_new, 1, "Sender 42 must have exactly 1 new message");
    assert_eq!(sender_42_dup, 49, "Sender 42 must have exactly 49 duplicates");
    assert_eq!(unique_42.len(), 1, "Sender 42 must have exactly 1 unique ID");

    assert_eq!(sender_999_new, 1, "Sender 999 must have exactly 1 new message");
    assert_eq!(sender_999_dup, 49, "Sender 999 must have exactly 49 duplicates");
    assert_eq!(unique_999.len(), 1, "Sender 999 must have exactly 1 unique ID");

    assert_ne!(id_42, id_999, "Sender 42 and Sender 999 must NOT share message ID!");

    let events_phase1 = *EVENT_COUNTER.lock().unwrap();
    println!("  'newMessage' events emitted: {events_phase1}");
    assert_eq!(events_phase1, 2, "Expected exactly 2 events emitted in Phase 1");

    let query_chat3030 = dispatch_command(host_ptr, r#"{"command":"telegram.getMessages","params":{"chat_id":3030}}"#);
    let store_count_3030 = query_chat3030["result"].as_array().map(|a| a.len()).unwrap_or(0);
    println!("  Messages in store for chat 3030: {store_count_3030}");
    assert_eq!(store_count_3030, 2, "Chat 3030 must contain exactly 2 messages");
    println!("✓ PHASE 1 PASSED: Perfect cross-sender isolation under concurrent collision!");

    // -------------------------------------------------------------------------
    // Phase 2: Boundary Nonces Concurrent Collision (50 threads nonce 0, 50 threads nonce u64::MAX)
    // -------------------------------------------------------------------------
    println!("\n[PHASE 2] Boundary Nonces Concurrency (50 threads nonce 0, 50 threads nonce u64::MAX)...");
    let u64_max_str = u64::MAX.to_string();
    let barrier2 = Arc::new(Barrier::new(total_threads));
    let results2 = Arc::new(Mutex::new(Vec::new()));
    let mut handles2 = Vec::new();

    for thread_idx in 0..total_threads {
        let b = Arc::clone(&barrier2);
        let res_arc = Arc::clone(&results2);
        let (nonce_str, is_zero) = if thread_idx < 50 { ("0".to_string(), true) } else { (u64_max_str.clone(), false) };

        let handle = thread::spawn(move || {
            let req = format!(
                r#"{{"command":"telegram.sendMessage","params":{{"chat_id":4040,"sender_id":1,"client_nonce":{nonce_str},"text":"Boundary message from thread {thread_idx}"}}}}"#
            );
            b.wait();
            let v = dispatch_command(host_ptr, &req);
            res_arc.lock().unwrap().push((is_zero, v));
        });
        handles2.push(handle);
    }

    for h in handles2 {
        h.join().unwrap();
    }

    let phase2_results = results2.lock().unwrap().clone();
    assert_eq!(phase2_results.len(), 100);

    let mut nonce0_ids = Vec::new();
    let mut nonce_max_ids = Vec::new();
    let mut nonce0_new = 0;
    let mut nonce0_dup = 0;
    let mut nonce_max_new = 0;
    let mut nonce_max_dup = 0;

    for (is_zero, resp) in &phase2_results {
        assert_eq!(resp["ok"], true);
        let result_obj = &resp["result"];
        let is_dup = result_obj["duplicate"].as_bool().unwrap_or(false);
        let msg_id = result_obj["id"].as_i64().unwrap();

        if *is_zero {
            nonce0_ids.push(msg_id);
            if is_dup { nonce0_dup += 1; } else { nonce0_new += 1; }
        } else {
            nonce_max_ids.push(msg_id);
            if is_dup { nonce_max_dup += 1; } else { nonce_max_new += 1; }
        }
    }

    let unique_0: std::collections::HashSet<i64> = nonce0_ids.iter().cloned().collect();
    let unique_max: std::collections::HashSet<i64> = nonce_max_ids.iter().cloned().collect();
    let id_0 = *unique_0.iter().next().unwrap();
    let id_max = *unique_max.iter().next().unwrap();

    println!("  Nonce 0: new={}, dup={}, unique_ids={:?}", nonce0_new, nonce0_dup, unique_0);
    println!("  Nonce u64::MAX: new={}, dup={}, unique_ids={:?}", nonce_max_new, nonce_max_dup, unique_max);
    println!("  ID 0 = {id_0}, ID MAX = {id_max}");

    assert_eq!(nonce0_new, 1, "Nonce 0 must have exactly 1 new message");
    assert_eq!(nonce0_dup, 49, "Nonce 0 must have exactly 49 duplicates");
    assert_eq!(unique_0.len(), 1, "Nonce 0 must have exactly 1 unique ID");

    assert_eq!(nonce_max_new, 1, "Nonce MAX must have exactly 1 new message");
    assert_eq!(nonce_max_dup, 49, "Nonce MAX must have exactly 49 duplicates");
    assert_eq!(unique_max.len(), 1, "Nonce MAX must have exactly 1 unique ID");

    assert_ne!(id_0, id_max, "Nonce 0 and Nonce MAX must NOT share message ID!");

    let events_phase2 = *EVENT_COUNTER.lock().unwrap();
    println!("  'newMessage' events total: {events_phase2}");
    assert_eq!(events_phase2, 4, "Expected exactly 4 events emitted total after Phase 2 (2 in Phase 1 + 2 in Phase 2)");

    let query_chat4040 = dispatch_command(host_ptr, r#"{"command":"telegram.getMessages","params":{"chat_id":4040}}"#);
    let store_count_4040 = query_chat4040["result"].as_array().map(|a| a.len()).unwrap_or(0);
    println!("  Messages in store for chat 4040: {store_count_4040}");
    assert_eq!(store_count_4040, 2, "Chat 4040 must contain exactly 2 messages");
    println!("✓ PHASE 2 PASSED: Boundary nonces 0 and u64::MAX correctly handled under concurrency!");

    // -------------------------------------------------------------------------
    // Phase 3: Wave 2 (All 100 requests hit AlreadyExists cache)
    // -------------------------------------------------------------------------
    println!("\n[PHASE 3] Wave 2 AlreadyExists Cache Verification (100 concurrent requests)...");
    let barrier3 = Arc::new(Barrier::new(total_threads));
    let results3 = Arc::new(Mutex::new(Vec::new()));
    let mut handles3 = Vec::new();

    for thread_idx in 0..total_threads {
        let b = Arc::clone(&barrier3);
        let res_arc = Arc::clone(&results3);
        let sender_id = if thread_idx < 50 { 42u64 } else { 999u64 };

        let handle = thread::spawn(move || {
            let req = format!(
                r#"{{"command":"telegram.sendMessage","params":{{"chat_id":3030,"sender_id":{sender_id},"client_nonce":7777777,"text":"Wave 2 duplicate from thread {thread_idx}"}}}}"#
            );
            b.wait();
            let v = dispatch_command(host_ptr, &req);
            res_arc.lock().unwrap().push((sender_id, v));
        });
        handles3.push(handle);
    }

    for h in handles3 {
        h.join().unwrap();
    }

    let phase3_results = results3.lock().unwrap().clone();
    assert_eq!(phase3_results.len(), 100);

    for (s_id, resp) in &phase3_results {
        assert_eq!(resp["ok"], true);
        let result_obj = &resp["result"];
        assert_eq!(result_obj["duplicate"].as_bool(), Some(true), "All wave 2 must be duplicates");
        let msg_id = result_obj["id"].as_i64().unwrap();
        if *s_id == 42 {
            assert_eq!(msg_id, id_42);
        } else {
            assert_eq!(msg_id, id_999);
        }
    }

    let events_phase3 = *EVENT_COUNTER.lock().unwrap();
    assert_eq!(events_phase3, 4, "No new events must be emitted during wave 2!");
    println!("✓ PHASE 3 PASSED: All 100 Wave 2 requests cleanly returned AlreadyExists duplicate responses!");

    // Teardown
    unsafe {
        telegram_app_host_destroy(host);
    }
    let _ = std::fs::remove_dir_all(&temp_dir);

    println!("\n================================================================================");
    println!(" ALL ADVERSARIAL STRESS CHALLENGES PASSED EMPIRICALLY WITH ZERO DEFECTS!");
    println!("================================================================================");
}

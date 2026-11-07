#!/usr/bin/env python3
"""
Adversarial Stress Test for Rust Host 24-Hour Nonce Deduplication
Testing:
1. 100 concurrent duplicate requests with same sender_id and client_nonce.
2. Event callback count: exactly 1 push event.
3. Duplicate responses: exactly 1 original + 99 duplicates with duplicate: true and identical message_id.
4. Different sender IDs with same nonce: independent messages.
5. Boundary nonces: client_nonce = 0 and client_nonce = u64::MAX.
"""

import ctypes
import json
import os
import shutil
import sys
import tempfile
import threading
import time

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
DYLIB_PATH = os.path.join(REPO_ROOT, "target/debug/libmahayana_app_host.dylib")

if not os.path.isfile(DYLIB_PATH):
    print(f"ERROR: dylib not found at {DYLIB_PATH}", file=sys.stderr)
    sys.exit(1)

ffi = ctypes.CDLL(DYLIB_PATH)

EVENT_CALLBACK_TYPE = ctypes.CFUNCTYPE(None, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p)

ffi.telegram_app_host_create_test.argtypes = [ctypes.c_char_p]
ffi.telegram_app_host_create_test.restype = ctypes.c_void_p

ffi.telegram_app_host_dispatch.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
ffi.telegram_app_host_dispatch.restype = ctypes.c_void_p

ffi.telegram_app_host_free_string.argtypes = [ctypes.c_void_p]
ffi.telegram_app_host_free_string.restype = None

ffi.telegram_app_host_destroy.argtypes = [ctypes.c_void_p]
ffi.telegram_app_host_destroy.restype = None

ffi.telegram_bridge_register_event_callback.argtypes = [
    ctypes.c_void_p, EVENT_CALLBACK_TYPE, ctypes.c_void_p
]
ffi.telegram_bridge_register_event_callback.restype = None

def dispatch_json(host, cmd_dict):
    req_bytes = json.dumps(cmd_dict).encode("utf-8")
    ptr = ffi.telegram_app_host_dispatch(host, req_bytes)
    if not ptr:
        return None
    try:
        raw_str = ctypes.cast(ptr, ctypes.c_char_p).value.decode("utf-8")
        return json.loads(raw_str)
    finally:
        ffi.telegram_app_host_free_string(ptr)

def run_nonce_dedup_stress():
    print("================================================================================")
    print(" ADVERSARIAL STRESS: Rust Host 24-Hour Nonce Deduplication & Concurrency")
    print("================================================================================")

    temp_dir = tempfile.mkdtemp(prefix="fabushi_dedup_stress_")
    host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
    assert host, "Failed to create test host"

    emitted_events = []
    events_lock = threading.Lock()

    def on_event(event_type_p, payload_p, ctx):
        ev_type = ctypes.cast(event_type_p, ctypes.c_char_p).value.decode("utf-8")
        payload = ctypes.cast(payload_p, ctypes.c_char_p).value.decode("utf-8")
        with events_lock:
            emitted_events.append((ev_type, payload))

    cb_func = EVENT_CALLBACK_TYPE(on_event)
    ffi.telegram_bridge_register_event_callback(host, cb_func, None)

    try:
        # ----------------------------------------------------------------------
        # Baseline: Sequential Duplicate Requests (Sanity Check)
        # ----------------------------------------------------------------------
        print("\n[BASELINE] Sequential Duplicate Requests...")
        base_req = {
            "command": "telegram.sendMessage",
            "params": {"chat_id": 9999, "sender_id": 1, "client_nonce": 11111, "text": "Base msg 1"}
        }
        res_seq_1 = dispatch_json(host, base_req)
        print(f"  res_seq_1: {res_seq_1}")
        assert res_seq_1 and res_seq_1.get("ok"), "Sequential send 1 failed"
        assert not res_seq_1["result"].get("duplicate", False), "Sequential send 1 must not be duplicate"
        seq_msg_id = res_seq_1["result"]["id"]

        res_seq_2 = dispatch_json(host, base_req)
        print(f"  res_seq_2: {res_seq_2}")
        assert res_seq_2 and res_seq_2.get("ok"), "Sequential send 2 failed"
        assert res_seq_2["result"].get("duplicate", False), "Sequential send 2 must be duplicate"
        assert res_seq_2["result"]["id"] == seq_msg_id, "Sequential send 2 must match message id"
        print(f"✓ Baseline PASSED: Sequential duplicates correctly deduplicated (id: {seq_msg_id}).")

        # ----------------------------------------------------------------------
        # Test 1: 100 Concurrent Duplicate Requests
        # ----------------------------------------------------------------------
        print("\n[CHALLENGE 1] 100 Concurrent duplicate requests with same sender_id & client_nonce...")
        with events_lock:
            emitted_events.clear()

        sender_id = 42
        client_nonce = 987654321
        chat_id = 1001
        thread_count = 100

        barrier = threading.Barrier(thread_count)
        responses = [None] * thread_count

        def worker(idx):
            req = {
                "command": "telegram.sendMessage",
                "params": {
                    "chat_id": chat_id,
                    "sender_id": sender_id,
                    "client_nonce": client_nonce,
                    "text": f"Concurrent duplicate message from thread {idx}"
                }
            }
            # Wait for all threads to align
            barrier.wait()
            res = dispatch_json(host, req)
            responses[idx] = res

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(thread_count)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        # Analyze responses
        print(f"  All {thread_count} threads completed.")
        ok_responses = [r for r in responses if r and r.get("ok")]
        assert len(ok_responses) == thread_count, f"Expected {thread_count} ok responses, got {len(ok_responses)}"

        duplicate_flags = [r.get("result", {}).get("duplicate", False) for r in ok_responses]
        message_ids = [r.get("result", {}).get("id") for r in ok_responses]
        unique_message_ids = set(message_ids)

        new_count = duplicate_flags.count(False)
        dup_count = duplicate_flags.count(True)

        print(f"  - Non-duplicate (new) count: {new_count}")
        print(f"  - Duplicate count: {dup_count}")
        print(f"  - Unique message IDs returned: {len(unique_message_ids)} -> {unique_message_ids}")

        with events_lock:
            new_msg_events = [ev for ev in emitted_events if ev[0] == "newMessage"]
            print(f"  - 'newMessage' push events emitted: {len(new_msg_events)}")

        # Check local_store messages
        history_res = dispatch_json(host, {
            "command": "telegram.getMessages",
            "params": {"chat_id": chat_id}
        })
        messages_in_store = history_res.get("result", []) if history_res and history_res.get("ok") else []
        print(f"  - Messages in local_store for chat {chat_id}: {len(messages_in_store)}")

        # Assertions
        success = True
        if new_count != 1:
            print(f"FAIL: Expected exactly 1 new message response, got {new_count}")
            success = False
        if dup_count != 99:
            print(f"FAIL: Expected 99 duplicate responses, got {dup_count}")
            success = False
        if len(unique_message_ids) != 1:
            print(f"FAIL: Expected all 100 responses to have the same message_id, got {len(unique_message_ids)} distinct IDs")
            success = False
        if len(new_msg_events) != 1:
            print(f"FAIL: Expected exactly 1 'newMessage' push event, got {len(new_msg_events)}")
            success = False
        if len(messages_in_store) != 1:
            print(f"FAIL: Expected exactly 1 message in local_store, got {len(messages_in_store)}")
            success = False

        if not success:
            print("\n>>> CHALLENGE 1 FAILED! CONCURRENCY RACE CONDITION DETECTED! <<<")
            return False
        else:
            print("✓ CHALLENGE 1 PASSED: Exactly 1 inserted, exactly 1 emitted, 99 duplicates identical.")

        # ----------------------------------------------------------------------
        # Test 2: Different sender IDs with same nonce
        # ----------------------------------------------------------------------
        print("\n[CHALLENGE 2] Different sender IDs with same nonce (isolation)...")
        res_sender_b = dispatch_json(host, {
            "command": "telegram.sendMessage",
            "params": {
                "chat_id": chat_id,
                "sender_id": 999, # different sender
                "client_nonce": client_nonce, # same nonce
                "text": "Different sender message"
            }
        })
        assert res_sender_b and res_sender_b.get("ok")
        assert not res_sender_b["result"].get("duplicate", False), "Different sender must NOT be flagged as duplicate"
        assert res_sender_b["result"]["id"] not in unique_message_ids, "Different sender must receive a new message ID"
        print(f"✓ CHALLENGE 2 PASSED: Sender 999 admitted independently with message_id {res_sender_b['result']['id']}.")

        # ----------------------------------------------------------------------
        # Test 3: Boundary Nonces: 0 and u64::MAX
        # ----------------------------------------------------------------------
        print("\n[CHALLENGE 3] Boundary nonces: 0 and u64::MAX...")
        # Boundary 0
        res_0_1 = dispatch_json(host, {
            "command": "telegram.sendMessage",
            "params": {"chat_id": chat_id, "sender_id": 1, "client_nonce": 0, "text": "Nonce 0 First"}
        })
        assert res_0_1 and res_0_1.get("ok")
        assert not res_0_1["result"].get("duplicate", False)
        id_0 = res_0_1["result"]["id"]

        res_0_2 = dispatch_json(host, {
            "command": "telegram.sendMessage",
            "params": {"chat_id": chat_id, "sender_id": 1, "client_nonce": 0, "text": "Nonce 0 Second"}
        })
        assert res_0_2 and res_0_2.get("ok")
        assert res_0_2["result"].get("duplicate", False)
        assert res_0_2["result"]["id"] == id_0, "Duplicate nonce 0 must return original id"
        print("  - Nonce 0 deduplication verified.")

        # Boundary u64::MAX (18446744073709551615)
        u64_max = 18446744073709551615
        res_max_1 = dispatch_json(host, {
            "command": "telegram.sendMessage",
            "params": {"chat_id": chat_id, "sender_id": 1, "client_nonce": u64_max, "text": "Nonce MAX First"}
        })
        assert res_max_1 and res_max_1.get("ok")
        assert not res_max_1["result"].get("duplicate", False)
        id_max = res_max_1["result"]["id"]

        res_max_2 = dispatch_json(host, {
            "command": "telegram.sendMessage",
            "params": {"chat_id": chat_id, "sender_id": 1, "client_nonce": u64_max, "text": "Nonce MAX Second"}
        })
        assert res_max_2 and res_max_2.get("ok")
        assert res_max_2["result"].get("duplicate", False)
        assert res_max_2["result"]["id"] == id_max, "Duplicate nonce u64::MAX must return original id"
        print("  - Nonce u64::MAX deduplication verified.")
        print("✓ CHALLENGE 3 PASSED: Boundary nonces 0 and u64::MAX handled accurately.")

        return True

    finally:
        ffi.telegram_app_host_destroy(host)
        shutil.rmtree(temp_dir, ignore_errors=True)

if __name__ == "__main__":
    success = run_nonce_dedup_stress()
    sys.exit(0 if success else 1)

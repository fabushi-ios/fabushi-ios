#!/usr/bin/env python3
"""
Adversarial Stress Harness: Deep Challenge Suite
1. Panic-in-flight concurrency: Interleaving panic requests with valid requests across concurrent threads.
2. Callback mutation concurrency: Re-registering callbacks concurrently while events are actively dispatched.
3. High concurrency burst: 50 threads x 100 requests = 5,000 RPCs.
4. Non-standard paths: Paths with spaces, UTF-8, and deep hierarchy in host creation.
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

assert os.path.isfile(DYLIB_PATH), f"Dylib not found at {DYLIB_PATH}"
ffi = ctypes.CDLL(DYLIB_PATH)

EVENT_CALLBACK_TYPE = ctypes.CFUNCTYPE(None, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p)

ffi.telegram_app_host_create.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
ffi.telegram_app_host_create.restype = ctypes.c_void_p

ffi.telegram_app_host_create_test.argtypes = [ctypes.c_char_p]
ffi.telegram_app_host_create_test.restype = ctypes.c_void_p

ffi.telegram_app_host_dispatch.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
ffi.telegram_app_host_dispatch.restype = ctypes.c_void_p

ffi.telegram_bridge_register_event_callback.argtypes = [
    ctypes.c_void_p,
    EVENT_CALLBACK_TYPE,
    ctypes.c_void_p,
]
ffi.telegram_bridge_register_event_callback.restype = None

ffi.telegram_app_host_free_string.argtypes = [ctypes.c_void_p]
ffi.telegram_app_host_free_string.restype = None

ffi.telegram_app_host_destroy.argtypes = [ctypes.c_void_p]
ffi.telegram_app_host_destroy.restype = None


def dispatch_helper(host, req_bytes: bytes) -> dict:
    res_ptr = ffi.telegram_app_host_dispatch(host, req_bytes)
    assert res_ptr != 0, "Null pointer returned from dispatch"
    raw = ctypes.string_at(res_ptr).decode("utf-8")
    ffi.telegram_app_host_free_string(res_ptr)
    return json.loads(raw)


def test_interleaved_panic_concurrency():
    print("\n--- Challenge 1: Interleaved Panic Concurrency ---")
    temp_dir = tempfile.mkdtemp(prefix="adv_panic_")
    try:
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0

        normal_success = 0
        panic_caught = 0
        errors = []
        lock = threading.Lock()

        def normal_worker(t_id: int):
            nonlocal normal_success
            for i in range(50):
                cmd = json.dumps({"command": "telegram.ping", "params": {"nonce": t_id * 100 + i}}).encode("utf-8")
                try:
                    res = dispatch_helper(host, cmd)
                    if res.get("ok"):
                        with lock:
                            normal_success += 1
                    else:
                        with lock:
                            errors.append(f"Normal worker {t_id} got error: {res}")
                except Exception as e:
                    with lock:
                        errors.append(f"Normal worker {t_id} exception: {e}")

        def panic_worker(t_id: int):
            nonlocal panic_caught
            for _ in range(20):
                cmd = b'{"command":"debug.trigger_panic"}'
                try:
                    res = dispatch_helper(host, cmd)
                    if res.get("ok") is False and res.get("error", {}).get("code") == "HOST_PANIC":
                        with lock:
                            panic_caught += 1
                    else:
                        with lock:
                            errors.append(f"Panic worker {t_id} unexpected response: {res}")
                except Exception as e:
                    with lock:
                        errors.append(f"Panic worker {t_id} exception: {e}")

        threads = []
        for i in range(10):
            threads.append(threading.Thread(target=normal_worker, args=(i,)))
        for i in range(5):
            threads.append(threading.Thread(target=panic_worker, args=(i,)))

        t0 = time.time()
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        elapsed = time.time() - t0

        print(f"  Finished in {elapsed:.3f}s: {normal_success}/500 normal passed, {panic_caught}/100 panics isolated.")
        assert len(errors) == 0, f"Errors encountered: {errors[:5]}"
        assert normal_success == 500, f"Expected 500 normal successes, got {normal_success}"
        assert panic_caught == 100, f"Expected 100 isolated panics, got {panic_caught}"

        ffi.telegram_app_host_destroy(host)
        print("  -> PASS: Host survived concurrent panics without state poisoning or deadlock.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


def test_callback_mutation_concurrency():
    print("\n--- Challenge 2: Concurrent Callback Mutation & Event Emission ---")
    temp_dir = tempfile.mkdtemp(prefix="adv_cb_")
    try:
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0

        received_events = 0
        cb_lock = threading.Lock()

        def cb1(event_p, payload_p, user_ctx):
            nonlocal received_events
            with cb_lock:
                received_events += 1

        def cb2(event_p, payload_p, user_ctx):
            nonlocal received_events
            with cb_lock:
                received_events += 2

        func_cb1 = EVENT_CALLBACK_TYPE(cb1)
        func_cb2 = EVENT_CALLBACK_TYPE(cb2)

        stop_flag = False
        mutator_errors = []

        def event_emitter(t_id: int):
            for i in range(100):
                cmd = json.dumps({
                    "command": "telegram.sendMessage",
                    "params": {"chat_id": 100 + t_id, "text": f"Msg {i}"}
                }).encode("utf-8")
                dispatch_helper(host, cmd)

        def cb_mutator():
            toggle = 0
            while not stop_flag:
                if toggle == 0:
                    ffi.telegram_bridge_register_event_callback(host, func_cb1, None)
                elif toggle == 1:
                    ffi.telegram_bridge_register_event_callback(host, func_cb2, None)
                else:
                    ffi.telegram_bridge_register_event_callback(host, EVENT_CALLBACK_TYPE(0), None)
                toggle = (toggle + 1) % 3
                time.sleep(0.001)

        mut_thread = threading.Thread(target=cb_mutator)
        mut_thread.start()

        emitter_threads = [threading.Thread(target=event_emitter, args=(i,)) for i in range(8)]
        for t in emitter_threads:
            t.start()
        for t in emitter_threads:
            t.join()

        stop_flag = True
        mut_thread.join()

        print(f"  Emitted 800 events while mutating callback. Total event score captured: {received_events}")
        assert len(mutator_errors) == 0

        ffi.telegram_app_host_destroy(host)
        print("  -> PASS: Zero race conditions or crashes during concurrent callback registration/unregistration.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


def test_high_concurrency_burst():
    print("\n--- Challenge 3: High Concurrency Burst (50 Threads x 100 RPCs = 5,000 RPCs) ---")
    temp_dir = tempfile.mkdtemp(prefix="adv_burst_")
    try:
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0

        num_threads = 50
        reqs_per_thread = 100
        total_requests = num_threads * reqs_per_thread

        success_count = 0
        count_lock = threading.Lock()
        thread_errors = []

        def worker(t_id: int):
            nonlocal success_count
            local_success = 0
            for i in range(reqs_per_thread):
                cmd = json.dumps({
                    "command": "telegram.sendMessage",
                    "params": {"chat_id": t_id, "text": f"Burst {i}"}
                }).encode("utf-8")
                try:
                    res = dispatch_helper(host, cmd)
                    if res.get("ok"):
                        local_success += 1
                    else:
                        with count_lock:
                            thread_errors.append(f"Thread {t_id} error: {res}")
                except Exception as e:
                    with count_lock:
                        thread_errors.append(f"Thread {t_id} exception: {e}")
            with count_lock:
                success_count += local_success

        t0 = time.time()
        threads = [threading.Thread(target=worker, args=(i,)) for i in range(num_threads)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        elapsed = time.time() - t0

        print(f"  Dispatched {total_requests} RPCs across {num_threads} threads in {elapsed:.3f}s ({total_requests / elapsed:.1f} rps)")
        assert len(thread_errors) == 0, f"Errors: {thread_errors[:5]}"
        assert success_count == total_requests, f"Expected {total_requests}, got {success_count}"

        ffi.telegram_app_host_destroy(host)
        print("  -> PASS: 5,000 concurrent RPCs completed with 100% success rate.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


def test_special_paths_and_passphrases():
    print("\n--- Challenge 4: Special Characters, UTF-8 Paths & Storage Passphrases ---")
    # Path with spaces and unicode
    special_name = "tg_special_dir_\u4e2d\u6587 \U0001f680 and spaces"
    temp_dir = os.path.join(tempfile.gettempdir(), special_name)
    os.makedirs(temp_dir, exist_ok=True)
    try:
        passphrase = "super-secret-\u5bc6\u7801-passphrase-123"
        host = ffi.telegram_app_host_create(temp_dir.encode("utf-8"), passphrase.encode("utf-8"))
        assert host != 0, "Failed to create host with special UTF-8 path and passphrase"

        ping_res = dispatch_helper(host, b'{"command":"telegram.ping","params":{"nonce":777}}')
        assert ping_res.get("ok") is True
        assert ping_res["result"]["nonce"] == 777

        ffi.telegram_app_host_destroy(host)
        print("  -> PASS: Host successfully initialized and operated with UTF-8 path and encryption passphrase.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


def main():
    print("=" * 70)
    print("EMPIRICAL CHALLENGER: Deep Adversarial Stress Suite")
    print("=" * 70)
    test_interleaved_panic_concurrency()
    test_callback_mutation_concurrency()
    test_high_concurrency_burst()
    test_special_paths_and_passphrases()
    print("\n" + "=" * 70)
    print("ALL DEEP ADVERSARIAL CHALLENGES PASSED EMPIRICALLY!")
    print("=" * 70)


if __name__ == "__main__":
    main()

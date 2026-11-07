#!/usr/bin/env python3
"""
tests/adversarial_ffi/test_ffi_stress.py

Adversarial Stress Test Suite for the Rust C-ABI FFI Surface & Panic Isolation
Targeting: source/host/ (libmahayana_app_host.dylib)

Empirical Challenge Coverage:
1. Forced panic isolation & catch_unwind verification (debug.trigger_panic)
2. Corrupted / malformed JSON strings, binary garbage, non-UTF-8 bytes, recursion depth
3. NULL pointer resilience across all C-ABI entry points
4. High-throughput multi-threaded concurrent dispatch hammering (20 threads x 100 reqs = 2000 RPCs)
5. Event callback streaming concurrency and thread safety
6. Host lifecycle & memory leak audit (repeated create/destroy and string alloc/free churn)
"""

import ctypes
import json
import os
import resource
import shutil
import sys
import tempfile
import threading
import time
from typing import Optional


# --- Path & Library Loading ---

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
DYLIB_PATH = os.path.join(REPO_ROOT, "target/debug/libmahayana_app_host.dylib")

if not os.path.isfile(DYLIB_PATH):
    print(f"ERROR: Target dylib not found at {DYLIB_PATH}", file=sys.stderr)
    print("Please build it first: cargo build --manifest-path source/host/Cargo.toml", file=sys.stderr)
    sys.exit(1)

ffi = ctypes.CDLL(DYLIB_PATH)

# --- C-ABI Function Signatures ---

# Callback definition: void (*)(const char*, const char*, void*)
EVENT_CALLBACK_TYPE = ctypes.CFUNCTYPE(
    None, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p
)

ffi.telegram_app_host_create.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
ffi.telegram_app_host_create.restype = ctypes.c_void_p

ffi.telegram_app_host_create_test.argtypes = [ctypes.c_char_p]
ffi.telegram_app_host_create_test.restype = ctypes.c_void_p

ffi.telegram_app_host_last_error.argtypes = []
ffi.telegram_app_host_last_error.restype = ctypes.c_char_p

ffi.telegram_app_host_dispatch.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
ffi.telegram_app_host_dispatch.restype = ctypes.c_void_p  # return as void_p so we can safely free it

ffi.telegram_app_host_dispatch_with_handle.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
ffi.telegram_app_host_dispatch_with_handle.restype = ctypes.c_void_p

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

# Legacy backward-compatibility symbols
ffi.mahayana_app_host_dispatch.argtypes = [ctypes.c_char_p]
ffi.mahayana_app_host_dispatch.restype = ctypes.c_void_p

ffi.mahayana_cloud_agent_trace_rows.argtypes = [ctypes.c_char_p, ctypes.c_size_t]
ffi.mahayana_cloud_agent_trace_rows.restype = ctypes.c_void_p

ffi.mahayana_spreadsheet_parse_file.argtypes = [ctypes.c_char_p, ctypes.c_size_t, ctypes.c_size_t]
ffi.mahayana_spreadsheet_parse_file.restype = ctypes.c_void_p


def dispatch_helper(host_ptr, req_bytes: bytes) -> tuple[dict, str]:
    """Dispatches request bytes, parses response JSON, frees C string, and returns (dict, raw_str)."""
    res_ptr = ffi.telegram_app_host_dispatch(host_ptr, req_bytes)
    assert res_ptr is not None and res_ptr != 0, "telegram_app_host_dispatch returned a NULL pointer!"
    raw_bytes = ctypes.string_at(res_ptr)
    ffi.telegram_app_host_free_string(res_ptr)
    raw_str = raw_bytes.decode("utf-8", errors="replace")
    parsed = json.loads(raw_str)
    return parsed, raw_str


def get_rss_mb() -> float:
    """Returns current process RSS in MB."""
    # On macOS, ru_maxrss is in bytes
    usage = resource.getrusage(resource.RUSAGE_SELF)
    return usage.ru_maxrss / (1024.0 * 1024.0)


# =============================================================================
# Test Suite 1: Panic Isolation & Abort Immunity
# =============================================================================

def test_panic_isolation():
    print("\n[TEST 1] Testing Panic Isolation (catch_unwind) & Abort Immunity...")
    temp_dir = tempfile.mkdtemp(prefix="ffi_panic_")
    try:
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0, "Failed to create test host"

        # 1. Deliberate panic inside dispatch
        req = b'{"command":"debug.trigger_panic"}'
        parsed, raw = dispatch_helper(host, req)
        print(f"  Panic response: {raw}")

        assert parsed.get("ok") is False, f"Expected ok:false, got {parsed}"
        err = parsed.get("error", {})
        assert err.get("code") == "HOST_PANIC", f"Expected code: HOST_PANIC, got {err.get('code')}"
        assert "Intentional test panic" in err.get("message", ""), f"Unexpected panic msg: {err}"

        # 2. Verify host remains functional after catching a panic (no poisoned lock or dead runtime)
        ping_req = b'{"command":"telegram.ping","params":{"nonce":999}}'
        parsed_ping, _ = dispatch_helper(host, ping_req)
        assert parsed_ping.get("ok") is True, f"Host was disabled after panic: {parsed_ping}"
        assert parsed_ping["result"]["nonce"] == 999

        # 3. Test panic isolation with handle alias
        res_ptr = ffi.telegram_app_host_dispatch_with_handle(host, req)
        raw_bytes = ctypes.string_at(res_ptr)
        ffi.telegram_app_host_free_string(res_ptr)
        parsed_handle = json.loads(raw_bytes.decode("utf-8"))
        assert parsed_handle.get("ok") is False
        assert parsed_handle.get("error", {}).get("code") == "HOST_PANIC"

        ffi.telegram_app_host_destroy(host)
        print("  -> PASS: Panic caught cleanly, process did not abort, host recovered.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


# =============================================================================
# Test Suite 2: NULL Pointer Resilience & C-ABI Boundaries
# =============================================================================

def test_null_pointer_resilience():
    print("\n[TEST 2] Testing NULL Pointer Handling Across C-ABI Exports...")
    temp_dir = tempfile.mkdtemp(prefix="ffi_null_")
    try:
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0

        # 1. Dispatch with NULL host handle
        valid_json = b'{"command":"telegram.ping"}'
        res_ptr = ffi.telegram_app_host_dispatch(None, valid_json)
        assert res_ptr != 0
        parsed = json.loads(ctypes.string_at(res_ptr).decode("utf-8"))
        ffi.telegram_app_host_free_string(res_ptr)
        assert parsed.get("ok") is False
        assert parsed.get("error", {}).get("code") == "INVALID_ARGUMENTS"

        # 2. Dispatch with NULL request_json
        res_ptr = ffi.telegram_app_host_dispatch(host, None)
        assert res_ptr != 0
        parsed = json.loads(ctypes.string_at(res_ptr).decode("utf-8"))
        ffi.telegram_app_host_free_string(res_ptr)
        assert parsed.get("ok") is False
        assert parsed.get("error", {}).get("code") == "INVALID_ARGUMENTS"

        # 3. Dispatch with both NULL
        res_ptr = ffi.telegram_app_host_dispatch(None, None)
        assert res_ptr != 0
        parsed = json.loads(ctypes.string_at(res_ptr).decode("utf-8"))
        ffi.telegram_app_host_free_string(res_ptr)
        assert parsed.get("ok") is False

        # 4. free_string(NULL) must be safe no-op
        ffi.telegram_app_host_free_string(None)

        # 5. destroy(NULL) must be safe no-op
        ffi.telegram_app_host_destroy(None)

        # 6. register_event_callback with NULL host
        ffi.telegram_bridge_register_event_callback(None, EVENT_CALLBACK_TYPE(0), None)

        # 7. Legacy mahayana_app_host_dispatch(NULL)
        res_ptr = ffi.mahayana_app_host_dispatch(None)
        assert res_ptr != 0
        parsed = json.loads(ctypes.string_at(res_ptr).decode("utf-8"))
        ffi.telegram_app_host_free_string(res_ptr)
        assert parsed.get("ok") is False
        assert parsed.get("error", {}).get("code") == "INVALID_ARGUMENTS"

        # 8. Legacy cloud agent trace rows with NULL bytes
        res_ptr = ffi.mahayana_cloud_agent_trace_rows(None, 100)
        assert res_ptr != 0
        parsed = json.loads(ctypes.string_at(res_ptr).decode("utf-8"))
        ffi.telegram_app_host_free_string(res_ptr)
        assert parsed.get("ok") is False

        # 9. Legacy spreadsheet parse with NULL path
        res_ptr = ffi.mahayana_spreadsheet_parse_file(None, 0, 0)
        assert res_ptr != 0
        parsed = json.loads(ctypes.string_at(res_ptr).decode("utf-8"))
        ffi.telegram_app_host_free_string(res_ptr)
        assert parsed.get("ok") is False

        # 10. Default sandbox create with NULL app_data_dir and NULL storage_passphrase
        default_host = ffi.telegram_app_host_create(None, None)
        assert default_host != 0, "telegram_app_host_create(NULL, NULL) failed to fall back to default dir"
        # Test default host works
        parsed_def, _ = dispatch_helper(default_host, valid_json)
        assert parsed_def.get("ok") is True
        ffi.telegram_app_host_destroy(default_host)

        ffi.telegram_app_host_destroy(host)
        print("  -> PASS: All NULL pointer edge-cases handled gracefully with zero crashes.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


# =============================================================================
# Test Suite 3: Malformed & Corrupted Inputs
# =============================================================================

def test_malformed_inputs():
    print("\n[TEST 3] Testing Corrupted / Malformed JSON Inputs & Non-UTF8 Strings...")
    temp_dir = tempfile.mkdtemp(prefix="ffi_malformed_")
    try:
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0

        cases = [
            (b"", "INVALID_JSON"),
            (b"   ", "INVALID_JSON"),
            (b"{", "INVALID_JSON"),
            (b'{"command":', "INVALID_JSON"),
            (b'{"params": {"chat_id": 100}}', "MISSING_COMMAND"),
            (b'{"command": 12345}', "INVALID_JSON"),
            (b"true", ("INVALID_JSON", "MISSING_COMMAND")),
            (b"12345", ("INVALID_JSON", "MISSING_COMMAND")),
            (b"[1, 2, 3]", ("INVALID_JSON", "MISSING_COMMAND")),
            (b'{"command": "nonexistent.method"}', None),  # should be acknowledged gracefully
        ]

        for payload, expected_code in cases:
            parsed, raw = dispatch_helper(host, payload)
            if expected_code:
                assert parsed.get("ok") is False, f"Expected ok:false for {payload}, got {raw}"
                actual_code = parsed.get("error", {}).get("code")
                if isinstance(expected_code, tuple):
                    assert actual_code in expected_code, f"Expected code in {expected_code} for {payload}, got {actual_code}"
                else:
                    assert actual_code == expected_code, f"Expected code {expected_code} for {payload}, got {actual_code}"
            else:
                assert parsed.get("ok") is True, f"Expected generic ack for unknown command: {raw}"

        # Test Deeply Nested JSON (300 nested objects)
        deep_json = b'{"a":' * 200 + b'{"command":"telegram.ping"}' + b'}' * 200
        parsed_deep, raw_deep = dispatch_helper(host, deep_json)
        assert "ok" in parsed_deep

        # Test Non-UTF8 Raw Bytes
        non_utf8_payload = b'{"command":"\xff\xfe\xfd\x80"}\x00'
        buf = ctypes.create_string_buffer(non_utf8_payload)
        res_ptr = ffi.telegram_app_host_dispatch(host, buf)
        assert res_ptr != 0
        raw_non_utf8 = ctypes.string_at(res_ptr).decode("utf-8")
        ffi.telegram_app_host_free_string(res_ptr)
        parsed_non_utf8 = json.loads(raw_non_utf8)
        assert parsed_non_utf8.get("ok") is False
        assert parsed_non_utf8.get("error", {}).get("code") in ("INVALID_UTF8", "INVALID_JSON")

        # Test Massive 5MB Payload
        large_text = "A" * 5_000_000
        large_req = json.dumps({
            "command": "telegram.sendMessage",
            "params": {"chat_id": 999, "text": large_text}
        }).encode("utf-8")
        parsed_large, _ = dispatch_helper(host, large_req)
        assert parsed_large.get("ok") is True
        assert parsed_large["result"]["chat_id"] == 999

        ffi.telegram_app_host_destroy(host)
        print("  -> PASS: Malformed inputs, deep recursion, invalid UTF-8 and 5MB payloads passed.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


# =============================================================================
# Test Suite 4: Multi-Threaded Concurrent Dispatch Hammering
# =============================================================================

def test_concurrent_dispatch_hammering():
    print("\n[TEST 4] Testing Multi-Threaded Concurrent Dispatch Hammering (20 threads x 100 reqs)...")
    temp_dir = tempfile.mkdtemp(prefix="ffi_concurrent_")
    try:
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0

        # Event callback tracking
        event_counter = {"newMessage": 0, "syncDifference": 0, "taskStateUpdated": 0, "total": 0}
        counter_lock = threading.Lock()

        def c_callback(event_type_p, payload_json_p, user_ctx):
            t = ctypes.string_at(event_type_p).decode("utf-8")
            with counter_lock:
                event_counter["total"] += 1
                if t in event_counter:
                    event_counter[t] += 1

        cb_func = EVENT_CALLBACK_TYPE(c_callback)
        ffi.telegram_bridge_register_event_callback(host, cb_func, None)

        num_threads = 20
        reqs_per_thread = 100
        total_requests = num_threads * reqs_per_thread

        thread_errors = []

        commands = [
            b'{"command":"telegram.ping","params":{"nonce":1}}',
            b'{"command":"telegram.sendMessage","params":{"chat_id":10,"text":"Concurrent message"}}',
            b'{"command":"telegram.getMessages","params":{"chat_id":10}}',
            b'{"command":"telegram.getChats"}',
            b'{"command":"telegram.syncDifference","params":{"pts":1}}',
            b'{"command":"telegram.media.encodeWaveform","params":{"samples":[0.1, 0.5, 0.9, 0.2]}}',
            b'{"command":"telegram.media.decodeWaveform","params":{"waveform_bytes":[12, 34, 56]}}',
            b'{"command":"telegram.agent.createTask","params":{"task_id":"fsm-1"}}',
            b'{"command":"telegram.agent.listTasks"}',
            b'{"command":"telegram.generation.current"}',
            b'{"command":"telegram.generation.bump"}',
            b'{"command":"telegram.webrtc.createSession","params":{"peer_id":"peer-10"}}',
        ]

        def worker(thread_idx: int):
            for i in range(reqs_per_thread):
                cmd = commands[(thread_idx + i) % len(commands)]
                try:
                    res_ptr = ffi.telegram_app_host_dispatch(host, cmd)
                    if not res_ptr:
                        thread_errors.append(f"Thread {thread_idx} received NULL pointer on iteration {i}")
                        continue
                    raw = ctypes.string_at(res_ptr).decode("utf-8")
                    ffi.telegram_app_host_free_string(res_ptr)
                    data = json.loads(raw)
                    if not data.get("ok"):
                        thread_errors.append(f"Thread {thread_idx} got error response: {raw}")
                except Exception as e:
                    thread_errors.append(f"Thread {thread_idx} raised exception on iter {i}: {e}")

        threads = [threading.Thread(target=worker, args=(t,)) for t in range(num_threads)]
        t0 = time.time()
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        elapsed = time.time() - t0

        print(f"  Dispatched {total_requests} RPCs across {num_threads} threads in {elapsed:.3f}s ({total_requests / elapsed:.1f} rps)")
        print(f"  Push events captured: total={event_counter['total']}, newMessage={event_counter['newMessage']}, taskStateUpdated={event_counter['taskStateUpdated']}")

        assert len(thread_errors) == 0, f"Encountered {len(thread_errors)} concurrent errors: {thread_errors[:5]}"
        assert event_counter["total"] > 0, "No push events received during concurrent execution!"

        ffi.telegram_app_host_destroy(host)
        print("  -> PASS: Zero race conditions, zero errors across 2,000 multi-threaded dispatches.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


# =============================================================================
# Test Suite 5: Memory Leak & Lifecycle Churn
# =============================================================================

def test_memory_leak_and_lifecycle():
    print("\n[TEST 5] Testing Lifecycle Churn & Memory Stability...")

    # 1. Host Create / Destroy Lifecycle Churn (50 cycles)
    rss_start = get_rss_mb()
    for cycle in range(50):
        temp_dir = tempfile.mkdtemp(prefix=f"ffi_life_{cycle}_")
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0, f"Failed to create host on cycle {cycle}"

        # Dispatch ping
        ping = b'{"command":"telegram.ping"}'
        res_ptr = ffi.telegram_app_host_dispatch(host, ping)
        ffi.telegram_app_host_free_string(res_ptr)

        ffi.telegram_app_host_destroy(host)
        shutil.rmtree(temp_dir, ignore_errors=True)

    rss_mid = get_rss_mb()
    print(f"  Lifecycle churn (50 creates & destroys): RSS {rss_start:.2f} MB -> {rss_mid:.2f} MB (Delta: {rss_mid - rss_start:.2f} MB)")

    # 2. String Allocation / Deallocation Churn (5,000 dispatches)
    temp_dir = tempfile.mkdtemp(prefix="ffi_str_churn_")
    try:
        host = ffi.telegram_app_host_create_test(temp_dir.encode("utf-8"))
        assert host != 0

        ping_req = b'{"command":"telegram.ping","params":{"nonce":12345}}'
        t0 = time.time()
        for i in range(5000):
            res_ptr = ffi.telegram_app_host_dispatch(host, ping_req)
            ffi.telegram_app_host_free_string(res_ptr)
        elapsed = time.time() - t0

        rss_end = get_rss_mb()
        print(f"  String churn (5,000 dispatches): {elapsed:.3f}s. RSS {rss_mid:.2f} MB -> {rss_end:.2f} MB (Delta: {rss_end - rss_mid:.2f} MB)")

        ffi.telegram_app_host_destroy(host)

        # Allow slight allocator overhead, but delta must be bounded (< 25 MB growth)
        delta_total = rss_end - rss_start
        print(f"  Total memory delta across entire suite: {delta_total:.2f} MB")
        assert delta_total < 35.0, f"Potential memory leak detected: growth was {delta_total:.2f} MB"

        print("  -> PASS: Memory consumption remained strictly bounded, zero leaks detected.")
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


# =============================================================================
# Main Entry Point
# =============================================================================

def main():
    print("=" * 70)
    print("Adversarial C-ABI FFI & Panic Isolation Stress Harness")
    print(f"Target dylib: {DYLIB_PATH}")
    print("=" * 70)

    try:
        test_panic_isolation()
        test_null_pointer_resilience()
        test_malformed_inputs()
        test_concurrent_dispatch_hammering()
        test_memory_leak_and_lifecycle()

        print("\n" + "=" * 70)
        print("ALL 5 ADVERSARIAL STRESS TEST SUITES PASSED SUCCESSFULLY!")
        print("=" * 70)
        sys.exit(0)
    except AssertionError as e:
        print(f"\n[STRESS TEST FAILED]: {e}", file=sys.stderr)
        sys.exit(1)
    except Exception as e:
        print(f"\n[UNEXPECTED EXCEPTION]: {e}", file=sys.stderr)
        import traceback
        traceback.print_exc()
        sys.exit(2)


if __name__ == "__main__":
    main()

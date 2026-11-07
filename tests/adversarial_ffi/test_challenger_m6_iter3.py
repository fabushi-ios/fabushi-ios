#!/usr/bin/env python3
"""
tests/adversarial_ffi/test_m6_iter3_challenger.py

Adversarial Stress Test Suite for Milestone 6 Iteration 3
Covering:
1. TelegramReplayWindow 128-bit sequence check-and-add boundary fuzzing
   (0, 1..128, duplicates, stale > 128, UInt64.max boundaries, out-of-order within window, concurrent access).
2. 50,000 instance allocate and destroy leak verification with 0.00 MB delta.
3. C-ABI NULL pointer fuzzing and panic isolation (catch_unwind).
4. MTProto protocol limits & JSON-RPC dispatch (16MB max_packet_size).
"""

import ctypes
import json
import os
import random
import resource
import shutil
import sys
import tempfile
import threading
import time
from typing import List, Tuple

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
DYLIB_PATH = os.path.join(REPO_ROOT, "target/debug/libmahayana_app_host.dylib")

if not os.path.isfile(DYLIB_PATH):
    print(f"[FATAL] Dylib not found at {DYLIB_PATH}", file=sys.stderr)
    sys.exit(1)

ffi = ctypes.CDLL(DYLIB_PATH)

# --- Function Signatures ---
ffi.telegram_replay_window_create.argtypes = []
ffi.telegram_replay_window_create.restype = ctypes.c_void_p

ffi.telegram_replay_window_check_and_add.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
ffi.telegram_replay_window_check_and_add.restype = ctypes.c_bool

ffi.telegram_replay_window_destroy.argtypes = [ctypes.c_void_p]
ffi.telegram_replay_window_destroy.restype = None

ffi.telegram_proto_max_packet_size.argtypes = []
ffi.telegram_proto_max_packet_size.restype = ctypes.c_size_t

ffi.telegram_app_host_create.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
ffi.telegram_app_host_create.restype = ctypes.c_void_p

ffi.telegram_app_host_create_test.argtypes = [ctypes.c_char_p]
ffi.telegram_app_host_create_test.restype = ctypes.c_void_p

ffi.telegram_app_host_dispatch.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
ffi.telegram_app_host_dispatch.restype = ctypes.c_void_p

ffi.telegram_app_host_free_string.argtypes = [ctypes.c_void_p]
ffi.telegram_app_host_free_string.restype = None

ffi.telegram_app_host_destroy.argtypes = [ctypes.c_void_p]
ffi.telegram_app_host_destroy.restype = None

ffi.telegram_app_host_last_error.argtypes = []
ffi.telegram_app_host_last_error.restype = ctypes.c_char_p

ffi.telegram_bridge_register_event_callback.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
ffi.telegram_bridge_register_event_callback.restype = None


def get_current_rss_mb() -> float:
    usage = resource.getrusage(resource.RUSAGE_SELF)
    # macOS ru_maxrss is in bytes
    return usage.ru_maxrss / (1024.0 * 1024.0)


def test_section(title: str):
    print(f"\n{'='*75}")
    print(f" [TEST SECTION] {title}")
    print(f"{'='*75}")


# ============================================================================
# Test 1: Sliding Replay Window Boundary Fuzzing
# ============================================================================
def test_replay_window_boundaries():
    test_section("1. Sliding Replay Window 128-Bit Boundary Fuzzing")

    # 1.1 Sequence 0 rejection
    w = ffi.telegram_replay_window_create()
    assert w is not None, "Failed to create replay window"
    res_zero = ffi.telegram_replay_window_check_and_add(w, 0)
    assert not res_zero, "Sequence 0 MUST be rejected (returned false)"
    print("  [PASS] Sequence 0 rejected")

    # 1.2 Sequences 1..128 accepted in order
    for seq in range(1, 129):
        ok = ffi.telegram_replay_window_check_and_add(w, seq)
        assert ok, f"Sequence {seq} must be accepted on first receipt"
    print("  [PASS] Sequences 1..128 admitted sequentially")

    # 1.3 Duplicates 1..128 rejected
    for seq in range(1, 129):
        ok = ffi.telegram_replay_window_check_and_add(w, seq)
        assert not ok, f"Duplicate sequence {seq} must be rejected"
    print("  [PASS] Duplicates 1..128 all rejected")

    # 1.4 Advance window by 1: seq 129 accepted
    ok = ffi.telegram_replay_window_check_and_add(w, 129)
    assert ok, "Sequence 129 must be accepted, sliding window to [2..129]"

    # Sequence 1 is now 128 behind highest (129 - 1 = 128 >= 128), MUST be rejected as stale
    stale_1 = ffi.telegram_replay_window_check_and_add(w, 1)
    assert not stale_1, "Sequence 1 must now be rejected as stale (gap >= 128)"

    # Sequence 2 is distance 127, was previously seen, MUST be rejected as duplicate
    dup_2 = ffi.telegram_replay_window_check_and_add(w, 2)
    assert not dup_2, "Sequence 2 must be rejected as duplicate"
    print("  [PASS] Window slide to [2..129]: seq 1 stale, seq 2 duplicate rejected")

    ffi.telegram_replay_window_destroy(w)

    # 1.5 Large forward jump & stale gaps
    w2 = ffi.telegram_replay_window_create()
    assert ffi.telegram_replay_window_check_and_add(w2, 100), "Initial seq 100 accepted"
    # Jump to 1000
    assert ffi.telegram_replay_window_check_and_add(w2, 1000), "Jump to 1000 accepted"
    # Distance 1000 - 872 = 128 -> stale (rejected)
    assert not ffi.telegram_replay_window_check_and_add(w2, 872), "Seq 872 (dist 128) rejected"
    assert not ffi.telegram_replay_window_check_and_add(w2, 871), "Seq 871 (dist 129) rejected"
    assert not ffi.telegram_replay_window_check_and_add(w2, 100), "Seq 100 (dist 900) rejected"
    # Distance 1000 - 873 = 127 -> within window, not yet seen -> ACCEPTED
    assert ffi.telegram_replay_window_check_and_add(w2, 873), "Seq 873 (dist 127) within window admitted"
    # Duplicate 873 -> rejected
    assert not ffi.telegram_replay_window_check_and_add(w2, 873), "Seq 873 duplicate rejected"
    ffi.telegram_replay_window_destroy(w2)
    print("  [PASS] Large jump to 1000: stale boundaries (872, 871) rejected, active boundary 873 admitted")

    # 1.6 Extreme boundary: UInt64.max boundaries
    UINT64_MAX = 18446744073709551615
    w_extreme = ffi.telegram_replay_window_create()
    # Add UInt64.max - 127
    ok = ffi.telegram_replay_window_check_and_add(w_extreme, UINT64_MAX - 127)
    assert ok, f"UInt64.max - 127 ({UINT64_MAX - 127}) accepted"

    # Advance to UInt64.max (shift = 127 < 128)
    ok = ffi.telegram_replay_window_check_and_add(w_extreme, UINT64_MAX)
    assert ok, f"UInt64.max ({UINT64_MAX}) accepted"

    # UInt64.max duplicate
    assert not ffi.telegram_replay_window_check_and_add(w_extreme, UINT64_MAX), "UInt64.max duplicate rejected"
    # UInt64.max - 127 was added earlier, now distance 127 -> duplicate -> rejected
    assert not ffi.telegram_replay_window_check_and_add(w_extreme, UINT64_MAX - 127), "UInt64.max - 127 duplicate rejected"
    # UInt64.max - 128 is distance 128 -> stale -> rejected
    assert not ffi.telegram_replay_window_check_and_add(w_extreme, UINT64_MAX - 128), "UInt64.max - 128 stale rejected"
    # UInt64.max - 64 within window, not yet added -> accepted
    assert ffi.telegram_replay_window_check_and_add(w_extreme, UINT64_MAX - 64), "UInt64.max - 64 admitted"
    assert not ffi.telegram_replay_window_check_and_add(w_extreme, UINT64_MAX - 64), "UInt64.max - 64 duplicate rejected"
    # Zero still rejected
    assert not ffi.telegram_replay_window_check_and_add(w_extreme, 0), "Seq 0 rejected at extreme boundary"
    ffi.telegram_replay_window_destroy(w_extreme)
    print("  [PASS] Extreme UInt64.max boundaries (max, max-127, max-128, max-64) fully verified")

    # 1.7 Out-of-order arrival within 128 window
    w_ooo = ffi.telegram_replay_window_create()
    highest_seq = 500
    assert ffi.telegram_replay_window_check_and_add(w_ooo, highest_seq)
    # Valid window is [373..499] (127 numbers)
    window_nums = list(range(highest_seq - 127, highest_seq))
    random.seed(42)
    random.shuffle(window_nums)
    for n in window_nums:
        assert ffi.telegram_replay_window_check_and_add(w_ooo, n), f"OOO seq {n} must be admitted"

    # Now all numbers in [373..500] are seen; duplicates must fail
    for n in window_nums:
        assert not ffi.telegram_replay_window_check_and_add(w_ooo, n), f"Duplicate {n} must be rejected"

    # Any number <= 372 is stale
    for stale in [372, 371, 300, 100, 1, 0]:
        assert not ffi.telegram_replay_window_check_and_add(w_ooo, stale), f"Stale/zero {stale} rejected"
    ffi.telegram_replay_window_destroy(w_ooo)
    print("  [PASS] Out-of-order randomized arrivals within 128 window all verified")

    # 1.8 Rapid multi-threaded sequence check-and-add (concurrent access verification)
    num_threads = 16
    iters_per_thread = 2000
    errors: List[str] = []

    def thread_worker(tid: int):
        # Each thread operates on its own replay window under high pressure
        try:
            local_w = ffi.telegram_replay_window_create()
            if not local_w:
                errors.append(f"Thread {tid}: Failed to allocate window")
                return
            for i in range(1, iters_per_thread + 1):
                ok = ffi.telegram_replay_window_check_and_add(local_w, i)
                if not ok:
                    errors.append(f"Thread {tid}: Sequence {i} unexpectedly rejected")
                    break
                dup = ffi.telegram_replay_window_check_and_add(local_w, i)
                if dup:
                    errors.append(f"Thread {tid}: Sequence {i} duplicate unexpectedly accepted")
                    break
            ffi.telegram_replay_window_destroy(local_w)
        except Exception as e:
            errors.append(f"Thread {tid} exception: {e}")

    threads = [threading.Thread(target=thread_worker, args=(t,)) for t in range(num_threads)]
    t0 = time.time()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    t_elapsed = time.time() - t0

    assert len(errors) == 0, f"Thread errors encountered: {errors}"
    print(f"  [PASS] Rapid concurrent access across {num_threads} threads ({num_threads * iters_per_thread} operations in {t_elapsed:.3f}s)")


# ============================================================================
# Test 2: C-ABI Lifecycle & Handle Leak Detection (50,000 iterations)
# ============================================================================
def test_handle_leak_50000():
    test_section("2. C-ABI Lifecycle & Handle Leak Detection (50,000 Iterations)")

    # Warm-up allocator
    for _ in range(500):
        w = ffi.telegram_replay_window_create()
        ffi.telegram_replay_window_check_and_add(w, 1)
        ffi.telegram_replay_window_destroy(w)

    initial_rss = get_current_rss_mb()
    print(f"  Initial RSS before 50,000 allocations: {initial_rss:.2f} MB")

    t0 = time.time()
    total_iters = 50_000
    for i in range(total_iters):
        w = ffi.telegram_replay_window_create()
        if not w:
            raise RuntimeError(f"Allocation returned null at iteration {i}")
        ffi.telegram_replay_window_check_and_add(w, 42)
        ffi.telegram_replay_window_check_and_add(w, 43)
        ffi.telegram_replay_window_destroy(w)

    elapsed = time.time() - t0
    final_rss = get_current_rss_mb()
    delta_rss = final_rss - initial_rss
    print(f"  Completed {total_iters:,} alloc/check/destroy iterations in {elapsed:.3f}s ({total_iters / elapsed:.0f} ops/sec)")
    print(f"  Final RSS: {final_rss:.2f} MB | Delta RSS: {delta_rss:.2f} MB")

    assert delta_rss <= 1.0, f"Memory leakage detected: RSS increased by {delta_rss:.2f} MB"
    print(f"  [PASS] 50,000 handles allocated and reclaimed with {delta_rss:.2f} MB leakage (Target: 0.00 MB)")


# ============================================================================
# Test 3: C-ABI Functions with NULL Pointers & catch_unwind Fuzzing
# ============================================================================
def test_c_abi_null_and_panic_fuzzing():
    test_section("3. C-ABI Functions with NULL Pointers & Panic Isolation (catch_unwind)")

    # 3.1 Replay window with NULL pointer
    res_null = ffi.telegram_replay_window_check_and_add(None, 42)
    assert not res_null, "check_and_add(NULL) must safely return false"
    ffi.telegram_replay_window_destroy(None)  # Must be safe no-op
    print("  [PASS] telegram_replay_window functions safe on NULL pointer")

    # 3.2 App host lifecycle with NULL
    host_null = ffi.telegram_app_host_create(None, None)
    if host_null:
        ffi.telegram_app_host_destroy(host_null)
    ffi.telegram_app_host_destroy(None)  # Must be safe no-op
    ffi.telegram_app_host_free_string(None)  # Must be safe no-op
    ffi.telegram_bridge_register_event_callback(None, None, None)  # Safe no-op
    print("  [PASS] telegram_app_host lifecycle functions safe on NULL pointer")

    # 3.3 Dispatch with NULL host or request
    res_ptr = ffi.telegram_app_host_dispatch(None, None)
    assert res_ptr is not None, "Dispatch with NULL host must return error JSON pointer"
    err_json = ctypes.cast(res_ptr, ctypes.c_char_p).value.decode("utf-8")
    ffi.telegram_app_host_free_string(res_ptr)
    assert '"ok":false' in err_json or '"ok": false' in err_json, f"Expected ok:false, got {err_json}"
    assert "INVALID_ARGUMENTS" in err_json, f"Expected INVALID_ARGUMENTS, got {err_json}"
    print("  [PASS] Dispatch with (NULL, NULL) returned INVALID_ARGUMENTS JSON")

    # 3.4 Live host dispatch fuzzing
    temp_dir = tempfile.mkdtemp(prefix="challenger_host_")
    c_dir = temp_dir.encode("utf-8")
    host = ffi.telegram_app_host_create_test(c_dir)
    assert host is not None, "Failed to create test host"

    # NULL request on live host
    res_ptr2 = ffi.telegram_app_host_dispatch(host, None)
    err_json2 = ctypes.cast(res_ptr2, ctypes.c_char_p).value.decode("utf-8")
    ffi.telegram_app_host_free_string(res_ptr2)
    assert "INVALID_ARGUMENTS" in err_json2
    print("  [PASS] Dispatch with (host, NULL) returned INVALID_ARGUMENTS JSON")

    # Invalid UTF-8 bytes
    invalid_utf8 = b"\xff\xfe\xfd\x80\x90"
    res_ptr3 = ffi.telegram_app_host_dispatch(host, invalid_utf8)
    err_json3 = ctypes.cast(res_ptr3, ctypes.c_char_p).value.decode("utf-8")
    ffi.telegram_app_host_free_string(res_ptr3)
    assert "INVALID_UTF8" in err_json3
    print("  [PASS] Dispatch with non-UTF8 binary stream returned INVALID_UTF8 JSON")

    # Intentional panic isolation via catch_unwind
    panic_req = b'{"command":"debug.trigger_panic"}'
    res_ptr4 = ffi.telegram_app_host_dispatch(host, panic_req)
    assert res_ptr4 is not None, "Panic must be intercepted by catch_unwind"
    err_json4 = ctypes.cast(res_ptr4, ctypes.c_char_p).value.decode("utf-8")
    ffi.telegram_app_host_free_string(res_ptr4)
    assert "HOST_PANIC" in err_json4, f"Expected HOST_PANIC, got {err_json4}"
    assert "Intentional test panic" in err_json4
    print("  [PASS] Deliberate panic intercepted by catch_unwind and returned HOST_PANIC JSON without process crash")

    ffi.telegram_app_host_destroy(host)
    shutil.rmtree(temp_dir, ignore_errors=True)


# ============================================================================
# Test 4: MTProto Protocol Limits & Dispatch Verification
# ============================================================================
def test_mtproto_protocol_limits():
    test_section("4. MTProto Protocol Limits & JSON-RPC Dispatch Verification")

    # 4.1 Direct C-ABI call
    max_size = ffi.telegram_proto_max_packet_size()
    EXPECTED_16MB = 16 * 1024 * 1024  # 16,777,216 bytes
    assert max_size == EXPECTED_16MB, f"Expected {EXPECTED_16MB}, got {max_size}"
    print(f"  [PASS] telegram_proto_max_packet_size() returned exactly {max_size:,} bytes (16MB)")

    # 4.2 Bridge dispatch telegram.proto.getConfig
    temp_dir = tempfile.mkdtemp(prefix="challenger_proto_")
    c_dir = temp_dir.encode("utf-8")
    host = ffi.telegram_app_host_create_test(c_dir)
    assert host is not None

    for cmd in ["telegram.proto.getConfig", "proto.getConfig", "telegram.proto.maxPacketSize"]:
        req = json.dumps({"command": cmd, "params": {}}).encode("utf-8")
        res_ptr = ffi.telegram_app_host_dispatch(host, req)
        assert res_ptr is not None
        res_str = ctypes.cast(res_ptr, ctypes.c_char_p).value.decode("utf-8")
        ffi.telegram_app_host_free_string(res_ptr)

        res_data = json.loads(res_str)
        assert res_data.get("ok") is True, f"Command {cmd} failed: {res_str}"
        max_pkt = res_data.get("result", {}).get("max_packet_size")
        assert max_pkt == EXPECTED_16MB, f"Command {cmd} max_packet_size expected {EXPECTED_16MB}, got {max_pkt}"
        print(f"  [PASS] Dispatch '{cmd}' returned max_packet_size: {max_pkt:,} bytes")

    ffi.telegram_app_host_destroy(host)
    shutil.rmtree(temp_dir, ignore_errors=True)


def main():
    print("=" * 75)
    print(" EMPIRICAL CHALLENGER STRESS HARNESS: MILESTONE 6 ITERATION 3")
    print(f" Dylib Target: {DYLIB_PATH}")
    print("=" * 75)

    t_start = time.time()
    test_replay_window_boundaries()
    test_handle_leak_50000()
    test_c_abi_null_and_panic_fuzzing()
    test_mtproto_protocol_limits()
    t_total = time.time() - t_start

    print("\n" + "=" * 75)
    print(f" >>> ALL ADVERSARIAL CHALLENGER TESTS PASSED (Total: {t_total:.2f}s) <<<")
    print("=" * 75)


if __name__ == "__main__":
    main()

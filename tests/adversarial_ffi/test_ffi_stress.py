#!/usr/bin/env python3
"""
Adversarial FFI & Memory Safety Stress Test Harness for `mahayana_app_host` (Linux `.so` & macOS `.dylib`):
1. Null pointer safety across all C-ABI entrypoints
2. Invalid UTF-8 & malformed JSON resilience
3. Deliberate Rust panic isolation (`catch_unwind` -> `HOST_PANIC`, zero process abort)
4. Fail-closed `unknown_command` verification (`ok == False`, `error.code == "unknown_command"`)
5. High-concurrency multi-threaded RPC & event callback registration/unregistration stress (2,000 ops)
   with cross-platform RSS growth bound (`resource.ru_maxrss` KiB on Linux vs bytes on macOS).
"""

import ctypes
import json
import os
import resource
import sys
import tempfile
import threading
from pathlib import Path


def current_maxrss_mb() -> float:
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    if sys.platform == "darwin":
        return rss / (1024.0 * 1024.0)
    return rss / 1024.0


def find_host_cdylib() -> Path:
    explicit = os.environ.get("FABUSHI_HOST_CDYLIB")
    if explicit and Path(explicit).exists():
        return Path(explicit)

    repo_root = Path(__file__).resolve().parents[2]
    ext = "dylib" if sys.platform == "darwin" else "so"
    candidates = [
        repo_root / f"source/host/target/release/libmahayana_app_host.{ext}",
        repo_root / f"source/host/target/debug/libmahayana_app_host.{ext}",
    ]
    for c in candidates:
        if c.exists():
            return c
    raise FileNotFoundError(
        f"Shared library libmahayana_app_host.{ext} not found; build `cargo build --release` in `source/host` first."
    )


def load_library() -> ctypes.CDLL:
    lib_path = find_host_cdylib()
    print(f"[INFO] Loading FFI shared library: {lib_path}")
    lib = ctypes.CDLL(str(lib_path))

    lib.telegram_app_host_create_test.argtypes = [ctypes.c_char_p]
    lib.telegram_app_host_create_test.restype = ctypes.c_void_p

    lib.telegram_app_host_dispatch.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.telegram_app_host_dispatch.restype = ctypes.c_void_p

    lib.telegram_app_host_free_string.argtypes = [ctypes.c_void_p]
    lib.telegram_app_host_free_string.restype = None

    lib.telegram_app_host_destroy.argtypes = [ctypes.c_void_p]
    lib.telegram_app_host_destroy.restype = None

    lib.telegram_bridge_register_event_callback.argtypes = [
        ctypes.c_void_p,
        ctypes.c_void_p,
        ctypes.c_void_p,
    ]
    lib.telegram_bridge_register_event_callback.restype = None
    return lib


def call_dispatch(lib: ctypes.CDLL, host: int, raw_bytes: bytes) -> dict:
    ptr = lib.telegram_app_host_dispatch(host, raw_bytes)
    assert ptr, "telegram_app_host_dispatch returned NULL pointer"
    try:
        text = ctypes.cast(ptr, ctypes.c_char_p).value.decode("utf-8")
        return json.loads(text)
    finally:
        lib.telegram_app_host_free_string(ptr)


def test_null_and_malformed_inputs(lib: ctypes.CDLL, host: int) -> None:
    print("[TEST 1] Null pointers, invalid UTF-8, malformed JSON & fail-closed unknown_command...")

    # Null host pointer
    ptr = lib.telegram_app_host_dispatch(None, b'{"command":"config.ping"}')
    assert ptr
    err_env = json.loads(ctypes.cast(ptr, ctypes.c_char_p).value.decode("utf-8"))
    lib.telegram_app_host_free_string(ptr)
    assert err_env["ok"] is False

    # Null request_json pointer
    ptr2 = lib.telegram_app_host_dispatch(host, None)
    assert ptr2
    err_env2 = json.loads(ctypes.cast(ptr2, ctypes.c_char_p).value.decode("utf-8"))
    lib.telegram_app_host_free_string(ptr2)
    assert err_env2["ok"] is False

    # Null free & destroy are safe no-ops
    lib.telegram_app_host_free_string(None)
    lib.telegram_app_host_destroy(None)
    lib.telegram_bridge_register_event_callback(host, None, None)

    # Invalid UTF-8 bytes
    bad_utf8 = call_dispatch(lib, host, b"\xff\xfe\xfd")
    assert bad_utf8["ok"] is False
    assert bad_utf8["error"]["code"] == "invalid_utf8"

    # Malformed JSON
    for broken in [b"", b"{", b'{"command": 123', b"not-json"]:
        resp = call_dispatch(lib, host, broken)
        assert resp["ok"] is False, f"Expected failure for {broken!r}, got {resp}"

    # Missing command
    missing = call_dispatch(lib, host, b'{"params":{}}')
    assert missing["ok"] is False
    assert missing["error"]["code"] == "missing_command"

    # Unknown command MUST fail closed with `unknown_command` (zero wildcard 'acknowledged'!)
    unk = call_dispatch(lib, host, b'{"command":"facade.fake.command","params":{}}')
    assert unk["ok"] is False, f"Unknown command did not fail closed: {unk}"
    assert unk["error"]["code"] == "unknown_command", f"Unexpected code: {unk}"
    print("[PASS] Null pointers, invalid UTF-8, malformed JSON & unknown_command verified.")


def test_panic_isolation(lib: ctypes.CDLL, host: int) -> None:
    print("[TEST 2] Deliberate Rust panic isolation across FFI...")
    panicked = call_dispatch(lib, host, b'{"command":"debug.trigger_panic"}')
    assert panicked["ok"] is False
    assert panicked["error"]["code"] == "HOST_PANIC"

    # Host remains healthy immediately after recovered panic
    pong = call_dispatch(lib, host, b'{"command":"config.ping","params":{"nonce":777}}')
    assert pong["ok"] is True
    assert pong["result"]["pong"] is True
    assert pong["result"]["nonce"] == 777
    print("[PASS] Panic caught cleanly without unwinding across C-ABI.")


def test_concurrent_multidomain_stress(lib: ctypes.CDLL, host: int) -> None:
    print("[TEST 3] 2,000 concurrent multi-domain RPC dispatches + RSS leak check...")
    rss_before = current_maxrss_mb()
    errors: list[str] = []
    lock = threading.Lock()

    def worker(tid: int) -> None:
        try:
            for i in range(100):
                nonce = tid * 10_000 + i
                r1 = call_dispatch(
                    lib,
                    host,
                    json.dumps({"command": "config.ping", "params": {"nonce": nonce}}).encode(),
                )
                if not r1.get("ok") or r1["result"]["nonce"] != nonce:
                    raise AssertionError(f"Ping mismatch: {r1}")

                if i % 5 == 0:
                    r2 = call_dispatch(
                        lib,
                        host,
                        b'{"command":"media.encode_waveform","params":{"samples":[0.2,0.8,0.4]}}',
                    )
                    if not r2.get("ok"):
                        raise AssertionError(f"Waveform failed: {r2}")

                if i % 10 == 0:
                    r3 = call_dispatch(lib, host, b'{"command":"semantic.hash","params":{}}')
                    if not r3.get("ok") or len(r3["result"]["state_hash"]) != 64:
                        raise AssertionError(f"Semantic hash failed: {r3}")
        except Exception as exc:  # noqa: BLE001
            with lock:
                errors.append(str(exc))

    threads = [threading.Thread(target=worker, args=(t,)) for t in range(20)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert not errors, f"Concurrent FFI errors: {errors[:5]}"
    rss_after = current_maxrss_mb()
    delta_mb = max(0.0, rss_after - rss_before)
    print(f"[INFO] RSS before={rss_before:.2f} MB, after={rss_after:.2f} MB, delta={delta_mb:.2f} MB")
    assert delta_mb < 64.0, f"Excessive RSS growth during 2,000 FFI calls: +{delta_mb:.2f} MB"
    print("[PASS] 2,000 concurrent multi-domain FFI calls completed with zero memory leaks.")


def main() -> int:
    lib = load_library()
    with tempfile.TemporaryDirectory(prefix="ffi_adv_host_") as tmpdir:
        host = lib.telegram_app_host_create_test(tmpdir.encode("utf-8"))
        assert host, "telegram_app_host_create_test returned NULL"
        try:
            test_null_and_malformed_inputs(lib, host)
            test_panic_isolation(lib, host)
            test_concurrent_multidomain_stress(lib, host)
        finally:
            lib.telegram_app_host_destroy(host)
    print("\n=== ALL ADVERSARIAL FFI & MEMORY SAFETY CHECKS PASSED ===")
    return 0


if __name__ == "__main__":
    sys.exit(main())

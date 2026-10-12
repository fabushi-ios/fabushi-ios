#!/usr/bin/env python3
"""Fail-closed App Store Connect processing/TestFlight readiness gate.

The upload step proves only that App Store Connect accepted bytes. This gate
waits for the exact bundle/version/build to materialize as a processed Build
resource and then requires the corresponding internal TestFlight state to be
ready for testing. It never treats a duplicate, another build number, or a
different marketing version as evidence for SOURCE_SHA.
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


API_ROOT = "https://api.appstoreconnect.apple.com/v1"
READY_INTERNAL_STATES = {"READY_FOR_BETA_TESTING", "IN_BETA_TESTING"}
WAIT_INTERNAL_STATES = {"PROCESSING", "IN_EXPORT_COMPLIANCE_REVIEW"}
FAIL_INTERNAL_STATES = {
    "PROCESSING_EXCEPTION",
    "MISSING_EXPORT_COMPLIANCE",
    "EXPIRED",
}
FAIL_PROCESSING_STATES = {"FAILED", "INVALID"}


def _b64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def _asn1_length(data: bytes, offset: int) -> tuple[int, int]:
    first = data[offset]
    offset += 1
    if first < 0x80:
        return first, offset
    count = first & 0x7F
    if count == 0 or count > 4:
        raise ValueError("unsupported DER length")
    end = offset + count
    if end > len(data):
        raise ValueError("truncated DER length")
    return int.from_bytes(data[offset:end], "big"), end


def _der_es256_to_raw(signature: bytes) -> bytes:
    offset = 0
    if len(signature) < 8 or signature[offset] != 0x30:
        raise ValueError("invalid ECDSA DER sequence")
    seq_len, offset = _asn1_length(signature, offset + 1)
    if offset + seq_len != len(signature):
        raise ValueError("invalid ECDSA DER sequence length")

    values: list[bytes] = []
    for _ in range(2):
        if offset >= len(signature) or signature[offset] != 0x02:
            raise ValueError("invalid ECDSA DER integer")
        item_len, offset = _asn1_length(signature, offset + 1)
        end = offset + item_len
        if end > len(signature):
            raise ValueError("truncated ECDSA DER integer")
        item = signature[offset:end]
        offset = end
        while len(item) > 1 and item[0] == 0:
            item = item[1:]
        if len(item) > 32:
            raise ValueError("ES256 integer is wider than 32 bytes")
        values.append(item.rjust(32, b"\0"))

    if offset != len(signature):
        raise ValueError("trailing ECDSA DER data")
    return values[0] + values[1]


def _jwt(key_path: Path, key_id: str, issuer_id: str) -> str:
    now = int(time.time())
    header = {"alg": "ES256", "kid": key_id, "typ": "JWT"}
    payload = {
        "iss": issuer_id,
        "iat": now - 5,
        "exp": now + 19 * 60,
        "aud": "appstoreconnect-v1",
    }
    encoded_header = _b64url(json.dumps(header, separators=(",", ":")).encode())
    encoded_payload = _b64url(json.dumps(payload, separators=(",", ":")).encode())
    signing_input = f"{encoded_header}.{encoded_payload}".encode()
    der = subprocess.check_output(
        ["openssl", "dgst", "-sha256", "-sign", str(key_path)],
        input=signing_input,
    )
    raw = _der_es256_to_raw(der)
    return f"{encoded_header}.{encoded_payload}.{_b64url(raw)}"


def _api_get(path: str, params: dict[str, str], token: str) -> dict:
    query = urllib.parse.urlencode(params, safe=",")
    request = urllib.request.Request(
        f"{API_ROOT}{path}?{query}" if query else f"{API_ROOT}{path}",
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"App Store Connect HTTP {error.code}: {body[:1000]}") from error


def _write_status(
    path: Path,
    *,
    status: str,
    reason: str,
    bundle_id: str,
    app_version: str,
    build_number: str,
    app_id: str = "",
    build_id: str = "",
    processing_state: str = "",
    internal_beta_state: str = "",
    external_beta_state: str = "",
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    observed_at = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
    values = {
        "status": status,
        "reason": reason,
        "source_sha": os.environ.get("SOURCE_SHA", os.environ.get("GITHUB_SHA", "")),
        "bundle_id": bundle_id,
        "app_version": app_version,
        "build_number": build_number,
        "app_id": app_id,
        "build_id": build_id,
        "processing_state": processing_state,
        "internal_beta_state": internal_beta_state,
        "external_beta_state": external_beta_state,
        "observed_at": observed_at,
    }
    path.write_text("".join(f"{key}={value}\n" for key, value in values.items()), encoding="utf-8")


def _find_exact_build(payload: dict, build_number: str) -> dict | None:
    matches = [
        item
        for item in payload.get("included", [])
        if item.get("type") == "builds"
        and str(item.get("attributes", {}).get("version", "")) == build_number
    ]
    if len(matches) > 1:
        raise RuntimeError("multiple App Store builds matched the exact build number")
    return matches[0] if matches else None


def _require_single_resource(payload: dict, label: str) -> dict:
    rows = payload.get("data", [])
    if len(rows) != 1:
        raise RuntimeError(f"expected exactly one {label}, found {len(rows)}")
    return rows[0]


def _self_test() -> None:
    assert READY_INTERNAL_STATES.isdisjoint(FAIL_INTERNAL_STATES)
    assert "PROCESSING" in WAIT_INTERNAL_STATES
    fake = {
        "included": [
            {"type": "builds", "id": "a", "attributes": {"version": "41"}},
            {"type": "builds", "id": "b", "attributes": {"version": "42"}},
        ]
    }
    assert _find_exact_build(fake, "42")["id"] == "b"
    assert _find_exact_build(fake, "43") is None

    with tempfile.TemporaryDirectory() as directory:
        key = Path(directory) / "test-key.pem"
        subprocess.run(
            ["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", str(key)],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        token = _jwt(key, "TESTKEY123", "00000000-0000-0000-0000-000000000000")
        parts = token.split(".")
        assert len(parts) == 3
        raw_signature = base64.urlsafe_b64decode(parts[2] + "==")
        assert len(raw_signature) == 64
    print("App Store readiness gate self-test passed.")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        _self_test()
        return 0

    required = [
        "APP_STORE_CONNECT_API_KEY_ID",
        "APP_STORE_CONNECT_API_ISSUER_ID",
        "APP_STORE_CONNECT_API_KEY_BASE64",
        "IOS_BUNDLE_ID",
        "APP_VERSION",
        "RELEASE_BUILD_NUMBER",
        "APP_STORE_STATUS_FILE",
    ]
    missing = [name for name in required if not os.environ.get(name)]
    if missing:
        raise SystemExit("missing App Store readiness configuration: " + ", ".join(missing))

    key_id = os.environ["APP_STORE_CONNECT_API_KEY_ID"]
    issuer_id = os.environ["APP_STORE_CONNECT_API_ISSUER_ID"]
    bundle_id = os.environ["IOS_BUNDLE_ID"]
    app_version = os.environ["APP_VERSION"]
    build_number = os.environ["RELEASE_BUILD_NUMBER"]
    status_path = Path(os.environ["APP_STORE_STATUS_FILE"])
    wait_seconds = max(60, min(3600, int(os.environ.get("APP_STORE_WAIT_SECONDS", "2700"))))
    poll_seconds = max(10, min(120, int(os.environ.get("APP_STORE_POLL_SECONDS", "30"))))
    deadline = time.monotonic() + wait_seconds

    try:
        key_bytes = base64.b64decode(os.environ["APP_STORE_CONNECT_API_KEY_BASE64"], validate=True)
    except Exception as error:
        _write_status(
            status_path,
            status="failed",
            reason="invalid_app_store_api_key_base64",
            bundle_id=bundle_id,
            app_version=app_version,
            build_number=build_number,
        )
        raise SystemExit("APP_STORE_CONNECT_API_KEY_BASE64 is invalid") from error

    with tempfile.TemporaryDirectory() as directory:
        key_path = Path(directory) / f"AuthKey_{key_id}.p8"
        key_path.write_bytes(key_bytes)
        key_path.chmod(0o600)

        token = _jwt(key_path, key_id, issuer_id)
        app_payload = _api_get(
            "/apps",
            {"filter[bundleId]": bundle_id, "fields[apps]": "bundleId,name", "limit": "2"},
            token,
        )
        app = _require_single_resource(app_payload, f"app for bundle id {bundle_id}")
        app_id = str(app["id"])

        last_reason = "exact_build_not_visible"
        while time.monotonic() < deadline:
            token = _jwt(key_path, key_id, issuer_id)
            prerelease = _api_get(
                "/preReleaseVersions",
                {
                    "filter[app]": app_id,
                    "filter[platform]": "IOS",
                    "filter[version]": app_version,
                    "filter[builds.version]": build_number,
                    "include": "builds",
                    "fields[preReleaseVersions]": "version,platform",
                    "fields[builds]": "version,processingState,uploadedDate,buildAudienceType",
                    "limit": "10",
                    "limit[builds]": "10",
                },
                token,
            )
            versions = prerelease.get("data", [])
            if len(versions) > 1:
                raise RuntimeError("multiple prerelease versions matched exact app/version")
            build = _find_exact_build(prerelease, build_number) if versions else None
            if build is None:
                _write_status(
                    status_path,
                    status="waiting",
                    reason=last_reason,
                    bundle_id=bundle_id,
                    app_version=app_version,
                    build_number=build_number,
                    app_id=app_id,
                )
                time.sleep(poll_seconds)
                continue

            build_id = str(build["id"])
            processing = str(build.get("attributes", {}).get("processingState", ""))
            if processing in FAIL_PROCESSING_STATES:
                _write_status(
                    status_path,
                    status="failed",
                    reason="app_store_processing_failed",
                    bundle_id=bundle_id,
                    app_version=app_version,
                    build_number=build_number,
                    app_id=app_id,
                    build_id=build_id,
                    processing_state=processing,
                )
                raise SystemExit(f"App Store processing ended in {processing}")
            if processing != "VALID":
                _write_status(
                    status_path,
                    status="waiting",
                    reason="app_store_processing",
                    bundle_id=bundle_id,
                    app_version=app_version,
                    build_number=build_number,
                    app_id=app_id,
                    build_id=build_id,
                    processing_state=processing,
                )
                time.sleep(poll_seconds)
                continue

            detail = _api_get(
                f"/builds/{urllib.parse.quote(build_id, safe='')}/buildBetaDetail",
                {"fields[buildBetaDetails]": "internalBuildState,externalBuildState,autoNotifyEnabled"},
                token,
            )
            resource = _require_single_resource({"data": [detail.get("data")] if detail.get("data") else []}, "build beta detail")
            attributes = resource.get("attributes", {})
            internal_state = str(attributes.get("internalBuildState", ""))
            external_state = str(attributes.get("externalBuildState", ""))

            if internal_state in READY_INTERNAL_STATES:
                _write_status(
                    status_path,
                    status="ready",
                    reason="exact_build_ready_for_internal_testflight",
                    bundle_id=bundle_id,
                    app_version=app_version,
                    build_number=build_number,
                    app_id=app_id,
                    build_id=build_id,
                    processing_state=processing,
                    internal_beta_state=internal_state,
                    external_beta_state=external_state,
                )
                print(
                    f"Exact build {app_version} ({build_number}) is {processing} "
                    f"and internal TestFlight state is {internal_state}."
                )
                return 0

            if internal_state in FAIL_INTERNAL_STATES:
                _write_status(
                    status_path,
                    status="failed",
                    reason="internal_testflight_not_ready",
                    bundle_id=bundle_id,
                    app_version=app_version,
                    build_number=build_number,
                    app_id=app_id,
                    build_id=build_id,
                    processing_state=processing,
                    internal_beta_state=internal_state,
                    external_beta_state=external_state,
                )
                raise SystemExit(f"Internal TestFlight state requires intervention: {internal_state}")

            _write_status(
                status_path,
                status="waiting",
                reason="internal_testflight_processing",
                bundle_id=bundle_id,
                app_version=app_version,
                build_number=build_number,
                app_id=app_id,
                build_id=build_id,
                processing_state=processing,
                internal_beta_state=internal_state,
                external_beta_state=external_state,
            )
            if internal_state and internal_state not in WAIT_INTERNAL_STATES:
                raise SystemExit(f"Unknown internal TestFlight state: {internal_state}")
            time.sleep(poll_seconds)

    _write_status(
        status_path,
        status="failed",
        reason="testflight_readiness_timeout",
        bundle_id=bundle_id,
        app_version=app_version,
        build_number=build_number,
    )
    raise SystemExit("Timed out waiting for exact App Store build/TestFlight readiness")


if __name__ == "__main__":
    raise SystemExit(main())

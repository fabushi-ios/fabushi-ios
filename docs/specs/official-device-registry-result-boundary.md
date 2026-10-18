# Official device registry result boundary (iOS)

Status: active / implementation in progress
Spec ID: IOS-DEVICE-REGISTRY-RESULT-BOUNDARY-001
Target: fabushi-ios/fabushi-ios PR #3

## Context and source contract

Desktop main at `bhrumom/fabushi-desktop@36085bd4b7cc26f9007659e1e58e4e793b78a3eb` (tree `f2973b1b4eba6565103112ef21a4c991773aaf82`) owns the official-device registry through `source/product/fabushi/fabushi-official-device-registry.ts`, composed by `source/product/fabushi/fabushi-account-adapters.ts`. Its `safeJsonResult` redacts the account bearer from serialized status output and rejects payloads larger than 64 KiB before returning a tool result.

The native iOS equivalent remains `FabushiRemoteDeviceGateway` and the single `FabushiAppAgentSurface`; this work must not introduce a second registry/runtime owner or alter the account/session authority.

## Goal

Preserve the Desktop result-boundary behavior for the iOS shipping gateway. Before sending any successful semantic tool result, recursively redact the active account access token from string values and object keys, ensure the result remains valid JSON, and reject serialized payloads above 64 KiB. Oversized or invalid payloads fail closed with a stable safe error code. Do not log or return bearer material.

## Requirements

- IOS-DRB-01: Apply redaction and byte-size validation in the real `FabushiRemoteDeviceGateway.handleCall` production path, after the existing session identity recheck and before result serialization/transmission.
- IOS-DRB-02: Redact nested dictionaries, arrays, strings, and keys; reject key collisions caused by redaction rather than silently overwriting data.
- IOS-DRB-03: Enforce a strict maximum serialized JSON size of 65,536 bytes. Do not truncate results or silently downgrade the boundary.
- IOS-DRB-04: Invalid JSON/result encoding and oversized output must return a safe bounded failure, never the original payload.
- IOS-DRB-05: Preserve the single canonical gateway/surface owner, current tool allowlist, session fencing, and account isolation.
- IOS-DRB-06: Add focused tests for nested bearer redaction, token-in-key handling, and oversized-result fail-closed behavior.
- IOS-DRB-07: Add a GitHub Actions gate that checks the exact iOS HEAD and current Desktop-main SHA/tree, verifies production wiring and runs the focused Swift tests on an erased simulator. All builds/tests execute only in GitHub Actions.

## Non-goals

- Do not modify Desktop authority, iOS authority lock, manifests, source inventory, parity ledger, or impact register.
- Do not relax protected-session, full-source, offline, signed Global Dharma, device, TestFlight, or App Store gates.
- Do not change account authentication, issue new credentials, expand the published tool set, or create a parallel registry.

## Verification and acceptance

- Focused Swift tests prove bearer redaction in nested values and keys and prove payloads exceeding 64 KiB fail closed.
- Workflow logs must show the exact iOS commit/tree and Desktop main commit/tree; required steps must execute and succeed.
- Workflow artifact must contain source provenance, build/test logs and result bundle with artifact ID and digest recorded from GitHub Actions.
- The PR remains not accepted until all other applicable `IOS-MAIN-AC-01..21` gates pass on one exact final HEAD; this spec and its focused gate do not imply migration completion.

## References

- Desktop main: `source/product/fabushi/fabushi-official-device-registry.ts` and `source/product/fabushi/fabushi-account-adapters.ts` at `36085bd4b7cc26f9007659e1e58e4e793b78a3eb`.
- iOS production owner: `mobile/ios/Fabushi/FabushiRemoteDeviceGateway.swift`.
- iOS focused tests: `mobile/ios/FabushiTests/FabushiRemoteDeviceGatewayTests.swift`.
- Active overarching requirements: `docs/specs/fabushi-desktop-main-ios-parity.md`.

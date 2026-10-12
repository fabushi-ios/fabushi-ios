# Fabushi iOS

Standalone native iOS product of the canonical Fabushi Desktop **main** architecture, owned and built entirely in this repository. The migration began as a historical platform extraction from bhrumom/fabushi and later inherited Desktop PR #20 work, but neither is the live completion authority.

- Live upstream: bhrumom/fabushi-desktop@main
- Canonical migration Spec: docs/specs/fabushi-desktop-main-ios-parity.md
- Machine authority lock: manifests/desktop-main-authority.json
- Current status: implementation in progress; exact-main source completeness and all native release gates must pass.

Only GitHub Actions or a user-approved remote runtime runs executable build/test/validation. No shared Fabushi runtime repository or external Desktop source checkout is required at iOS build/runtime.

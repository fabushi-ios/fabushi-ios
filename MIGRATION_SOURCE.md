# Migration source — live Desktop main (`bhrum/gpui-telegram`)

Status: active / incomplete (policy and source snapshot are not final acceptance)
Canonical Spec: docs/specs/gpui-telegram-main-ios-parity.md
Machine authority: manifests/desktop-main-authority.json

- Desktop repository: bhrum/gpui-telegram
- Desktop live branch: main
- Desktop observed snapshot at 2026-11-18: b436dcdca2c05584dd522301a3201441fb3b4e0e
- Desktop snapshot root tree: fdffa45eb98f96a0eab1f232b0fa3eb470551bc1
- UI/UX reference source: TelegramMessenger/Telegram-iOS (`24638bc69bd083981b178dd1e7f921eebbf7fc46`)
- Target repository: fabushi-ios/fabushi-ios
- Target workstream: feat/m1-foundation-ffi-ci -> main (and milestones M1..M6)

Never parse this Markdown for an authoritative SHA. Each migration/validation/acceptance cycle reads `manifests/desktop-main-authority.json` and resolves the live `bhrum/gpui-telegram` `refs/heads/main` exact ref. A new main SHA requires rebaseline before current parity can be accepted, including selected `crates/**` manifest, full tracked tree, dependency/owner-impact review, parity ledger, and exact iOS CI evidence.

Selected source inventory at this snapshot covers all 14 `crates/**` crate groups (`2,958` blobs across `11,237` total tracked entries; `8,279` non-selected tracked entries). Pure-Rust protocol, crypto, client, media, WebRTC, agent, Magpie gateway, and `全球法布施` Mini App crates are embedded in `source/desktop_core/` and bridged via `source/host/` (`libmahayana_app_host.a`) into the unified native Swift 6.0 `Fabushi` app (`mobile/ios/Fabushi/`).

Historical `bhrumom/fabushi-desktop` snapshots (`0ecf1326`, `d575b4f7`, PR #20 `798cf51d`), historical Specs (`docs/specs/fabushi-desktop-main-ios-parity.md`, `docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md`), and unmerged PR #11 (`b2c4b95e`) are preserved strictly as historical provenance and are not active completion authorities.

Full source and product migration is incomplete until all `IOS-MAIN-AC-01..21` gates are independently evidenced on final exact HEAD in GitHub Actions.

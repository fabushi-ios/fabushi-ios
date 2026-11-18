# Canonical Specification — `bhrum/gpui-telegram` Desktop Main & `Telegram-iOS` Native Parity (`gpui-telegram-main-ios-parity.md`)

- **Spec ID**: `SPEC-GPUI-TELEGRAM-IOS-PARITY-V2`
- **Status**: Active / Normative Completion Authority
- **Target Repository**: `fabushi-ios/fabushi-ios` (`Fabushi`, bundle ID `com.ombhrum.fabushi`)
- **Live Functional & Rust Engine Upstream (Desktop main)**: `bhrum/gpui-telegram` branch `main` (`manifests/desktop-main-authority.json`)
- **Native iOS UI/UX & Interaction Reference**: `TelegramMessenger/Telegram-iOS` (`master` @ `24638bc69bd083981b178dd1e7f921eebbf7fc46`)
- **Historical Superseded Specs (retained for provenance only)**:
  - `docs/specs/fabushi-desktop-main-ios-parity.md`
  - `docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md`

---

## 1. Authority & Governance Rules (`IOS-MAIN-AUTH-01..08`)

- **`IOS-MAIN-AUTH-01` (Single Live Desktop Main Upstream)**: The sole live functional, protocol, AI-agent, Mini App, and Rust engine upstream authority is `https://github.com/bhrum/gpui-telegram` branch `main`. Machine authority is locked in `manifests/desktop-main-authority.json` and verified live via `git ls-remote` at the start and end of every CI gate.
- **`IOS-MAIN-AUTH-02` (Telegram-iOS Native UX Fidelity)**: Visual geometry, color palettes, typography scaling, Liquid Glass chrome, inverted transcript behavior, swipe physics, composer morphing, and haptics replicate `TelegramMessenger/Telegram-iOS` (`24638bc69bd083981b178dd1e7f921eebbf7fc46`) using native iOS Swift 6.0 (`UIKit` + `SwiftUI`) without importing desktop `gpui` widgets or Bazel/AsyncDisplayKit forks.
- **`IOS-MAIN-AUTH-03` (Single Unified iOS Application)**: All capabilities ship inside the single native iOS app target `Fabushi` (`com.ombhrum.fabushi`). Introducing a second app target, a stripped human-only shell, or an external runtime checkout is prohibited.
- **`IOS-MAIN-AUTH-04` (Historical Provenance Non-Promotion)**: Prior `bhrumom/fabushi-desktop` snapshots (`0ecf1326`, `d575b4f7`), PR #20 (`798cf51d`), Grok branches, and unmerged PR #11 (`b2c4b95e`) are historical research only; their artifacts and row statuses never satisfy current-main acceptance.
- **`IOS-MAIN-AUTH-05` (Zero Stub / Zero Facade Integrity)**: Every Host FFI namespace (`source/host/`) and Swift service (`mobile/ios/Fabushi/Core/HostBridge/`) must execute genuine Rust logic backed by `source/desktop_core/`. Hardcoded `sample_chats`, wildcard `"status": "acknowledged"` fallbacks, test deletions, `#[ignore]`, and `XCTSkip` workarounds are strictly forbidden.
- **`IOS-MAIN-AUTH-06` (GitHub-Actions-Only Execution)**: Per `AGENTS.md`, all executable builds, tests, linters, validators, and archives execute exclusively in GitHub Actions (`ubuntu-24.04` and `macos-15` Xcode 16.4) or a user-authorized remote runtime, never in local assistant containers.
- **`IOS-MAIN-AUTH-07` (Live Rebaseline Discipline)**: Whenever `bhrum/gpui-telegram@main` advances, `manifests/desktop-main-authority.json`, `manifests/desktop-main-reference-index.json`, `manifests/desktop-main/*.json`, `docs/parity/desktop-main*.json`, and `source/desktop_core/` must be rebaselined to exact `commit` and `rootTreeSha`, fencing modified/added `crates/**` blobs with `requires-current-main-review`.
- **`IOS-MAIN-AUTH-08` (Honest Gate Accounting)**: Missing signing secrets, App Store Connect credentials, or physical-device runners remain `pending`/`blocked` rather than fabricated as `passed`.

---

## 2. Source Inventory & Layered iOS Ownership (`IOS-MAIN-INV-01..07`)

- **`IOS-MAIN-INV-01` (Selected Source Roots)**: Selected upstream source roots in `bhrum/gpui-telegram@main` are `["crates/**"]`, partitioned into 14 per-crate manifest and parity-ledger chunks (`crates-fabushi_miniapp`, `crates-fabushi_miniapp_wasm`, `crates-gpui_telegram_agent`, `crates-gpui_telegram_cli`, `crates-gpui_telegram_client`, `crates-gpui_telegram_common`, `crates-gpui_telegram_core`, `crates-gpui_telegram_desktop`, `crates-gpui_telegram_media`, `crates-gpui_telegram_proto`, `crates-gpui_telegram_server`, `crates-gpui_telegram_test_harness`, `crates-gpui_telegram_webrtc`, `crates-magpie_gateway`).
- **`IOS-MAIN-INV-02` (Full Tracked Tree Accounting)**: Every tracked Git entry outside `crates/**` (`inherited/**`, `tests/**`, `scripts/**`, `.github/**`, `vendor/**`, workspace manifests) is recorded with exact mode/type/SHA/size in `manifests/desktop-main/tracked-outside-selected.json` and SHA-256 tree digest (`fullTreeSha256`, `selectedTreeSha256`).
- **`IOS-MAIN-INV-03` (Embedded Pure-Rust Core — `source/desktop_core/`)**: Directly embeds UI-agnostic Rust crates (`gpui_telegram_core`, `gpui_telegram_proto`, `gpui_telegram_common`, `gpui_telegram_client`, `gpui_telegram_media`, `gpui_telegram_webrtc`, `gpui_telegram_agent`, `gpui_telegram_cli`, `gpui_telegram_server`, `gpui_telegram_test_harness`, `fabushi_miniapp`, `magpie_gateway`, plus required `inherited/*` and `vendor/xattr-0.2.3`) with zero `gpui`, zero C++ `tdlib`, and zero `openssl` dependencies (`rustls` + `webpki-roots` + bundled `rusqlite`).
- **`IOS-MAIN-INV-04` (Five Mandatory iOS Sandbox Adaptations)**:
  1. Writable sandbox directory resolution (`FABUSHI_APP_SUPPORT_DIR` -> `GPUI_TELEGRAM_CONFIG_DIR` -> `Application Support/Fabushi`) and canonical default cloud server `http://34.46.77.133:8080` (`FABUSHI_SERVER_URL` override).
  2. `ScenePhase` background suspension safety (`config.lifecycle_background` stops WS/Outbox/Tailnet sockets, checkpoints SQLite WAL, and releases POSIX file locks to prevent `0xdead10cc`; `config.lifecycle_foreground` reconnects WS, flushes Outbox, and runs PTS `sync.get_difference`).
  3. Native iOS `Security.framework` Keychain & `LocalAuthentication` (`LAContext` Face ID / Touch ID) bridges for session credentials, passcode lock, and TON Wallet mnemonics (with isolated sandbox fallback in unsigned Simulator UI-test mode).
  4. User-space controller-mode `TailnetEngine` (`TSWG` ChaCha20-Poly1305 UDP + WebSocket DERP without kernel `utun` or local `/bin/sh` subprocess execution on `target_os = "ios"`).
  5. `Info.plist` App Transport Security (`NSAllowsArbitraryLoads = true`, `NSAllowsLocalNetworking = true` for `http://34.46.77.133:8080` and user-configured custom servers) and custom URL schemes (`fabushi`, `gpui-telegram`, `tg`, `ton`, `magpie`).
- **`IOS-MAIN-INV-05` (C-ABI Host Runtime — `source/host/`)**: Compiles `libmahayana_app_host.a` (`staticlib` for `aarch64-apple-ios` & `aarch64-apple-ios-sim`, plus `cdylib` on Linux CI for `ctypes` stress verification) exporting panic-isolated (`catch_unwind`) C-ABI symbols (`telegram_app_host_*`, `telegram_bridge_register_event_callback`, `telegram_replay_window_*`, `telegram_proto_max_packet_size`, and backward-compatible `mahayana_*` symbols).
- **`IOS-MAIN-INV-06` (Canonical Native Swift 6.0 Presentation Owners)**:
  - `mobile/ios/Fabushi/Core/`: `HostBridge/` (`FabushiHostClientProtocol`, `FabushiHostClient` actor, `HostError`, `HostRealtimeEvent`), `Security/` (`KeychainStore`, `BiometricsAuthenticator`), `Localization/` (`LocalizationManager` `zh-CN`/`en-US`), `Navigation/` (`AppRouterProtocol`, `AppRouter`), `SemanticTree/` (`SemanticWorkspaceTree` SHA-256 exporter).
  - `mobile/ios/Fabushi/DesignSystem/`: `TelegramThemeEngine` (4 canonical themes + 7 peer avatar gradients), `TelegramTypography` (7-step `14..26pt` scale + `31pt` composer floor), `TelegramBubbleGeometry` (`16pt/8pt/6pt` Bezier tail + Bubble-Free mode + 1/2/3-dash stripes), `LiquidGlassComponents` (`GlassBackgroundView`, `LiquidLensView`, `50pt` `EdgeEffectView`, `PeerAvatarView`, `26pt/11pt` cards), `TelegramOverlays` (`SheetContainer`, `GlassAlertPresenter`, `UndoToastBanner`).
  - `mobile/ios/Fabushi/App/`: `FabushiAppEntry`, `TelegramRootSplitViewController` (`>680pt` `.doubleColumn` split + hardware keyboard shortcuts), `FabushiRootShellView` (`62pt` Liquid Glass / `49pt` classic tab bar, lifecycle & deep-link wiring).
  - `mobile/ios/Fabushi/Features/`: Domain modules across Milestones M2–M5 (`Auth`, `ChatList`, `Stories`, `Contacts`, `Conversation`, `Composer`, `Media`, `PeerInfo`, `Settings`, `Calls`, `Wallet`, `HermesStudio`, `Magpie`, `MiniApps`, `Computers`).
- **`IOS-MAIN-INV-07` (Desktop-Only Source Disposition)**: Platform-specific desktop windowing/daemon files (`crates/gpui_telegram_desktop/src/platform/{linux,mac,win}/**`, GPUI widget trees, `crates/gpui_telegram_cli` host CLI entrypoint, `crates/fabushi_miniapp_wasm` browser glue) use `not-applicable-with-replacement` or `ios-adapted` mapped to their concrete Swift/Rust iOS replacements.

---

## 3. Parity Ledger & Anti-Facade Contract (`IOS-MAIN-LEDGER-01..08`)

- **`IOS-MAIN-LEDGER-01`**: `docs/parity/desktop-main-index.json` and its 14 `docs/parity/desktop-main/crates-*.json` chunks must have 1:1 path and Git blob SHA parity with `manifests/desktop-main-reference-index.json`.
- **`IOS-MAIN-LEDGER-02`**: Every row records `desktop_path`, `desktop_blob_sha`, `desktop_responsibility`, `desktop_owner`, `desktop_visible_effect`, `ios_disposition`, `ios_target_path`, `ios_language`, `ios_platform_delta`, `implementation_status`, `authority_review_state`, and `desktop_review_commit`.
- **`IOS-MAIN-LEDGER-03`**: Rows changed or added in `docs/parity/desktop-main-impact.json` (`directChanges`) must remain review-fenced (`requires-current-main-review`) and cannot be marked `implemented` or `verified` until current-main owner review and exact-HEAD CI evidence close.
- **`IOS-MAIN-LEDGER-04`**: Unknown RPC methods dispatched to `source/host/` MUST fail closed with structured `HostError` code `"unknown_command"` (`ok: false`), never `"status": "acknowledged"`.
- **`IOS-MAIN-LEDGER-05`**: All 29 canonical RPC domain namespaces (`config.*`, `auth.*`, `accounts.*`, `sync.*`, `chats.*`/`dialogs.*`, `folders.*`, `stories.*`, `search.*`, `messages.*`, `drafts.*`, `outbox.*`, `e2ee.*`, `media.*`, `stickers.*`, `reactions.*`, `polls.*`, `peers.*`/`groups.*`/`forums.*`, `contacts.*`, `calls.*`, `talk.*`, `settings.*`, `wallet.*`, `stars.*`, `alipay.*`, `hermes.*`, `actions.*`/`acp.*`, `magpie.*`, `bots.*`, `miniapps.*`, `fabushi_miniapp.*`/`global_dharma.*`, `computers.*`, `tailnet.*`, `semantic.*`) dispatch to genuine Rust client/protocol/storage implementations.
- **`IOS-MAIN-LEDGER-06`**: Background WebSocket frames (`WsServerFrame`), `ClientSyncCoordinator` PTS gap detection (`sync_get_difference`), and `OutboxWorker` delivery transitions push real-time events across `telegram_bridge_register_event_callback` into Swift `AsyncStream<HostRealtimeEvent>`.
- **`IOS-MAIN-LEDGER-07`**: No stale root `TEST_READY.md` or `@main` executable test shim inside `FabushiTests` is permitted.
- **`IOS-MAIN-LEDGER-08`**: Existing tracked files (`SearchIndexingBridge.swift`, `XCTestShim.swift`, historical Specs, legacy suites) are never deleted to silence build failures.

---

## 4. Complete 57-Feature Parity Matrix (`F01..F57`)

| ID | Feature | Milestone | Authoritative Owner |
|---|---|---|---|
| `F01` | Username & Password Login / Registration (`auto_welcome`) | M2 | `source/host` (`auth.*`) + `Features/Auth` |
| `F02` | Alipay QR OAuth, Mobile Callback (`fabushi://`, `gpui-telegram://`) & SDK Login | M2 | `source/host` (`auth.alipay_*`) + `Features/Auth` |
| `F03` | Email OTP Verification & Registration | M2 | `source/host` (`auth.email_*`) + `Features/Auth` |
| `F04` | Phone OTP Login (`send-code` & `verify`) | M2 | `source/host` (`auth.phone_*`) + `Features/Auth` |
| `F05` | Two-Factor Authentication (Cloud Password / SRP-6a) | M2 | `gpui_telegram_core`/`source/host` (`auth.*2fa*`) + `Features/Auth` |
| `F06` | QR Code Multi-Device Pairing (`pair/*`) | M2 | `source/host` (`auth.qr_pair_*`) + `Features/Auth` |
| `F07` | WebAuthn / P-256 Passkeys | M4 | `source/host` (`auth.*passkey*`) + `Features/Settings` |
| `F08` | Custom Server URL (`http://34.46.77.133:8080` default) & HTTP/SOCKS5 Proxy | M2 | `source/host` (`config.*`) + `Features/Auth` |
| `F09` | Startup Session Hydration, Logout & Session Revocation | M2 | `Core/HostBridge` + `source/host` (`auth.*`) |
| `F10` | Multi-Account Switcher (Up to 3 / 6 Premium Accounts) | M2 | `source/host` (`accounts.*`) + `App/` & `Features/Auth` |
| `F11` | Adaptive Split/Tab Root Shell (`>680pt` `.doubleColumn`, `62pt` Liquid Glass / `49pt` classic Tab Bar) | M1 | `App/TelegramRootSplitViewController.swift`, `App/FabushiRootShellView.swift` |
| `F12` | 4-Mode Telegram Theme Engine (`Day Classic`, `Day`, `Night #000000`, `Night Tinted #18222D`) & 7 Peer Gradients | M1 | `DesignSystem/TelegramThemeEngine.swift` |
| `F13` | Bilingual Localization (`zh-CN` / `en-US`) & Live Runtime Toggle (`Cmd+L`) | M1 | `Core/Localization/LocalizationManager.swift` + `source/host` (`i18n`) |
| `F14` | Local Passcode App Lock, Face ID (`LAContext`) & Idle Auto-Lock | M4 | `Core/Security` + `Features/Settings` |
| `F15` | `SemanticWorkspaceTree` SHA-256 State Hash & Programmatic UI Inspection | M1 | `Core/SemanticTree/SemanticWorkspaceTree.swift` + `source/host` (`semantic.*`) |
| `F16` | In-App Release Checker (`AboutTelegram` / GitHub Releases Semver) | M4 | `source/host` (`updater`) + `Features/Settings` |
| `F17` | Virtualized `ChatListItem` (`76pt` row, `60pt` avatar at `x=16pt`, text `x=86pt`, `20pt` badges) | M2 | `Features/ChatList` + `source/host` (`chats.*`) |
| `F18` | Chat Folders (`/api/v1/folders`) & Collapsible Archived Chats Drawer | M2 | `Features/ChatList` + `source/host` (`folders.*`, `chats.*`) |
| `F19` | Bidirectional `24pt` Swipe Actions, Edit Mode & Live `Peek` Preview | M2 | `Features/ChatList` |
| `F20` | Collapsible `36pt` Search Bar, Scope Tabs & Tantivy Full-Text Search | M2 | `Features/ChatList` + `source/host` (`search.*`) |
| `F21` | Expandable `83pt` Stories Strip & Fullscreen Stories Carousel Viewer | M2 | `Features/Stories` + `source/host` (`stories.*`) |
| `F22` | New Private Chat, Group, Forum, Channel, Invite & Companion Bot Modals | M2 | `Features/ChatList` + `source/host` (`chats.*`, `peers.*`, `bots.*`) |
| `F23` | `180°`-Inverted Transcript, Classic Bubbles (`16pt/8pt/6pt`) & Bubble-Free Page Mode | M3 | `Features/Conversation` + `DesignSystem/TelegramBubbleGeometry.swift` |
| `F24` | UTF-16 Rich Text Entities, Particle Spoilers, 1/2/3-Dash Quotes & `Menlo` Code Copy | M3 | `Features/Conversation` + `source/host` (`parse_markdown_spans`) |
| `F25` | Media Bubbles, Mosaic Albums, `GalleryController` & HTTP 206 Range Streaming | M3 | `Features/Media` + `source/host` (`media.*`) |
| `F26` | Voice Notes (5-Bit Waveform), `240pt` Round Video Notes & `1x/1.5x/2x` Mini-Player | M3 | `Features/Media` + `gpui_telegram_media` |
| `F27` | Interactive Polls, Quizzes, `TodoCard`, Locations, Stars Invoice & Generative UI | M3 | `Features/Conversation` + `source/host` (`polls.*`) |
| `F28` | Replies, Quotes, Edits, Multi-Chat Forwards, Deletes & Batch Selection | M3 | `Features/Conversation` + `source/host` (`messages.*`) |
| `F29` | `46pt` Pinned Message Cycling Banner & Per-Chat Auto-Delete TTL | M3 | `Features/Conversation` + `source/host` (`messages.*`, `chats.*`) |
| `F30` | `30pt` Reaction Capsules, Star Gold Particles & `PortalView` 7-Reaction Context Menu | M3 | `Features/Conversation` + `source/host` (`messages.*`) |
| `F31` | In-Chat Search, `InstantView` Reader, Message Translation & Report | M3 | `Features/Conversation` + `source/host` (`search.*`, `hermes.*`) |
| `F32` | PTS Gap-Healing Sync (`ClientSyncCoordinator`) & Durable SQLite `OutboxWorker` | M1 | `source/desktop_core` + `source/host` (`sync.*`, `outbox.*`) |
| `F33` | Signal-Grade Multi-Device E2EE (X3DH + Double Ratchet + Group `SenderKeyState`) | M1 | `source/desktop_core` + `source/host` (`e2ee.*`) |
| `F34` | Glass Composer (`40pt` 3-line AI morph), CJK Pinyin `markedText` Safety, Drafts & Article Editor | M3 | `Features/Composer` + `source/host` (`ime_machine`, `drafts.*`) |
| `F35` | `AttachmentContainer` (`24.88%` sheet), `MediaEditor`, 4 GiB SHA-256 Chunked Upload & Send Options | M3 | `Features/Composer` + `source/host` (`media.*`) |
| `F36` | Voice/Video Hold-Slide-Lock Orb, `EntityKeyboard` (`Stickers/Emoji/GIFs`) & `/slash`/`@` Popup | M3 | `Features/Composer` + `source/host` (`voice_gesture`, `stickers.*`) |
| `F37` | Stretchy `104pt` `PeerInfoScreen`, Glass Platters, Shared Media Tabs, Members, Stats & Export | M4 | `Features/PeerInfo` + `Features/Contacts` + `source/host` (`peers.*`, `contacts.*`) |
| `F38` | 1:1 & Group WebRTC Calls (`CallKit`/`AVAudioSession`), 4-Emoji SHA-256 & `<60ms` VAD Talk Mode | M4 | `Features/Calls` + `FabushiBroadcastUpload` + `source/host` (`calls.*`, `talk.*`) |
| `F39` | 26-Section Settings Suite (`26pt` Glass Cards), Storage Pie Chart, Sessions & APNs | M4 | `Features/Settings` + `source/host` (`settings.*`) |
| `F40` | TON 24-Word BIP39 Wallet (`UQ`/`EQ`), Telegram Stars & Alipay VIP / Redeem Codes | M4 | `Features/Wallet` + `source/host` (`wallet.*`, `stars.*`, `alipay.*`) |
| `F41` | Hermes 7-Tab Agent Studio, 54 `/slash` Commands & Platform Action Bus (`/api/v1/actions`) | M5 | `Features/HermesStudio` + `source/host` (`hermes.*`, `actions.*`) |
| `F42` | Magpie Multi-Provider AI Gateway, Circuit-Breaker Routing, PII Masking & `magpie://` Import | M5 | `Features/Magpie` + `source/host` (`magpie.*`) |
| `F43` | BotFather, Companion Bots & Embedded `全球法布施` 3D Globe/Zen Mini App (`WKWebView` + `fabushi_miniapp`) | M5 | `Features/MiniApps` + `source/host` (`bots.*`, `miniapps.*`, `fabushi_miniapp.*`) |
| `F44` | Tailnet Encrypted P2P/DERP Mesh (`TSWG`) & Remote `ComputerWorkbench` + Fenced Leases | M5 | `Features/Computers` + `source/host` (`computers.*`, `tailnet.*`) |
| `F45` | Liquid Glass (`GlassBackgroundView`), `LiquidLensView` & `50pt` `EdgeEffectView` | M1 | `DesignSystem/LiquidGlassComponents.swift` |
| `F46` | 7-Step `PresentationFontSize` (`14..26pt`, `31pt` composer floor) & Corner/Tail Geometry | M1 | `DesignSystem/TelegramTypography.swift`, `DesignSystem/TelegramBubbleGeometry.swift` |
| `F47` | Telegram-iOS `AuthorizationSequence` Motion, Auto-Advancing OTP Shake & 2FA Monkey Peek | M2 | `Features/Auth` |
| `F48` | 60/120fps `StreamingTextReveal` & `InstantPageV2` Rich Markdown Bubbles (`stableId`) | M3 | `Features/Conversation` |
| `F49` | `33pt` Circular Swipe-to-Reply Gesture (`absX > 2 && absY * 2 < absX`) & Medium Haptic | M3 | `Features/Conversation` |
| `F50` | First-Party Agent Composer (`34pt` Model/Effort Chips, `300x61pt` Menu & `300x113pt` Slider) | M3 | `Features/Composer` |
| `F51` | Profile `AgentConnectScreen` (`API KEY` / `INVALID API KEY` Validation + Toast) | M5 | `Features/PeerInfo` + `Features/Magpie` |
| `F52` | `SheetContainer` (`40%` dim), `GlassAlertPresenter` (`1.15->1.0` spring) & `UndoToastBanner` (`#474747` `5s`) | M1 | `DesignSystem/TelegramOverlays.swift` |
| `F53` | Complete Non-Stub 29-Namespace Host FFI & 5 iOS Sandbox Bridges | M1 | `source/host/**` + `Core/HostBridge/**` |
| `F54` | Spec-First Governance Lock (`AGENTS.md`, Canonical Spec & `manifests/desktop-main-authority.json`) | M1 | `AGENTS.md`, `docs/**`, `manifests/**`, `scripts/check-desktop-main-ios-architecture.py` |
| `F55` | Staged 4-Job GitHub Actions CI (`ubuntu-24.04` + `macos-15` Xcode 16.4, `75m` timeouts) | M1 | `.github/workflows/gpui-telegram-ios-ci.yml`, `scripts/build-mahayana-ios.sh` |
| `F56` | Opaque-Box 4-Tier E2E Parity Suite (`>=659` Cases) & Tier 5 White-Box Adversarial Hardening | M6 | `tests/e2e_ios_parity/**`, `mobile/ios/FabushiUITests/**` |
| `F57` | Brand-New `v2.0.0` GitHub Release (`SHA256SUMS`) & `ios-store-delivery.yml` Pipeline | M6 | `mobile/ios/project.yml`, `.github/workflows/ios-store-delivery.yml` |

---

## 5. Staged CI & Release Verification Gates (`IOS-MAIN-GATE-01..10`)

- **`IOS-MAIN-GATE-01` (Authority & Source-Tree Gate — `ubuntu-24.04`)**: Verifies exact `HEAD` SHA, runs `tests/test_desktop_main_authority.py`, clones pinned `bhrum/gpui-telegram` commit, runs `check-desktop-main-ios-architecture.py --strict --desktop-checkout`, and re-verifies live `refs/heads/main` at job end.
- **`IOS-MAIN-GATE-02` (Rust Core, Loopback Server & C-ABI Stress Gate — `ubuntu-24.04`)**: Executes `source/desktop_core` workspace tests (`gpui_telegram_*`, `fabushi_miniapp`, `magpie_gateway`), `source/host` unit & loopback-server integration tests across all 29 namespaces, and `tests/adversarial_ffi/test_ffi_stress.py` (`ctypes` panic isolation, NULL pointers, malformed UTF-8/JSON, `unknown_command`, 2,000 concurrent RPCs).
- **`IOS-MAIN-GATE-03` (iOS Staticlib & Loopback Server Artifact Gate — `macos-15`)**: Cross-compiles `libmahayana_app_host.a` for `aarch64-apple-ios` and `aarch64-apple-ios-sim` (`IPHONEOS_DEPLOYMENT_TARGET=17.0`, `-Cembed-bitcode=no`), verifies exported C-ABI symbols via `nm -gU`, builds `gpui_telegram_server`, and publishes `SHA256SUMS.txt`-verified artifacts.
- **`IOS-MAIN-GATE-04` (Xcode 16.4 Swift 6.0 Simulator Unit & Smoke UI Gate — `macos-15`, `timeout-minutes: 75`)**: Reuses prebuilt `libmahayana_app_host.a` (`FABUSHI_USE_PREBUILT_RUST=1`), runs `xcodegen`, boots `iPhone 16 Pro` Simulator alongside loopback `gpui_telegram_server`, compiles `Fabushi`, `FabushiBroadcastUpload`, `FabushiTests`, and `FabushiUITests` under `SWIFT_VERSION=6.0` + `SWIFT_STRICT_CONCURRENCY=complete`, and runs `CoreFFITests`, `Telegram*Tests`, and `FabushiUITests` smoke verification.
- **`IOS-MAIN-GATE-05` (Physical-Device Release Archive Gate — `macos-15`, `timeout-minutes: 75`)**: Reuses prebuilt `iphoneos` `libmahayana_app_host.a` and builds an unsigned Release `.xcarchive` (`generic/platform=iOS`) with `SHA256SUMS.txt`.
- **`IOS-MAIN-GATE-06` (Opaque-Box 4-Tier E2E Gate)**: Runs `scripts/run_ios_e2e_suite.sh --tier all` (`>= 659` tests across Tiers 1–4).
- **`IOS-MAIN-GATE-07` (Lifecycle, Offline Outbox & PTS Recovery Gate)**: Exercises background WAL checkpoint/lock release, reconnect Outbox flush, and `sync.get_difference` gap healing.
- **`IOS-MAIN-GATE-08` (Security & E2EE Gate)**: Verifies Keychain/Biometrics isolation, X3DH + Double Ratchet + Sender Keys tamper rejection, and Mini App `initData` HMAC-SHA256 validation.
- **`IOS-MAIN-GATE-09` (Tier 5 Adversarial & Soak Gate)**: White-box challenger fault injection, concurrency races, CJK/UTF-16 boundaries, and memory/FD soak stability.
- **`IOS-MAIN-GATE-10` (Signed App Store / TestFlight & GitHub Release Gate)**: `v2.0.0` GitHub Release with `SHA256SUMS` and `ios-store-delivery.yml` execution when Apple signing secrets are provisioned.

---

## 6. Normative Completion Criteria (`IOS-MAIN-AC-01..21`)

1. **`IOS-MAIN-AC-01`**: `AGENTS.md`, `README.md`, `MIGRATION_SOURCE.md`, and this Spec consistently bind to live `bhrum/gpui-telegram@main` and `manifests/desktop-main-authority.json`.
2. **`IOS-MAIN-AC-02`**: Live `bhrum/gpui-telegram@main` SHA matches `manifests/desktop-main-authority.json` at both the start and end of every required GitHub Actions gate.
3. **`IOS-MAIN-AC-03`**: Full tracked upstream tree (`11,237` entries) and selected `crates/**` inventory (`2,958` blobs across 14 crates) match exact SHA-256 digests (`fullTreeSha256`, `selectedTreeSha256`).
4. **`IOS-MAIN-AC-04`**: Every `crates/**` blob has an explicit parity-ledger row with non-fabricated status and current-main review fencing on changed blobs.
5. **`IOS-MAIN-AC-05`**: `source/desktop_core/` compiles and passes workspace tests on `ubuntu-24.04` and cross-compiles cleanly to `aarch64-apple-ios` and `aarch64-apple-ios-sim` with zero `gpui`/`tdlib`/`openssl` dependencies.
6. **`IOS-MAIN-AC-06`**: `source/host/` implements all 29 domain namespaces and real-time push events with zero `sample_chats` or `"acknowledged"` facades and passes C-ABI panic/concurrency stress tests.
7. **`IOS-MAIN-AC-07`**: Native Swift 6.0 `DesignSystem/`, `Core/`, and `App/` deliver the 4-mode `TelegramThemeEngine`, 7-step `PresentationFontSize` (`31pt` composer floor), Liquid Glass chrome, `UISplitViewController` (`>680pt`) + floating capsule Tab Bar, bilingual `zh-CN`/`en-US` toggle, and `SemanticWorkspaceTree` SHA-256 exporter.
8. **`IOS-MAIN-AC-08`**: Multi-method Auth (`F01..F10, F47`) including Password, Alipay QR/callback, Email OTP, Phone OTP, SRP 2FA, QR pairing, Passkeys, Proxy, Session hydration, and Multi-Account switching are verified end-to-end.
9. **`IOS-MAIN-AC-09`**: Native `ChatList`, Folders, Archive drawer, `24pt` bidirectional swipes, `Peek` preview, Tantivy search, Stories strip/viewer, and Chat/Group/Forum/Channel/Bot creation (`F17..F22`) are verified end-to-end.
10. **`IOS-MAIN-AC-10`**: `180°`-inverted Conversation transcript supporting Classic Bubbles & Bubble-Free Page mode, all 12 message cards, UTF-16 entities/spoilers/quotes/code copy, `StreamingTextReveal`/`InstantPageV2`, `30pt` reactions, `PortalView` menu, and `33pt` swipe-to-reply (`F23..F31, F48, F49`) are verified end-to-end.
11. **`IOS-MAIN-AC-11`**: PTS gap-healing `ClientSyncCoordinator` and crash-recoverable SQLite `OutboxWorker` (`F32`) preserve zero-loss ordered delivery across disconnects and restarts.
12. **`IOS-MAIN-AC-12`**: Multi-device X3DH, Double Ratchet, and Group Sender Keys E2EE (`F33`) encrypt, decrypt, and reject tampered ciphertexts deterministically.
13. **`IOS-MAIN-AC-13`**: Glass Composer (`40pt` 3-line AI morph), CJK Pinyin `markedText` safety, Rich Article Editor, `AttachmentContainer`, `MediaEditor`, 4 GiB SHA-256 chunked upload, Hold-Slide-Lock Voice/Video orb, `EntityKeyboard`, and 2-row Agent Model/Effort toolbar (`F34..F36, F50`) are verified end-to-end.
14. **`IOS-MAIN-AC-14`**: Stretchy-hero `PeerInfoScreen`, Contacts `A–Z`, Shared Media tabs, Members/Invites/Topics/Stats/Export, and `AgentConnectScreen` (`F37, F51`) are verified end-to-end.
15. **`IOS-MAIN-AC-15`**: 1:1 & Group WebRTC Calls (`CallKit`/`AVAudioSession`), ReplayKit broadcast extension, 4-emoji SHA-256 verification badge, and `<60ms` VAD barge-in AI Talk Mode (`F38`) are verified end-to-end.
16. **`IOS-MAIN-AC-16`**: All 26 `Settings` sections (`26pt` glass cards), Passcode/Face ID lock, Storage pie chart/clear, Active Sessions, APNs push registration, and GitHub Release updater (`F14, F16, F39`) are verified end-to-end.
17. **`IOS-MAIN-AC-17`**: TON 24-word BIP39 Wallet (`UQ`/`EQ`), Telegram Stars balance/topup/gifts, and Alipay VIP/redeem codes (`F40`) are verified end-to-end.
18. **`IOS-MAIN-AC-18`**: Hermes 7-tab Studio, 54 `/slash` commands, Action Bus approvals, Magpie AI Gateway + PII masking, BotFather, Mini Apps Center, and embedded `全球法布施` 3D Globe/Zen Mini App (`F41..F43`) are verified end-to-end.
19. **`IOS-MAIN-AC-19`**: User-space Tailnet encrypted P2P/DERP mesh, remote `ComputerWorkbench` screen/terminal control, and fenced lease/emergency takeover (`F44`) are verified end-to-end.
20. **`IOS-MAIN-AC-20`**: All 4 Opaque-Box E2E Tiers (`>=659` tests) and Tier 5 white-box adversarial suites (`F56`) pass 100% on exact `HEAD` in GitHub Actions with zero skipped/ignored tests.
21. **`IOS-MAIN-AC-21`**: Unsigned physical-device Release `.xcarchive`, `v2.0.0` GitHub Release (`SHA256SUMS`), and `ios-store-delivery.yml` signed archive/TestFlight evidence (`F57`, or explicit credential-blocker proof if secrets are absent) are recorded and independently audited on final `main` `HEAD`.

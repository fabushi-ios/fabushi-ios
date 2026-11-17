# GPUI-Telegram Desktop main -> Native Telegram iOS Product Parity — Specification

- **Status**: `active`
- **Spec ID**: `IOS-GPUI-TELEGRAM-MAIN-001`
- **Amended From**: `docs/specs/fabushi-desktop-main-ios-parity.md` (per `docs/specs/spec-first-ai-development.md:28` and explicit user mandate in `ORIGINAL_REQUEST.md`)
- **Last updated**: `2026-11-17`
- **Live upstream authority (`Desktop main`)**: `bhrum/gpui-telegram` branch `main` @ `c40494379cd03f71a2c1f57bae5f85d677cb0e3e` (`rootTreeSha`: `905b781897938c7d044fe4ea47508ad224fda0dd`, release line `v1.2.0` / `0.1.5`)
- **Machine authority**: `manifests/desktop-main-authority.json`
- **Target repository & branch**: `fabushi-ios/fabushi-ios` (`feat/gpui-telegram-ios-parity` -> `main`)

---

## 1. Context, Problem & Durable Spec Amendment Record

The explicit user requirement (`ORIGINAL_REQUEST.md`) mandates porting `https://github.com/bhrum/gpui-telegram` to native iOS with 1:1 Telegram iOS product, visual, tactile, and performance parity ("把这个做成ios版本，实现和telegram ios版本一样的效果").

Per `docs/specs/spec-first-ai-development.md:28` ("The latest explicit user requirement may amend an older Spec, but update the durable Spec/decision record before implementation intentionally diverges from it") and `AGENTS.md`:
1. Prior `bhrumom/fabushi-desktop` (`0ecf132681845231b597ece11bc5f979a9a8f0cc`) and PR #20 (`798cf51d96cb1cb98cf657af212bb47274ccb701`) Specs remain preserved on disk as historical provenance (`docs/specs/fabushi-desktop-main-ios-parity.md`, `docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md`).
2. Local prototype commit `8128951a71503a6f109ce0cb37618d022fe8e79a` (2026-11-08) vendored an obsolete `v0.1.0` snapshot (`68d422c6`), introduced compile-breaking Xcode targets (`XCTestShim.swift`, `@main` `master_e2e_suite.swift` in `FabushiTests`), left app launch split-brained behind legacy `ProductionRenderer`, and contained mock facades (`SearchIndexingBridge.swift`, `dispatch.rs` catch-all `acknowledged`, hardcoded `sample_chats`, and hardcoded latency constants). All Nov 8 claims (`TEST_READY.md`) are invalidated and removed.
3. This Specification establishes **`bhrum/gpui-telegram` Desktop main (`c40494379cd03f71a2c1f57bae5f85d677cb0e3e`)** as the sole live upstream product/protocol authority and locks all 11,225 tracked upstream files (`2,948` `crates/**` blobs + `8,277` non-selected blobs) in `manifests/desktop-main-authority.json`.

---

## 2. Goal

Deliver a single, unified, standalone native iOS application (`Fabushi`) in `fabushi-ios/fabushi-ios` that achieves:
1. **1:1 Functional & Protocol Parity** with `bhrum/gpui-telegram@c40494379cd03f71a2c1f57bae5f85d677cb0e3e` across all 44 features (`F01`–`F44`), sharing the pure-Rust `v1.2.0` client/core/proto/media/agent/miniapp/magpie crates (`source/desktop_core/`) over a real C-ABI FFI host (`source/host/`) speaking HTTPS REST (`/api/v1/*`) and JSON WebSockets (`/api/v1/ws`) to `gpui_telegram_server`.
2. **1:1 Native Telegram iOS UX Parity** across iPhone and iPad using Swift 6, SwiftUI, UIKit (`UICollectionView`, `UITextView`, `UITabBar`, `UIScrollView`), `LiveKitWebRTC`, `CallKit`, `PushKit`, `ReplayKit`, `AVFoundation`, `WKWebView`, `ASAuthorizationController`, `LocalAuthentication`, and Keychain.
3. **100% Authentic GitHub Actions Verification** (`ubuntu-24.04` + `macos-15`) with zero container builds/tests and zero stubs or mock facades.

---

## 3. Non-Goals / Out of Scope

1. Connecting to public Telegram MTProto servers (`149.154.*.*`) or linking TDLib/`grammers` — upstream `gpui-telegram` speaks HTTPS REST + JSON WebSockets (`/api/v1/ws`) against `gpui_telegram_server` (default cloud `http://104.197.143.175:8080`, or loopback `127.0.0.1:<port>` in CI).
2. Compiling Zed `gpui` desktop windowing (`crates/gpui_telegram_desktop`) or desktop audio/WebRTC C stacks (`cpal`, `symphonia`, `webrtc-rs`, `rubato`, `hound`) onto iOS targets.
3. Creating a second iOS app target, external cross-repo path/git runtime dependency, or macOS `.app` binary self-updater on iOS.
4. Executing `cargo`, `rustc`, `swiftc`, `xcodebuild`, `xcodegen`, or Python/Node test runners inside the Linux assistant container.

---

## 4. Product Requirements (`F01`–`F44`) & Upstream Shipping vs. Planned Audit (`AGENTS.md` #4)

### 4.1 Complete 44-Feature Scope (`F01`–`F44`)

| ID | Feature | Normative Requirement | Target Milestone |
|---|---|---|---|
| `F01` | Spec-First Authority & CI Pipeline | Active Spec, `c4049437` authority lock, validator, `source/desktop_core` `v1.2.0`, C-ABI host, `ubuntu-24.04` + `macos-15` + probe CI | M1 |
| `F02` | Cloud Username & Password Auth | `/api/v1/auth/{login,register}` Argon2id auth, inline validation, Keychain session persistence | M1, M2 |
| `F03` | Alipay QR Login & `fbs_` OAuth | Scannable QR (`EcLevel::Q`), 2s generation-fenced polling (`/api/v1/auth/alipay/{start,poll}`), expiry refresh, deep link | M1, M2 |
| `F04` | Email OTP Registration | 6-digit email verification with 60s resend cooldown (`/api/v1/auth/email/{send-code,register}`) | M1, M2 |
| `F05` | Editable Cloud Server URL Bar | Pre-login & settings endpoint editor with live `/health` dot, `server_config.json` persistence, clean cache reset | M1, M2 |
| `F06` | Phone + Country Picker OTP & QR Pairing | Searchable country picker (`+888` Fragment `FT`), formatted phone pad, `.oneTimeCode` OTP (`/api/v1/auth/phone/*`), QR device pairing (`/api/v1/auth/pair/*`) | M1, M2 |
| `F07` | Cloud 2FA Password & WebAuthn Passkeys | `/api/v1/auth/2fa*` verify/setup/recovery + `ASAuthorizationController` Passkey login & CRUD (`/api/v1/auth/passkey/*`) | M1, M2, M5 |
| `F08` | Session Hydration, Active Devices & Revocation | Startup `/api/v1/auth/me` validation, WS `401` auto-logout & wipe, active sessions list & remote revoke (`/api/v1/auth/sessions*`) | M1, M2, M5 |
| `F09` | Keychain Multi-Account & Passcode/FaceID | Up to 3/6 Keychain account slots (`AccountScopeManager` + generation fence) & blocking PIN/FaceID lock + App Switcher blur | M1, M2, M5 |
| `F10` | Telegram Design System, 4 Themes & i18n | `Night`, `Tinted`, `Day`, `Classic` palettes, 8 accents, 7 wallpapers, 7 peer gradients, `.tdesktop-theme`, Dynamic Type, live `zh-CN`/`en` | M2, M5 |
| `F11` | Adaptive iPhone 4-Tab & iPad Split Shell | iPhone translucent 4-tab `UITabBar` (`Contacts`, `Calls`, `Chats`, `Settings`) + swipe-back; iPad 2/3-col `NavigationSplitView` + `UIKeyCommand` | M2 |
| `F12` | Virtualized Dialog Rows, Swipe & Context Menu | 76pt dialog rows (54pt avatar, online, verified/mute, ticks, red `Draft:`, typing, media thumb, `OPEN` pill, `999+`), swipes, blurred preview, `Edit` | M1, M2 |
| `F13` | Chat Folders, Archive & Forum Topics | Synced `/api/v1/folders` strip + unread pills, pull-to-reveal collapsible Archive, Forum Topics (`/api/v1/chats/{id}/topics`) | M1, M2, M4 |
| `F14` | Universal 10-Category Search & Filter Chips | `Chats`, `Channels`, `Apps`, `Posts`, `Media`, `Downloads`, `Links`, `Files`, `Music`, `Voice` + 6 filter chips + Recent Peers + `/api/v1/search` | M1, M2 |
| `F15` | Stories Strip & Fullscreen Story Viewer | Pull-down horizontal Stories ring carousel + `StoriesViewer` (create, view tracking, caption, DM reply, delete via `/api/v1/stories*`) | M1, M2 |
| `F16` | Quick `+` Compose Menu | New DM, custom Fabushi AI Bot (`/api/v1/fabushi-bots`), Group (optional Forum), or Channel (`/api/v1/chats`) | M1, M2 |
| `F17` | Conversation Header, Search & Pinned Banner | Peer title + live online/typing/member/Thinking subtitle, avatar -> Profile, Call, In-Chat Search, More menu, cycling `PinnedMessagesBar` | M1, M3 |
| `F18` | Inverted 120Hz `UICollectionView` Transcript | Inverted `UICollectionView` + `RowHeightCache`, `.interactive` keyboard dismiss, top pagination, 900s grouping & Bezier tails, sticky dates, FABs | M3 |
| `F19` | Optimistic Durable Outbox & PTS Sync | SQLite `OutboxWorker` offline queue, `client_nonce` -> `server_id` reconciliation, monotonic `Sending -> Sent -> Read`, PTS `get_difference` | M1, M3 |
| `F20` | UTF-16 Rich Text Entities, Code & Spoilers | Exact UTF-16 entity slicing (Bold, Italic, Underline, Strike, `Code`/`Pre` Copy, collapsible `Blockquote`, `Spoiler`, URLs, Mentions, `/commands`) | M3 |
| `F21` | Photos, Videos, Mosaic Albums & `MediaViewer` | Aspect-clamped bubbles, spoilers, 2–10 Mosaic Albums (`+N`), HTTP 206 streaming, `MediaViewer` (`0.5x..3.0x` zoom/pan/rotate) & `MediaEditor` | M1, M3 |
| `F22` | Documents/Files & Chunked CAS Transfers | SHA-256 multipart upload (`init/part/status/complete`) & download progress cards + `QLPreviewController` QuickLook & Share Sheet | M1, M3 |
| `F23` | 100-Bar 5-Bit Voice & Circular Video Notes | Real `VoicePlaybackController` with interactive 100-bar 5-bit waveform seek, `1x/1.5x/2x` speed, proximity earpiece routing, circular `VideoNote` | M1, M3 |
| `F24` | Stickers, Animated Stickers, Dice & GIFs | Static/animated stickers (Power Saving aware), `/api/v1/stickers/*` browse/install, animated dice (`🎲🎯🏀⚽🎰🎳`), GIF search/autoplay | M1, M3 |
| `F25` | Polls, Quizzes, Todo, Invoice, Contact & Map | Live-voting Polls/Quizzes (confetti + explanation + close), Todo checklists, Stars `InvoiceCard`, `ContactsUI` card, `MapKit` Location card | M1, M3 |
| `F26` | Swipe-Reply, Quotes, Forwards, Edits & Deletes | 44pt haptic swipe-reply, substring `quote_text`, multi-forward w/ caption, edit (`edited`), delete (local/`for_everyone`), batch selection | M1, M3 |
| `F27` | Blurred Reaction Portal (`QuickStrip` + Grid) | Long-press blurred context portal with 7 quick reactions, 32-emoji 8-col grid, viewer-highlighted pills, `pending_nonce_reactions` | M1, M3 |
| `F28` | Inline Bot Keyboards, AI Cards & Instant View | `InlineKeyboardButton` callbacks/URLs/MiniApps, streaming AI `ThinkingBubble` & `ToolPermissionDock`, `Translate`, `IvViewer` Instant View | M1, M3 |
| `F29` | Per-Chat & Global Auto-Delete TTL Reaper | Configurable chat/global TTL (`Off`/`1d`/`1w`/`1mo`) + 2s reaper purging expired rows from SQLite & live transcript | M1, M3, M5 |
| `F30` | Multi-Line `UITextView` & `ImeSafetyMachine` | Auto-growing `36..120pt` `UITextView` enforcing `markedTextRange` + `just_committed` CJK IME safety fence | M3 |
| `F31` | Formatting Menu, Reply Strip, Silent/Schedule | Selection formatting menu, Reply/Edit/Forward banner, long-press Send (`Send Without Sound` & `Schedule Message`), `Send-As` picker | M1, M3 |
| `F32` | Attachment Sheet, Sticker Keyboard & Bot Menu | `PHPicker`/Camera gallery, File, `MapKit` Location, Poll creator, Contact, Mini-Apps drawer, Emoji/Sticker/GIF keyboard, Bot Menu button | M1, M3 |
| `F33` | Voice/Video Hold-to-Record FSM | 16kHz/AAC `AVAudioRecorder` + 6-state `VoiceGestureMachine` (`<0.5s` discard, `<-80pt` cancel, `<-50pt` lock, live 5-bit waveform) + Video-Circle toggle | M1, M3 |
| `F34` | Cloud Draft Sync & 61 `/slash` Autocomplete | Debounced local+server `PUT /draft`, 61 Hermes/Bot `/command` popup, `@mention`/`#tag`/`:emoji:` completion, `@file/@dir/@git/@url` badges | M1, M3 |
| `F35` | Stretchy Peer Profile, Admin, Stats & Export | Parallax avatar header, 5 shared media tabs, Group/Channel `Owner`/`Admin` roles, Invite Links, Forum CRUD, Boosts/Charts, JSON/HTML export | M1, M4 |
| `F36` | Contacts Tab, `@username` Resolve & Blocking | Synced Contacts (`A–Z`, online sort), add/delete contact, `/peers/resolve`, user/bot Block/Unblock | M1, M4, M5 |
| `F37` | WebRTC Calls, Screen Share & AI Talk Mode | `CallsTabView` (`All | Missed`), `LiveKitWebRTC` + `CallKit` + `PushKit` + `ReplayKit`, 4-emoji key, 1–5 rating, 60ms VAD AI Talk Mode orb | M1, M4 |
| `F38` | Signal-Style Multi-Device E2EE & Secret Chats | X3DH prekeys, Double Ratchet 1:1 DMs, Group Sender Keys, 4-emoji + 12×12 verification UI, device revoke (`/api/v1/e2ee/*`), APNs/PushKit | M1, M4 |
| `F39` | Complete 26-Section Settings, Storage & Proxy | All 26 `SettingsSectionType` routes: Profile, Notifications, Privacy, Sessions, Passcode, 2FA, Passkeys, TTL, Themes, Storage, Proxy, Language | M1, M5 |
| `F40` | Telegram Stars Wallet, Gifts, Alipay & Redeem | `/api/v1/wallet/*` Stars balance/ledger, top-up, Star Gifts (send/view/craft), Alipay orders, Premium redeem codes | M1, M5 |
| `F41` | Hermes 7-Tab Agent Studio & Magpie Gateway | `Tasks` Kanban, `Skills`, `Memory & Soul`, `MCP Tools` (28 toolsets), `Cron`, `Sessions` (FTS5/branch/rollback), `Approvals`, Magpie LLM routing | M1, M5 |
| `F42` | BotFather NL Creator, Mini Apps & `全球法布施` | `/botfather/nl-create`, Mini Apps catalog, ToS modal, `WKWebView` `Telegram.WebApp` JS bridge + CloudStorage, embedded `全球法布施` Mini App | M1, M5 |
| `F43` | Remote Computer Workbench & Tailnet/DERP | Live base64 desktop viewer with touch mouse/keyboard, remote shell, `32 MB` file browser, `AskEveryTime` approvals, Tailnet/DERP mesh status | M1, M5 |
| `F44` | Platform Version Check & Update Banner | iOS-native release metadata check (`config.check_release_update`) displaying non-intrusive TestFlight / App Store update banner | M1, M5 |

### 4.2 Forensic Distinction Between Upstream Shipping vs. Partially-Wired / Simulated Subsystems (`AGENTS.md` #4)

1. **41 Fully Shipping End-to-End Upstream Features**: Ship with complete GPUI Desktop UI, `TelegramClient` SDK methods, and `gpui_telegram_server` routes at `c4049437`.
2. **2 Partially-Wired Desktop UI / Complete Backend+SDK Features (Implemented End-to-End on iOS)**:
   - `F06` Phone OTP & QR Device Pairing: `gpui_telegram_server` and `TelegramClient` ship full `/api/v1/auth/phone/*` and `/api/v1/auth/pair/*` endpoints while Desktop `AuthWizardView` partially stubbed those wizard steps; iOS implements complete native UI wired to `auth.phone_*` and `auth.qr_pair_*`.
   - `F09` Multi-Account Switcher: Desktop `MainMenuDrawer` rendered multi-account UI over a single `session.json`; iOS implements genuine multi-slot Keychain session isolation (`AccountScopeManager` + `auth.switch_account_slot` + `GenerationFence`).
3. **2 Simulated / Client-Only Upstream Modules (Preserved Truthfully Without Fabricated Server Routes)**:
   - `gpui_telegram_core::mtproto`: In-memory packet framing and 128-bit replay window simulation (live wire transport is HTTPS REST + `/api/v1/ws`).
   - `gpui_telegram_core::passport`: Client-side form validation and local encryption structures with no `/api/v1/passport` server endpoints.

---

## 5. Current State (`fabushi-ios@8128951` Baseline Audit)

At pre-rebaseline commit `8128951a71503a6f109ce0cb37618d022fe8e79a`:
1. `source/desktop_core/` tracked stale `v0.1.0` crates lacking E2EE (`X3DH`/Double-Ratchet/Sender-Keys), `fabushi_miniapp`, `magpie_gateway`, `TailnetEngine`, `HermesClient`, and `v1.2.0` protocol DTOs.
2. `mobile/ios/FabushiTests/Support/XCTestShim.swift` shadowed `XCTest.XCTestCase`, preventing real XCTest discovery; `mobile/ios/project.yml` compiled `@main` `tests/e2e/master_e2e_suite.swift` into `FabushiTests`; `ChatListRowMetrics` was declared twice; and 7 Swift 6.1 / Xcode 16.4 strict-concurrency errors (`isolated deinit`, nonisolated `deinit` task reads, missing `import Combine`, `UIScreen.main.scale`, `UITextViewDelegate`) blocked `macos-15` Simulator compilation.
3. `mobile/ios/Fabushi/Bridge/SearchIndexingBridge.swift` mocked SQLite WAL and Tantivy in a Swift dictionary; `source/host/src/dispatch.rs` returned hardcoded `sample_chats` and wildcard `{"status": "acknowledged"}`; and `source/host/src/events.rs` used a non-nullable function pointer for `TelegramBridgeEventCallback` (UB on `NULL` unregister).

---

## 6. Target Native Telegram iOS UX State

1. **Unified Launch**: App startup initializes `TelegramBridgeRuntime.shared`, checks Keychain session hydration (`auth.get_me`), and routes directly into `TelegramAuthFlowView` (unauthenticated) or `TelegramRootShellView` (authenticated), gated by Passcode/FaceID lock (`F09`).
2. **Adaptive iPhone & iPad Chrome**: iPhone renders a translucent 49pt 4-tab `UITabBar` (`Contacts`, `Calls`, `Chats`, `Settings` with long-press Account Switcher) and full-width interactive pop gesture; iPad renders a 2/3-column `NavigationSplitView` with `UIKeyCommand` shortcuts (`⌘K`, `⌘N`, `⌘1..4`, `Esc`).
3. **120Hz Transcript & IME-Safe Composer**: Inverted `UICollectionView` with `RowHeightCache`, `.interactive` keyboard dismissal, 900s bubble clustering, haptic swipe-to-reply, blurred `QuickStrip` + 8-column reaction portal, `UITextView` `ImeSafetyMachine` (`markedTextRange` + `just_committed` CJK guard), and 6-state `VoiceGestureMachine`.

---

## 7. Architecture, Authority Protocol (`IOS-MAIN-AUTH-01..08`), Inventory (`IOS-MAIN-INV-01..07`) & Ledger Policy (`IOS-MAIN-LEDGER-01..08`)

### 7.1 Authority Protocol (`IOS-MAIN-AUTH-01`..`IOS-MAIN-AUTH-08`)
- `IOS-MAIN-AUTH-01`: Live upstream authority is `bhrum/gpui-telegram` branch `main` (`Desktop main`).
- `IOS-MAIN-AUTH-02`: Machine lock `manifests/desktop-main-authority.json` pins exact 40-hex `commit` (`c40494379cd03f71a2c1f57bae5f85d677cb0e3e`) and `rootTreeSha` (`905b781897938c7d044fe4ea47508ad224fda0dd`).
- `IOS-MAIN-AUTH-03`: `scripts/check-desktop-main-ios-architecture.py` queries live `git ls-remote https://github.com/bhrum/gpui-telegram.git refs/heads/main` at the start and end of CI gates and fails closed on drift.
- `IOS-MAIN-AUTH-04`: `IOS_EXPECTED_HEAD` binds CI checkout verification to the exact iOS commit under test.
- `IOS-MAIN-AUTH-05`: Historical `bhrumom/fabushi-desktop`, PR #20, and Nov 8 `8128951` artifacts cannot satisfy current-main gates.
- `IOS-MAIN-AUTH-06`: Upstream shipping vs. partially-wired/simulated status is recorded truthfully without fabrication.
- `IOS-MAIN-AUTH-07`: All build/runtime inputs reside inside `fabushi-ios/fabushi-ios` (`source/desktop_core`, `source/host`, `mobile/ios`); external cross-repo path/git dependencies are forbidden.
- `IOS-MAIN-AUTH-08`: No single-app boundary violation (no second app target, no Electron/GPUI desktop runtime on iOS).

### 7.2 Inventory Rules (`IOS-MAIN-INV-01`..`IOS-MAIN-INV-07`)
- `IOS-MAIN-INV-01`: Full tracked upstream tree (`11,225` non-`tree` entries, `0` gitlinks) is hashed via SHA-256 `tree_digest` (`bc11c21d0af807ca30aa8eb6baf49f4a82d51e507c402eb8c4b1cc0a5c29e29e`).
- `IOS-MAIN-INV-02`: Selected product/crate roots are `["crates/**"]` (`2,948` blobs across 13 crates, `selectedTreeSha256`: `e43d00a9ca9293bd94d22add3bdcd669f473e3cb13b3dc4c840a949868c89327`).
- `IOS-MAIN-INV-03`: All 13 crates (`fabushi_miniapp`, `fabushi_miniapp_wasm`, `gpui_telegram_agent`, `gpui_telegram_client`, `gpui_telegram_common`, `gpui_telegram_core`, `gpui_telegram_desktop`, `gpui_telegram_media`, `gpui_telegram_proto`, `gpui_telegram_server`, `gpui_telegram_test_harness`, `gpui_telegram_webrtc`, `magpie_gateway`) are indexed in `manifests/desktop-main-reference-index.json`.
- `IOS-MAIN-INV-04`: All `8,277` tracked items outside `crates/**` are recorded in `manifests/desktop-main/tracked-outside-selected.json` with exact `(mode, type, sha, size)` identities.
- `IOS-MAIN-INV-05`: `set(real_tree) == set(selected_manifests) | set(nonselected_register)` with zero overlap and zero missing paths.
- `IOS-MAIN-INV-06`: Direct/indirect upstream drift impacts are tracked in `docs/parity/desktop-main-impact.json`.
- `IOS-MAIN-INV-07`: `--strict --desktop-checkout` verifies every blob SHA and byte size against an actual pinned `bhrum/gpui-telegram` checkout.

### 7.3 Parity Ledger Policy (`IOS-MAIN-LEDGER-01`..`IOS-MAIN-LEDGER-08`)
- `IOS-MAIN-LEDGER-01`: `docs/parity/desktop-main-index.json` and its 13 `docs/parity/desktop-main/crates-*.json` chunks account 1:1 for all `2,948` `crates/**` blobs.
- `IOS-MAIN-LEDGER-02`: Every row specifies `desktop_owner`, `desktop_responsibility`, `desktop_visible_effect`, `ios_disposition`, `implementation_status`, `ios_target_path`, and `ios_platform_delta`.
- `IOS-MAIN-LEDGER-03`: Allowed dispositions are `direct-port`, `ios-adapted`, and `not-applicable-with-replacement` (restricted to the 65 desktop OS window/tray files under `crates/gpui_telegram_desktop/src/platform/{linux,mac,win}/**`).
- `IOS-MAIN-LEDGER-04`: Initial rebaseline status is `mapped` (`2,883` rows) or `not-applicable` (`65` rows); **zero** rows may be marked `verified` without exact-HEAD CI proof.
- `IOS-MAIN-LEDGER-05`: Promoting any row to `implemented` requires the real `ios_target_path` file on disk and non-empty `production_evidence`.
- `IOS-MAIN-LEDGER-06`: Promoting any row to `verified` under `--complete` requires `responsibility_id`, `ios_target_symbol`, `production_entrypoint`, `oracle_ids`, `ios_target_commit == IOS_EXPECTED_HEAD`, `workflow_run`, `run_attempt`, `job`, `artifact_id`, `artifact_digest`, and `independent_reviewer`.
- `IOS-MAIN-LEDGER-07`: Anti-facade static checks in `verify_ios_owners` fail closed if `SearchIndexingBridge.swift`, stale `TEST_READY.md`, `dispatch.rs` `acknowledged` catch-alls, hardcoded `sample_chats`, or fake benchmark constants reappear.
- `IOS-MAIN-LEDGER-08`: Final `--complete` mode fails closed until `baselineReady`, `dependenciesReviewed`, `completenessAccepted`, `acceptanceAccepted`, and all 21 `IOS-MAIN-AC-01..21` gates are `passed`.

---

## 8. Interfaces, C-ABI FFI Contracts, Launch Environment & Accessibility Identifiers

### 8.1 C-ABI Header (`source/host/include/mahayana_app_host.h`)
- `void *telegram_app_host_create(const char *app_data_dir, const char *storage_passphrase);`
- `void *telegram_app_host_create_test(const char *app_data_dir);`
- `const char *telegram_app_host_last_error(void);`
- `char *telegram_app_host_dispatch(void *host, const char *request_json);`
- `char *telegram_app_host_dispatch_with_handle(void *host, const char *request_json);`
- `void telegram_bridge_register_event_callback(void *host, TelegramBridgeEventCallback callback, void *user_context);` (`callback == NULL` unregisters cleanly via `Option<unsafe extern "C" fn(...)>` without UB; `telegram_app_host_destroy` clears callback & aborts observer tasks before drop).
- `void telegram_app_host_free_string(char *pointer);`
- `void telegram_app_host_destroy(void *host);`
- Replay window exports: `telegram_replay_window_create`, `telegram_replay_window_check_and_add`, `telegram_replay_window_destroy`, `telegram_proto_max_packet_size`.

### 8.2 JSON-RPC Wire Envelope & 10 Command Namespaces
- Success: `{"ok": true, "result": <T>}`
- Error: `{"ok": false, "error": {"code": "<typed_code>", "message": "<human_readable>", "http_status": <u16|null>}}`
- Unrecognized commands MUST fail closed with `{"ok": false, "error": {"code": "unknown_command", ...}}`. Wildcard `{"status": "acknowledged"}` is strictly forbidden.
- Namespaces (`PROJECT.md:146-155`): `config.*` (6), `auth.*` (25), `dialogs.*` (24), `messages.*` (20), `media.*` (13), `peers.*` (17), `calls.*` (10), `e2ee.*` (8), `wallet.*` (7), `agent.*` (24), plus 22 `ClientDomainEvent` push notifications delivered through `NSLock`-guarded multi-subscriber `TelegramEventStream`.

### 8.3 Launch Environment & Accessibility Identifier Contracts
- Launch env overrides (`ProcessInfo.processInfo.environment`): `GPUI_TELEGRAM_SERVER_URL` (default `http://104.197.143.175:8080`, CI loopback `http://127.0.0.1:18080`), `GPUI_TELEGRAM_STORAGE_DIR`, `GPUI_TELEGRAM_SIMULATE_OFFLINE`, `GPUI_TELEGRAM_LOCALE`.
- Stable `.accessibilityIdentifier(...)` strings across `auth.*`, `shell.*`, `chats.*`, `stage.*`, `mediaViewer.*`, `composer.*`, `attach.*`, `stickerPanel.*`, `profile.*`, `groupAdmin.*`, `contacts.*`, `calls.*`, `callOverlay.*`, `talkMode.*`, `settings.*`, `wallet.*`, `hermes.*`, `miniapp.*`, and `computer.*` (`PROJECT.md:168-174`).

---

## 9. Constraints & Non-Functional Requirements

1. **Toolchains**: Swift 6.0 language mode (`SWIFT_STRICT_CONCURRENCY: complete`) compatible with Xcode 16.4 (Swift 6.1) on GitHub Actions `macos-15`; Rust 2024 (`1.85+`) cross-compiling `staticlib` for `aarch64-apple-ios` and `aarch64-apple-ios-sim` (`IPHONEOS_DEPLOYMENT_TARGET=17.0`).
2. **iOS TLS & Target Gating**: `reqwest` and `tokio-tungstenite` enable `rustls-tls-webpki-roots` so `https://` and `wss://` succeed inside the iOS sandbox; `gpui_telegram_webrtc` gates `webrtc-rs`/`rubato`/`hound` on `#[cfg(not(target_os = "ios"))]` while compiling `WebRtcCallEngine`, `CallsEmojiFingerprint`, `CallSession`, `SignalingFrame`, `DefaultAiMediaTap`, and `opus_codec` on iOS; `gpui_telegram_test_harness` gates GPUI headless tests behind `desktop-headless`.
3. **App Transport Security**: `Info.plist` enables `NSAllowsLocalNetworking: true` and `NSAllowsArbitraryLoads: true` so configurable `http://` endpoints (`http://104.197.143.175:8080` and loopback `http://127.0.0.1:18080`) operate across `URLSession`, `AVPlayer`, and `WKWebView`.
4. **Disk & Build Hygiene**: Lean Cargo profiles (`debug = 0`, `strip = "debuginfo"`, `incremental = false`), single `target/` directory, `scripts/clean_build_garbage.py`, and zero `/tmp` usage in assistant workflows.

---

## 10. Failure Modes & Edge Cases

1. **CJK IME Candidate Return Fence**: Pressing Return while `UITextView.markedTextRange != nil` or within the `just_committed` cycle confirms CJK composition and MUST NOT send the message.
2. **WS-vs-HTTP `client_nonce` Race**: Optimistic bubble transitions monotonically `Sending -> Sent -> Read` without duplicate bubbles when WS `new_message` arrives before HTTP response.
3. **Offline Outbox Durability**: Queued messages survive app termination in SQLite WAL and flush in FIFO order upon reconnect.
4. **PTS Sequence Gap Recovery**: Out-of-order WS `pts` triggers `messages.sync_get_difference` without dropping intermediate updates.
5. **UTF-16 ZWJ Emoji Entity Slicing**: `MessageEntity` `(offset, length)` uses UTF-16 code units so CJK + ZWJ family emoji formatting never panics or misaligns.
6. **Voice Gesture Thresholds**: `< 0.5s` tap discards cleanly; `< -80pt` horizontal drag cancels; `< -50pt` vertical drag locks hands-free recording; empty waveform falls back to uniform minimal bars without division-by-zero.
7. **Startup `401` Session Expiry**: Revoked/expired token wipes local session and transitions cleanly to `TelegramAuthFlowView` without crash loops.
8. **`+888` Fragment Numbers**: Country picker resolves `+888` to Fragment (`FT`) anonymous numbers.
9. **Multi-Agent Loop Budget**: `@mention` bot handoffs stop deterministically at the 5-hop `LoopBudget`.
10. **Zero-Duration Call Hangup**: Calls ending with `duration_secs == 0` skip the 1–5 star rating modal.
11. **Mosaic Album `>4` & Spoiler Two-Tap**: 5–10 item albums render a 4-tile mosaic with `+N` counter; spoiler media requires first tap to unblur and second tap to open `MediaViewer`.
12. **FFI Callback Teardown Safety**: `telegram_app_host_destroy` atomically unregisters callbacks and aborts background tasks before freeing `TelegramAppHost` (zero UAF), and `catch_unwind` isolates Rust panics into `HOST_PANIC` JSON errors.

---

## 11. Implementation Strategy (`M1` -> `M6` + Parallel `E2E`)

- **`M1` (`M1.1` + `M1.2`)**: Spec-first `c4049437` rebaseline, `source/desktop_core` `v1.2.0` sync, Xcode 16.4 compile unblock & facade removal, `gpui-telegram-ios-ci.yml` + `gpui-telegram-probe.yml` (`M1.1`); full 10-namespace ~154-RPC `source/host/` C-ABI runtime + 22 push events + `TelegramBridgeFFITests` against loopback `gpui_telegram_server` (`M1.2`).
- **`M2`**: Unified `FabushiSceneRoot` launch, `TelegramAuthFlowView` (`F02`–`F08`), Keychain multi-account & Passcode/FaceID (`F09`), 4-theme Design System & i18n (`F10`), adaptive iPhone/iPad Shell (`F11`), Chats/Folders/Archive/Search/Stories/`+` Compose (`F12`–`F16`).
- **`M3`**: Inverted 120Hz `UICollectionView` transcript, Outbox/PTS sync, UTF-16 entities, 12+ rich cards, `MediaViewer`/`MediaEditor`, Reaction portal, TTL reaper (`F17`–`F29`), and `UITextView` `ImeSafetyMachine` + `VoiceRecorder` + Attachment/Sticker/Drafts/`/slash` composer (`F30`–`F34`).
- **`M4`**: Stretchy Peer Profile, Group/Channel Admin & Forum CRUD & Stats/Export (`F35`), Contacts (`F36`), `LiveKitWebRTC` + `CallKit` + `ReplayKit` + 60ms VAD AI Talk Mode (`F37`), and X3DH/Double-Ratchet/Sender-Keys E2EE (`F38`).
- **`M5`**: 26-section Settings (`F39`), Stars Wallet/Gifts/Alipay/Redeem (`F40`), Hermes 7-tab Studio & Magpie (`F41`), BotFather + `WKWebView` Mini Apps + `全球法布施` (`F42`), Remote Computer Workbench + Tailnet/DERP (`F43`), and iOS Version Update Banner (`F44`).
- **`E2E` & `M6`**: Parallel opaque-box 4-tier XCUITest suite (`>= 506` assertions against live loopback `gpui_telegram_server`), sequential Tier 1 -> 4 green certification (Phase 1), and Tier-5 white-box adversarial hardening (Phase 2).

---

## 12. Verification & Fail-Closed Gates (`IOS-MAIN-GATE-01..10`)

- `IOS-MAIN-GATE-01`: Start-of-job and end-of-job live `bhrum/gpui-telegram@main` SHA & `IOS_EXPECTED_HEAD` check.
- `IOS-MAIN-GATE-02`: `python3 scripts/check-desktop-main-ios-architecture.py --strict --desktop-checkout desktop-main-source` + `test_desktop_main_authority.py` on `ubuntu-24.04`.
- `IOS-MAIN-GATE-03`: `cargo test --manifest-path source/desktop_core/Cargo.toml --workspace` on `ubuntu-24.04`.
- `IOS-MAIN-GATE-04`: `cargo test --manifest-path source/host/Cargo.toml` on `ubuntu-24.04`.
- `IOS-MAIN-GATE-05`: `scripts/build-mahayana-ios.sh all` (`aarch64-apple-ios` + `aarch64-apple-ios-sim`) with `nm -gU` C-ABI symbol check on `macos-15`.
- `IOS-MAIN-GATE-06`: `xcodegen generate` + Simulator `xcodebuild test -only-testing:FabushiTests` against live loopback `gpui_telegram_server` on `macos-15`.
- `IOS-MAIN-GATE-07`: Unsigned physical-device compilation & archive (`xcodebuild archive -destination 'generic/platform=iOS'`) with SHA-256 artifact digest on `macos-15`.
- `IOS-MAIN-GATE-08`: Opaque-box 4-tier Simulator XCUITest suite (`FabushiUITests/Parity`, Tiers 1–4 `>= 506` cases) against loopback `gpui_telegram_server` on `macos-15`.
- `IOS-MAIN-GATE-09`: Tier-5 adversarial white-box probe workflow (`gpui-telegram-probe.yml`) on `ubuntu-24.04` and `macos-15`.
- `IOS-MAIN-GATE-10`: Signed archive, physical device install/upgrade, and TestFlight/App Store Connect delivery (`ios-store-delivery.yml`, marked `blocked` if signing secrets/hardware are absent per `AGENTS.md` #8).

---

## 13. Acceptance Criteria / Release Definition of Done (`IOS-MAIN-AC-01..21`)

- `IOS-MAIN-AC-01`: Live `bhrum/gpui-telegram@main` SHA matches `manifests/desktop-main-authority.json` at gate start and end, and `fullTreeSha256` / `selectedTreeSha256` match the pinned Git tree.
- `IOS-MAIN-AC-02`: All `2,948` `crates/**` blobs and `8,277` non-selected tracked items are accounted with zero unreviewed drift.
- `IOS-MAIN-AC-03`: Standalone iOS build inputs (`source/desktop_core`, `source/host`, `mobile/ios`) compile without external cross-repo dependencies and with zero mock facades (`SearchIndexingBridge`, `acknowledged` catch-all, `sample_chats`).
- `IOS-MAIN-AC-04`: Unified single-app launch (`TelegramBridgeRuntime` -> Auth / Root Shell) operates cleanly with Passcode/FaceID lock and Keychain multi-account generation fencing (`F01`, `F08`, `F09`).
- `IOS-MAIN-AC-05`: All cloud authentication & onboarding flows (`F02`–`F07`: Server URL, Password/Register, Alipay QR, Email OTP, Phone+Country OTP, QR Pairing, 2FA, Passkeys) pass end-to-end against `gpui_telegram_server`.
- `IOS-MAIN-AC-06`: Adaptive iPhone 4-tab `UITabBar` + full-width swipe-back, iPad `NavigationSplitView` + `UIKeyCommand`, 4 Telegram themes, `.tdesktop-theme`, and live `zh-CN`/`en` switching (`F10`, `F11`) pass verification.
- `IOS-MAIN-AC-07`: Virtualized 76pt chat list, swipe actions, blurred context menu, Folders, Archive, 10-category Tantivy Search, Stories strip/viewer, and `+` Compose (`F12`–`F16`) pass verification.
- `IOS-MAIN-AC-08`: Inverted 120Hz `UICollectionView` transcript, `RowHeightCache`, 900s grouping, header/pinned banner, and durable SQLite WAL Outbox + `client_nonce` + PTS gap recovery (`F17`–`F19`) survive offline/restart faults.
- `IOS-MAIN-AC-09`: UTF-16 entities, Photos/Videos/Mosaic Albums/HTTP 206 `MediaViewer`/`MediaEditor`, chunked CAS Documents, 100-bar 5-bit Voice & Video Notes, Stickers/Dice/GIFs, Polls/Quizzes/Todo/Invoice/Contact/Map, Replies/Forwards/Edits/Deletes, Reaction Portal, Bot/AI cards/Translate/Instant View, and TTL Reaper (`F20`–`F29`) pass verification.
- `IOS-MAIN-AC-10`: Multi-line `UITextView` CJK `ImeSafetyMachine`, formatting/silent/scheduled/Send-As, `PHPicker`/File/Map/Poll Attachment sheet, Sticker/GIF keyboard, 6-state `VoiceRecorder`, cloud drafts, and 61 `/slash` autocomplete (`F30`–`F34`) pass verification.
- `IOS-MAIN-AC-11`: Stretchy Peer Profile, Group/Channel Roles/Invites/Forum CRUD/Stats/Export, and Contacts/`@username` resolve/Block (`F35`, `F36`) pass verification.
- `IOS-MAIN-AC-12`: `LiveKitWebRTC` + `CallKit` + `PushKit` + `ReplayKit` 1:1/group voice/video calls, 4-emoji key badge, rating, and 60ms VAD AI Talk Mode (`F37`) pass verification.
- `IOS-MAIN-AC-13`: Signal-style X3DH, Double Ratchet, Group Sender Keys, 12×12 + 4-emoji verification UI, device revocation, and push token registration (`F38`) pass cryptographic round-trip verification.
- `IOS-MAIN-AC-14`: All 26 Settings routes, Storage pie/purge, SOCKS5/HTTP Proxy, and iOS release update banner (`F39`, `F44`) pass verification.
- `IOS-MAIN-AC-15`: Telegram Stars Wallet, Star Gifts, Alipay order flow, and redeem codes (`F40`) pass verification.
- `IOS-MAIN-AC-16`: Hermes 7-tab Agent Studio (`Tasks`, `Skills`, `Memory & Soul`, `MCP Tools`, `Cron`, `Sessions`, `Approvals`) and Magpie multi-provider LLM gateway (`F41`) pass verification.
- `IOS-MAIN-AC-17`: BotFather NL creator, `WKWebView` `Telegram.WebApp` Mini Apps, and embedded `全球法布施` Mini App (`F42`) pass verification.
- `IOS-MAIN-AC-18`: Multi-device Remote Computer Workbench and Tailnet/DERP mesh status (`F43`) pass verification with generation-fenced emergency takeover.
- `IOS-MAIN-AC-19`: Unsigned `generic/platform=iOS` release archive succeeds on `macos-15` with verified SHA-256 digest; signed IPA / physical device / TestFlight gates (`IOS-MAIN-GATE-10`) are evidenced or explicitly recorded as `blocked` if signing credentials/devices are unavailable.
- `IOS-MAIN-AC-20`: Full 4-tier XCUITest parity suite (`>= 506` cases) and Tier-5 adversarial hardening probes achieve 100% pass on exact final `HEAD` SHA.
- `IOS-MAIN-AC-21`: Independent Reviewer/Auditor verification confirms exact-HEAD GitHub Actions evidence, artifact SHA-256 digests, and Spec compliance across `docs/parity/desktop-main-release-evidence.json` with zero integrity violations.

---

## 14. Release, Migration & Rollback

1. All implementation lands incrementally on `feat/gpui-telegram-ios-parity` via Draft PR targeting `main`.
2. Switching `server_url` (`F05`) flushes peer/message SQLite tables cleanly while preserving app-level appearance/locale preferences.
3. Rollback is atomic at the Git branch/PR boundary prior to `main` merge, with `manifests/desktop-main-authority.json` and `docs/parity/desktop-main-release-evidence.json` remaining `finalAcceptance: false` until all 21 criteria close.

---

## 15. Observability & Evidence

Every GitHub Actions run records `source_sha`, `source_tree`, `DESKTOP_HEAD_SHA`, `RUN_ID`, `RUN_ATTEMPT`, `.xcresult` bundles, and `SHA256SUMS.txt` digests, which are indexed in `docs/parity/desktop-main-release-evidence.json` and milestone `handoff.md` reports.

---

## 16. References & Provenance

- Original User Request: `/home/antigravity/.agents/teamwork/ORIGINAL_REQUEST.md`
- Live Upstream Repository: `https://github.com/bhrum/gpui-telegram` (`main` @ `c40494379cd03f71a2c1f57bae5f85d677cb0e3e`)
- Governance & Spec Lifecycle: `AGENTS.md`, `docs/specs/spec-first-ai-development.md`, `MIGRATION_SOURCE.md`, `PROJECT.md`
- Historical Specs (Archival Provenance Only): `docs/specs/fabushi-desktop-main-ios-parity.md`, `docs/specs/fabushi-desktop-pr20-ios-architecture-parity.md`

---

## 17. Spec Compliance Record

All requirements (`F01`–`F44`) and acceptance criteria (`IOS-MAIN-AC-01`–`IOS-MAIN-AC-21`) are initialized to `pending` at Spec rebaseline and transition only to `passed`, `blocked` (with explicit external blocker evidence), or `not-applicable` upon exact-HEAD GitHub Actions verification:

| Scope Group | IDs | Current Status | Evidence Location |
|---|---|---|---|
| Foundation, Authority Lock, Rust `v1.2.0` & C-ABI Bridge | `F01`, `IOS-MAIN-AC-01..03` | `pending` | `gpui-telegram-ios-ci.yml`, `ios-desktop-main-architecture.yml` |
| Onboarding, Cloud Auth, Multi-Account, Passcode & Shell/Chats | `F02`–`F16`, `IOS-MAIN-AC-04..07` | `pending` | `docs/parity/desktop-main-release-evidence.json` |
| 120Hz Transcript, Rich Media/Cards, Outbox/PTS & Composer | `F17`–`F34`, `IOS-MAIN-AC-08..10` | `pending` | `docs/parity/desktop-main-release-evidence.json` |
| Profiles, Groups/Forums, Contacts, WebRTC Calls/Talk & E2EE | `F35`–`F38`, `IOS-MAIN-AC-11..13` | `pending` | `docs/parity/desktop-main-release-evidence.json` |
| 26-Route Settings, Wallet, Hermes/Magpie, MiniApps & Computers | `F39`–`F44`, `IOS-MAIN-AC-14..18` | `pending` | `docs/parity/desktop-main-release-evidence.json` |
| Device Archive, 4-Tier E2E + Tier-5 Hardening & Final Audit | `IOS-MAIN-AC-19..21` | `pending` | `docs/parity/desktop-main-release-evidence.json` |

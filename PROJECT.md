# Project: GPUI-Telegram Native iOS (`fabushi-ios`)

- **Upstream Reference**: `https://github.com/bhrum/gpui-telegram` (`main` @ `889246e4621e3e8e58022895a887c63ea3130bdd`, tree `ca25541f1993ed981a409aa1fb24b5c2f56eae89`, `Fabushi Desktop v0.1.5` / release line `v1.2.0`)
- **Target iOS Repository**: `/home/antigravity/fabushi-ios/fabushi-ios` (`fabushi-ios/fabushi-ios`, integration branch `feat/gpui-telegram-ios-parity`, Draft PR #11)
- **Original User Request**: `/home/antigravity/.agents/teamwork/ORIGINAL_REQUEST.md` (`https://github.com/bhrum/gpui-telegram 把这个做成ios版本，实现和telegram ios版本一样的效果`)
- **Execution Policy (`<user_rules>`)**: 100% of executable builds, Rust checks/tests, Python validators, `xcodegen`, `xcodebuild`, XCTest, and XCUITest run **ONLY in GitHub Actions** (`ubuntu-24.04` + `macos-15`), NEVER in the Linux assistant container. Scratch/temp files MUST use `/home/antigravity/.gemini/jetski/scratch/teamwork_tmp/<conversation-id>`, NEVER `/tmp`.

---

## Architecture

### 1. System Overview & Upstream Protocol Ground Truth
`bhrum/gpui-telegram@889246e4` is a self-contained, pure-Rust custom messaging, VoIP, Mini-App, and AI-agent platform (`Fabushi Desktop`) speaking HTTPS REST (`reqwest` + `rustls`) and JSON WebSockets (`/api/v1/ws`, `WsClientFrame` / `WsServerFrame`) with monotonic `pts` sequence sync, SQLite WAL offline cache, durable `client_nonce` outbox, CAS chunked HTTP 206 media streaming, Signal-style X3DH/Double-Ratchet/Sender-Keys E2EE, and self-hosted `/bot/{token}/{method}` Bot API against `gpui_telegram_server` (default cloud `http://104.197.143.175:8080`, or deterministic local loopback `127.0.0.1:<port>` in CI).

To achieve **1:1 functional parity with `bhrum/gpui-telegram@889246e4`** AND **authentic native Telegram iOS UX** ("实现和telegram ios版本一样的效果") inside a **single unified Fabushi/Telegram iOS app** (obeying `AGENTS.md` existing-owner-first / single-app rules and amending the Oct 2026 spec per `docs/specs/spec-first-ai-development.md:28`):

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│                     Native iOS Presentation Layer (Swift 6)                  │
│  • Auth & Onboarding (TelegramAuthFlowView: Server URL, Password/Register,   │
│    Phone+Country OTP, Email OTP, Alipay/Device QR, 2FA, Passkey)             │
│  • Adaptive Root Shell (iPhone 4-Tab UITabBar + Full-Width Swipe-Back;       │
│    iPad 2/3-Column NavigationSplitView + UIKeyCommand; Passcode + Face ID)   │
│  • Chats, Folders, Archive, Stories Strip/Viewer & 10-Tab Global Search      │
│  • Inverted 120Hz UICollectionView Transcript, 900s Clustering, Swipe-Reply, │
│    Blurred QuickStrip/8-Col Reaction Portal, 12+ Rich Cards, MediaViewer     │
│  • UITextView Composer (CJK ImeSafetyMachine, Voice Hold/Cancel/Lock FSM,    │
│    Attachments Sheet, Emoji/Sticker/GIF Panel, 61 Slash + @ Autocomplete)    │
│  • Stretchy Peer Profile, Groups/Forums/Stats, Contacts, 26-Route Settings,  │
│    Stars Wallet, BotFather WKWebView Mini Apps (全球法布施), Hermes 7-Tab     │
│    Agent Studio, Magpie Gateway, Remote Computer Workbench                   │
└───────────────────────────────┬──────────────────────────┬───────────────────┘
                                │ @MainActor Stores        │ Native Apple SDKs
                                ▼                          ▼
┌──────────────────────────────────────────┐  ┌────────────────────────────────┐
│   Swift FFI Bridge (`Fabushi/Bridge/`)   │  │ • LiveKitWebRTC (150.7871.02)  │
│  • TelegramBridgeRuntime (lifecycle/RPC) │  │ • CallKit + PushKit + APNs     │
│  • TelegramEventStream (multi-consumer   │  │ • ReplayKit Broadcast Upload   │
│    NSLock AsyncStream, 22 domain events) │  │ • AVAudioEngine / AVPlayer     │
│  • AccountScopeManager (Keychain slots)  │  │ • Keychain + LocalAuth FaceID  │
│  • MediaTransferManager (chunked CAS)    │  │ • WKWebView (Telegram.WebApp)  │
└───────────────────────────────┬──────────┘  └────────────────────────────────┘
                                │ Handle-based C-ABI (`libmahayana_app_host.a`)
                                ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│        Shared Rust Client Engine (`source/host` + `source/desktop_core`)     │
│  Rebaselined to `bhrum/gpui-telegram@889246e4621e3e8e58022895a887c63ea3130bdd`│
│  • gpui_telegram_{common,core,proto,media,client,agent,webrtc(signaling)}    │
│  • fabushi_miniapp + magpie_gateway                                          │
│  • LocalSqliteStore (WAL) + OutboxWorker + PtsSyncState + E2eeManager        │
└───────────────────────────────┬──────────────────────────────────────────────┘
                                │ HTTPS REST (`/api/v1/*`) + WS (`/api/v1/ws`)
                                ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│  `gpui_telegram_server` (Cloud `104.197.143.175:8080` or macOS CI Loopback)  │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 2. GitHub Actions Verification & Anti-Facade Architecture
- **Zero Container Execution**: All Rust unit/integration tests, architecture lock checks (`scripts/check-desktop-main-ios-architecture.py`), iOS staticlib cross-compiles (`aarch64-apple-ios`, `aarch64-apple-ios-sim`), `xcodegen generate`, `xcodebuild test` (`FabushiTests` + `FabushiUITests` against live loopback `gpui_telegram_server` on `127.0.0.1`), and unsigned `generic/platform=iOS` archives execute exclusively on GitHub Actions (`ubuntu-24.04` + `macos-15`).
- **Zero Facades**: Every stub inherited from commit `8128951` (`dispatch.rs:373` `other => acknowledged`, `SearchIndexingBridge.swift` dictionary mock, `dispatch.rs:223` hardcoded `sample_chats`, `master_e2e_suite.swift` hardcoded latency constants, static Settings/Contacts/Attachment/Sticker placeholders) MUST be deleted and replaced with real Rust-FFI + SQLite WAL + HTTP/WS server implementations.

---

## Feature Inventory

Every feature discovered across Step 0 Survey (`survey_1` 44 upstream features, `survey_3` native iOS UX parity specs, `survey_4` baseline defect remediation, and `spec_miner_delta_889246e4` live `889246e4` authz/streaming/Tier-5 parity refinements) is assigned below (`0` unassigned):

| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| F01 | Spec-First Authority & CI Pipeline | Rebaseline Spec (`docs/specs/gpui-telegram-main-ios-parity.md`), authority lock (`889246e4`), validator, `source/desktop_core` v1.2.0 crates, C-ABI FFI host, & `ubuntu-24.04` + `macos-15` + probe CI workflows | M1 | survey_2, survey_4, delta_889246e4 |
| F02 | Cloud Username & Password Auth | Login & registration (`/api/v1/auth/{login,register}`) with Argon2id verification, inline validation, and Keychain session storage | M1, M2 | survey_1 #1 |
| F03 | Alipay QR Login & `fbs_` OAuth | Scannable QR (`EcLevel::Q`), 2s generation-fenced polling (`/api/v1/auth/alipay/{start,poll}`), expiry refresh, deep-link callback | M1, M2 | survey_1 #2 |
| F04 | Email OTP Registration | 6-digit email verification with 60s resend cooldown (`/api/v1/auth/email/{send-code,register}`) | M1, M2 | survey_1 #3 |
| F05 | Editable Cloud Server URL Bar | Pre-login & settings server endpoint editor with live `/health` indicator dot, `server_config.json` persistence, and clean cache reset | M1, M2 | survey_1 #4 |
| F06 | Phone + Country Picker OTP & QR Device Pairing | Searchable country picker (`+888` Fragment `FT` flag), auto-formatted phone pad, `.oneTimeCode` 5/6-digit OTP boxes (`/api/v1/auth/phone/*`), & QR device pairing (`/api/v1/auth/pair/*`) | M1, M2 | survey_1 #5, survey_3 #2-3,6 |
| F07 | Cloud 2FA Password & WebAuthn Passkeys | 2FA password verification/setup/recovery (`/api/v1/auth/2fa*`) + `ASAuthorizationController` Passkey login & CRUD (`/api/v1/auth/passkey/*`) | M1, M2, M5 | survey_1 #6, survey_3 #5,7 |
| F08 | Session Hydration, Active Devices & Revocation | Startup `/api/v1/auth/me` check, WS `401` auto-logout & cache wipe, active sessions list & remote revocation (`/api/v1/auth/sessions*`) | M1, M2, M5 | survey_1 #7 |
| F09 | Keychain Multi-Account Switcher & Passcode/FaceID Lock | Up to 3/6 Keychain-backed account slots (Settings long-press quick menu + generation fence) & blocking PIN/FaceID lock screen with idle auto-lock & App Switcher blur | M1, M2, M5 | survey_1 #8,38, survey_3 #8-9 |
| F10 | Telegram Design System, 4 Themes & Bilingual i18n | Exact `Night`, `Tinted`, `Day`, `Classic` palettes, 8 accents, 7 wallpapers, 7 peer gradients, `.tdesktop-theme` parser, Dynamic Type/text size/corners, live `zh-CN`/`en` toggle | M2, M5 | survey_1 §1.4, survey_3 §1.2 |
| F11 | Adaptive Native iPhone 4-Tab & iPad Split Shell | iPhone translucent 4-tab `UITabBar` (`Contacts`, `Calls`, `Chats`, `Settings`), inline navbar `Connecting.../Updating...` morph, full-width interactive swipe-back; iPad 2/3-col `NavigationSplitView` + `UIKeyCommand` | M2 | survey_1 #9-10, survey_3 §1.1 |
| F12 | Virtualized Dialog Rows, Swipe & Blurred Context Menu | 76pt dialog rows (54pt avatar, online dot, verified/premium/mute, ticks, red `Draft:`, typing, media thumb, `OPEN` Mini-App pill, `999+` badge), leading/trailing swipes, `UIContextMenuInteraction` live preview, multi-select `Edit` | M1, M2 | survey_1 #11, survey_3 #11 |
| F13 | Chat Folders, Collapsible Archive & Forum Topics | Synced custom folder strip (`/api/v1/folders` CRUD + unread pills), pull-to-reveal/collapsible Archived Chats, and Forum Topics list/sub-threads (`/api/v1/chats/{id}/topics`) | M1, M2, M4 | survey_1 #12, survey_3 #12 |
| F14 | Universal 10-Category Tantivy Search & Filter Chips | `Chats`, `Channels`, `Apps`, `Posts`, `Media`, `Downloads`, `Links`, `Files`, `Music`, `Voice` tabs + 6 filter tags + Recent Peers + sequence-fenced `/api/v1/search` + tap-to-jump | M1, M2 | survey_1 #13, survey_3 #14 |
| F15 | Stories Strip & Fullscreen Story Viewer | Pull-down horizontal Stories ring carousel + fullscreen `StoriesViewer` (create story, view tracking, caption, DM story reply, delete via `/api/v1/stories*`) | M1, M2 | survey_1 #14, survey_3 #13 |
| F16 | Quick `+` Compose Menu (DM / AI Bot / Group / Channel) | Create human contact DM, custom Fabushi AI Bot (persona prompt + geometric/blob avatar via `/api/v1/fabushi-bots`), Group (optional Forum), or Channel (`/api/v1/chats`) | M1, M2 | survey_1 #15 |
| F17 | Conversation Header, In-Chat Search & Pinned Banner | Peer title + live online/typing/member/Thinking subtitle, tappable avatar -> Profile, Call button, In-Chat Search bar (prev/next jump), More menu, cycling `PinnedMessagesBar` (`/pins`) | M1, M3 | survey_1 #16-17, survey_3 #21 |
| F18 | Inverted 120Hz `UICollectionView` Transcript & Grouping | Inverted `UICollectionView` + `RowHeightCache`, `.interactive` drag keyboard dismiss, seamless top pagination, 900s sender grouping & Bezier tails, sticky date pills, Scroll-to-Bottom & `@` FABs | M3 | survey_1 #18, survey_3 #15 |
| F19 | Optimistic Durable Outbox & PTS Sync Reconciliation | SQLite `OutboxWorker` offline queue, `client_nonce` -> `server_id` deduplication (`reconcile_stage_message_list`), monotonic `Sending -> Sent -> Read`, PTS gap `get_difference` recovery | M1, M3 | survey_1 #26, survey_3 #16 |
| F20 | UTF-16 Rich Text Entities, Code Copy, Quotes & Spoilers | Exact UTF-16 entity slicing (Bold, Italic, Underline, Strike, `Code`, `Pre` w/ Copy, collapsible `Blockquote`, particle `Spoiler` reveal, URLs, Mentions, Hashtags, `/commands`, CustomEmoji) | M3 | survey_1 #19, survey_3 #17 |
| F21 | Photos, Videos, Mosaic Albums & Fullscreen `MediaViewer` | Aspect-clamped media bubbles, spoiler blur, 2–10 item Mosaic Albums (`+N`), HTTP 206 streaming, fullscreen `MediaViewer` (`0.5x..3.0x` pinch-zoom, pan, swipe-dismiss, 90° rotate) & `MediaEditor` (crop/spoiler/caption) | M1, M3 | survey_1 #20, survey_3 #18,22 |
| F22 | Documents/Files & Resumable Chunked CAS Transfers | SHA-256 multipart upload (`init/part/status/complete`) & download progress cards + native `QLPreviewController` QuickLook & Share Sheet | M1, M3 | survey_1 #21, survey_3 #20 |
| F23 | 100-Bar 5-Bit Voice Notes & Circular Video Notes | Real audio playback (`VoicePlaybackController`) with interactive 100-bar 5-bit waveform seek, `1x/1.5x/2x` speed pill, proximity earpiece routing, and circular `VideoNote` bubbles | M1, M3 | survey_1 #22, survey_3 #19 |
| F24 | Stickers, Animated Stickers, Dice & GIF Loops | Static/animated sticker rendering (respecting Power Saving), server sticker set browse/install (`/api/v1/stickers/*`), animated dice (`🎲🎯🏀⚽🎰🎳`), and GIF search/autoplay | M1, M3 | survey_1 #23, survey_3 #24 |
| F25 | Interactive Polls, Quizzes, Todo, Invoice, Contact & Map Cards | Live-voting Polls & Quizzes (confetti + 💡 explanation + close poll), interactive Todo checklists, Stars `InvoiceCard`, Contact card (`ContactsUI`), and `MapKit` Location card | M1, M3 | survey_1 #24-25, survey_3 #20 |
| F26 | Swipe-to-Reply, Quotes, Forwards, Edits, Deletes & Multi-Select | 44pt haptic swipe-left-to-reply, substring `quote_text`, multi-message forward w/ caption, edit (`edited` label), delete (local / `for_everyone`), and batch selection bar | M1, M3 | survey_1 #26, survey_3 #21 |
| F27 | Blurred Reaction Portal (`QuickStrip` + 8-Col Grid) | Long-press blurred context portal with 7 quick reactions, expandable 32-emoji 8-col grid, viewer-highlighted reaction pills, and `pending_nonce_reactions` queue | M1, M3 | survey_1 #27, survey_3 #21 |
| F28 | Inline Bot Keyboards, AI Streaming Cards, Translate & Instant View | `InlineKeyboardButton` callbacks/URLs/MiniApps, streaming AI `ThinkingBubble` & `ToolPermissionDock`, message `Translate` sheet, and `IvViewer` Instant View reader | M1, M3 | survey_1 #28, survey_3 #20 |
| F29 | Per-Chat & Global Auto-Delete Message TTL Reaper | Configurable chat/global TTL (`Off`/`1d`/`1w`/`1mo`) + 2s background reaper purging expired rows from SQLite & live transcript | M1, M3, M5 | survey_1 #29 |
| F30 | Multi-Line `UITextView` Composer & CJK `ImeSafetyMachine` | Auto-growing `36..120pt` `UITextView` enforcing `markedTextRange` + `just_committed` CJK IME safety fence so candidate selection never triggers premature send | M3 | survey_1 #30, survey_3 #23 |
| F31 | Formatting Menu, Reply/Edit Strip, Silent/Scheduled & Send-As | Text selection formatting menu, active Reply/Edit/Forward banner, long-press Send (`Send Without Sound` & `Schedule Message`), and `Send-As` peer identity picker | M1, M3 | survey_1 #31, survey_3 #23 |
| F32 | Attachment Half/Full Sheet, Sticker/GIF Keyboard & Bot Menu | Real `PHPicker`/Camera gallery (multi-select, HD, spoiler, caption), File, `MapKit` Location, Poll creator, Contact, Mini-Apps drawer, Emoji/Sticker/GIF keyboard, and Bot Menu button | M1, M3 | survey_1 #32, survey_3 #24 |
| F33 | Voice/Video Hold-to-Record Gesture FSM (`VoiceRecorder`) | Real 16kHz/AAC `AVAudioRecorder` + 6-state `VoiceGestureMachine` (`<0.5s` tap discard, `<-80pt` slide-cancel, `<-50pt` slide-up lock, live 5-bit waveform, preview & send) + Mic/Video-Circle toggle | M1, M3 | survey_1 #33, survey_3 #24 |
| F34 | Cloud Draft Sync & 61 Hermes/Bot `/slash` + `@` Autocomplete | Debounced local+server draft sync (`PUT /draft`), 61 Hermes & Bot `/command` popup, `@mention`/`#tag`/`:emoji:` completion, and `@file/@dir/@git/@url` context badges | M1, M3 | survey_1 #34, survey_3 #23 |
| F35 | Stretchy Peer Profile, Group/Channel Admin, Stats & Export | Parallax avatar header, quick action tiles, `@username` QR, 5 shared media tabs, Group/Channel Member Roles (`Owner`/`Admin`), Invite Links, Forum CRUD, Channel Boosts/Charts, JSON/HTML Export | M1, M4 | survey_1 #35, survey_3 #25 |
| F36 | Contacts Tab, Global `@username` Resolution & Blocking | Synced Contacts tab (`A–Z` index, online sort), add/delete contact, `@username` search (`/peers/resolve`), and user/bot Block/Unblock | M1, M4, M5 | survey_1 #36, survey_3 §1.3 |
| F37 | WebRTC Voice/Video Calls, Screen Share & AI Talk Mode | `CallsTabView` (`All | Missed`), `LiveKitWebRTC` + `CallKit` + `PushKit` + `ReplayKit` screen share, 4-emoji E2EE fingerprint, post-call 1–5 star rating, and 60ms VAD full-duplex **AI Talk Mode** orb | M1, M4 | survey_1 #37, survey_3 #26 |
| F38 | Signal-Style Multi-Device E2EE & Secret Chats | X3DH prekeys, Double Ratchet 1:1 DMs, Group Sender Keys, 4-emoji + 12×12 key verification UI, device revocation (`/api/v1/e2ee/*`), and APNs/PushKit device push registration | M1, M4 | survey_1 #39, survey_3 #25 |
| F39 | Complete 26-Section Settings Router, Storage & Proxy | All 26 `SettingsSectionType` screens: Profile edit, Notifications, Privacy, Sessions, Passcode/FaceID, 2FA, Passkeys, TTL, Themes, Folders, Storage pie/purge, Power Saving, Calls mic test, SOCKS5/HTTP Proxy, Language, Search | M1, M5 | survey_1 #38, survey_3 #27 |
| F40 | Telegram Stars Wallet, Star Gifts, Alipay & Redeem Codes | Stars balance & ledger (`/api/v1/wallet/*`), Stars top-up, peer Star Gifts (send/view/craft), Alipay order flow, and Premium/membership redeem codes | M1, M5 | survey_1 #40 |
| F41 | Hermes 7-Tab AI Agent Studio & Magpie Multi-Provider Gateway | 7-tab Studio (`Tasks` Kanban, `Skills`, `Memory & Soul`, `MCP Tools` 28 toolsets, `Cron`, `Sessions` FTS5/branch/rollback, `Approvals`) + Magpie LLM provider gateway settings (`/hermes/magpie/*`) | M1, M5 | survey_1 #41, survey_3 #28 |
| F42 | BotFather NL Creator, Native `WKWebView` Mini Apps & `全球法布施` | Natural-language BotFather (`/botfather/nl-create`), Mini Apps catalog, ToS disclaimer modal, `WKWebView` (`Telegram.WebApp` JS bridge + CloudStorage + Co-Pilot), and embedded `全球法布施` Mini App | M1, M5 | survey_1 #42, survey_3 #28 |
| F43 | Multi-Device Remote Computer Workbench & Tailnet/DERP Mesh | Live base64 desktop viewer with touch mouse/keyboard injection, remote shell terminal, file browser (`32 MB`), `AskEveryTime` approval cards, and Tailnet/DERP mesh status | M1, M5 | survey_1 #43, survey_3 #28 |
| F44 | Platform Version Check & TestFlight / App Store Update Banner | iOS-native version check against release metadata (`bhrum/gpui-telegram` / server) displaying non-intrusive update banner + TestFlight / App Store link (replacing macOS `.app` bundle swapper) | M1, M5 | survey_1 #44 |

---

## Milestones

| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| M1 | Spec-First Rebaseline, Rust `v1.2.0` Core, C-ABI Bridge & GitHub Actions CI | Branch `feat/gpui-telegram-ios-parity` (Draft PR #11); `docs/specs/gpui-telegram-main-ios-parity.md` + authority manifest/validator lock to `bhrum/gpui-telegram@889246e4`; upgrade `source/desktop_core/` to `v1.2.0` (`889246e4`); real `source/host/` C-ABI bridge (zero `acknowledged` stubs, all ~154 RPCs + 22 push events); thread-safe `Fabushi/Bridge/` (delete `SearchIndexingBridge` mock); fix 4 Xcode compile blockers; green `ubuntu-24.04` + `macos-15` + Challenger probe GitHub Actions workflows | none | IN_PROGRESS (`67628f739a49ed997eec7b993e698400`) |
| M2 | Unified Launch, Onboarding/Auth, Multi-Account, Passcode/FaceID, Design System, Shell & Chats/Search/Stories | Replace `ProductionRenderer` split-brain with unified `TelegramBridgeRuntime` startup -> `TelegramAuthFlowView` (Server URL, Password, Phone OTP, Email OTP, Alipay/Device QR, 2FA, Passkey) -> `TelegramRootShellView` (iPhone 4-tab + swipe-back, iPad `NavigationSplitView`, Passcode/FaceID, Multi-Account, 4 Themes + `zh-CN`/`en`) + complete `Chats` tab (Folders, Archive, Stories Strip/Viewer, `+` Compose, 10-tab Global Search) | M1 | PLANNED |
| M3 | Inverted 120Hz Conversation Stage, 12+ Rich Cards, MediaViewer & IME-Safe Composer | Inverted `UICollectionView` + `RowHeightCache`, `.interactive` keyboard dismiss, 900s grouping, swipe-reply, blurred reaction/context portal, optimistic Outbox/PTS sync, UTF-16 entities, 12+ rich cards (Photos/Mosaic/HTTP 206, 100-bar Voice, VideoNote, Docs, Stickers/Dice/GIFs, Polls/Quizzes, Todo/Invoice/Contact/Map, Inline Bot/AI cards, Instant View, TTL), fullscreen `MediaViewer`/`MediaEditor`, `UITextView` `ImeSafetyMachine`, real `VoiceRecorder` FSM, Attachment & Sticker/GIF sheets, Drafts & 61 `/slash` autocomplete | M2 | PLANNED |
| M4 | Peer Profiles, Groups/Forums/Stats, Contacts, WebRTC VoIP/Video Calls (`CallKit` + `TalkMode`) & E2EE | Stretchy `PeerInfoView` + 5 shared media tabs, `GroupChannelAdminView` (Roles, Invites, Forum Topics CRUD, Stats/Boosts, Export), `ContactsTabView` (`A–Z`, `@username` resolve, block), `CallsTabView` + `CallOverlayView` (`LiveKitWebRTC` + `CallKit` + `ReplayKit` + 4-emoji key + rating + 60ms VAD AI Talk Mode orb), and X3DH/Double-Ratchet/Sender-Keys E2EE | M3 | PLANNED |
| M5 | 26-Section Settings, Stars Wallet, Hermes 7-Tab Studio, BotFather Mini Apps (`全球法布施`) & Computer Workbench | All 26 `TelegramSettingsView` routes (Profile edit, Sessions, Privacy/2FA/Passkeys, Storage pie/purge, SOCKS5/HTTP Proxy, Appearance, Power Saving, Calls mic test, Language, Version banner), Stars Wallet/Gifts/Alipay/Redeem, BotFather + `WKWebView` Mini Apps + `全球法布施`, Hermes 7-tab Agent Studio + Magpie Gateway, and Remote `ComputerWorkbenchView` + Tailnet/DERP | M3, M4 | PLANNED |
| M6 | Final Milestone: Phase 1 (100% E2E Pass Tiers 1–4) & Phase 2 (Tier 5 Adversarial Hardening) | Phase 1: Sequential Tier 1 -> Tier 2 -> Tier 3 -> Tier 4 E2E execution & remediation on `macos-15` Simulator + real loopback `gpui_telegram_server` until 100% pass; Phase 2: Dedicated Tier-5 white-box Challenger -> Worker -> Reviewer + Whole-Project Auditor hardening loop matching upstream `tier5_adversarial_core_resilience` + `tier5_adversarial_hardening` | M1, M2, M3, M4, M5, E2E | PLANNED |
| E2E | E2E Testing Track (Opaque-Box 4-Tier Suite & Loopback Runner) | Author `TEST_INFRA.md`, build deterministic `macos-15` Simulator + loopback `gpui_telegram_server` runner (`tests/ios_e2e/`) & pure black-box `FabushiUITests/Parity/` covering all 44 features across Tiers 1–4 (`>= 506` test cases / assertions per 4-tier formula: Tier 1 `>=220`, Tier 2 `>=220`, Tier 3 `>=44`, Tier 4 `>=22`), and publish `TEST_READY.md` | none (parallel track) | IN_PROGRESS (`8addcc040781cc61f96e15fc66c92508`) |

---

## Interface Contracts

### 1. Rust Host C-ABI ↔ Swift `TelegramBridgeRuntime` (`source/host` ↔ `mobile/ios/Fabushi/Bridge`)
- **C Header (`source/host/include/mahayana_app_host.h`)**:
  - `TelegramAppHostHandle* telegram_app_host_create(const char* storage_dir, const char* storage_passphrase, const char* default_server_url);`
  - `void telegram_app_host_destroy(TelegramAppHostHandle* handle);` (MUST atomically clear event callback & join/abort background observer tasks before dropping handle — zero UAF!)
  - `char* telegram_app_host_dispatch(TelegramAppHostHandle* handle, const char* command_utf8, const char* params_json_utf8);`
  - `void telegram_app_host_free_string(char* ptr);`
  - `void telegram_bridge_register_event_callback(TelegramAppHostHandle* handle, TelegramBridgeEventCallback cb, void* user_ctx);` (`cb == NULL` unregisters cleanly)
- **JSON-RPC Envelope (`snake_case` wire format across Rust & Swift `JSONDecoder.keyDecodingStrategy = .convertFromSnakeCase`)**:
  - Success: `{"ok": true, "result": <T>}`
  - Error: `{"ok": false, "error": {"code": "<typed_code>", "message": "<human_readable>", "http_status": <u16|null>}}`
  - **Strict Anti-Facade Rule**: Unknown commands MUST return `{"ok": false, "error": {"code": "unknown_command", ...}}`. Catch-all `{"status": "acknowledged"}` is **FORBIDDEN**.
- **Command Namespace Groups (implemented in `source/host/src/dispatch/`)**:
  - `config.*`: `get_server_url`, `set_server_url`, `ping_health`, `set_proxy`, `set_network_online`, `check_release_update`
  - `auth.*`: `login`, `register`, `email_send_code`, `email_register`, `phone_send_code`, `phone_verify`, `alipay_start`, `alipay_poll`, `qr_pair_init`, `qr_pair_poll`, `qr_pair_accept`, `verify_2fa`, `get_2fa_status`, `set_2fa`, `disable_2fa`, `passkey_challenge`, `passkey_verify`, `list_passkeys`, `register_passkey`, `delete_passkey`, `get_me`, `list_sessions`, `revoke_session`, `logout`, `switch_account_slot`
  - `dialogs.*`: `list_chats`, `create_chat`, `create_bot`, `get_chat`, `toggle_pin`, `toggle_mute`, `toggle_archive`, `mark_read`, `mark_unread`, `clear_history`, `leave_or_delete`, `set_chat_ttl`, `list_folders`, `save_folder`, `delete_folder`, `list_topics`, `create_topic`, `update_topic`, `delete_topic`, `search_global`, `list_stories`, `create_story`, `view_story`, `delete_story`
  - `messages.*`: `list_messages`, `send_message`, `edit_message`, `delete_messages`, `forward_messages`, `toggle_reaction`, `list_pinned`, `pin_message`, `unpin_message`, `save_draft`, `send_typing`, `vote_poll`, `close_poll`, `toggle_todo`, `click_bot_button`, `translate_message`, `get_instant_view`, `list_send_as_peers`, `purge_expired_ttl`, `sync_get_difference`
  - `media.*`: `upload_file`, `upload_chunked_init`, `upload_chunked_part`, `upload_chunked_complete`, `download_media`, `get_stream_url`, `encode_waveform_5bit`, `decode_waveform_5bit`, `list_sticker_sets`, `install_sticker_set`, `search_gifs`, `get_storage_stats`, `clear_storage_cache`
  - `peers.*`: `get_peer_profile`, `update_my_profile`, `list_shared_media`, `list_members`, `update_member_role`, `kick_member`, `list_invites`, `create_invite`, `get_channel_stats`, `export_chat`, `list_contacts`, `add_contact`, `delete_contact`, `resolve_username`, `block_peer`, `unblock_peer`, `list_blocked`
  - `calls.*`: `list_call_logs`, `invite_call`, `send_call_signal`, `hangup_call`, `rate_call`, `compute_call_emojis`, `talk_start_session`, `talk_send_audio`, `talk_barge_in`, `talk_stop_session`
  - `e2ee.*`: `publish_prekeys`, `init_encrypted_session`, `send_encrypted`, `decrypt_envelope`, `get_safety_fingerprint`, `list_devices`, `revoke_device`, `register_push_token`
  - `wallet.*`: `get_balance`, `list_transactions`, `topup_stars`, `send_stars`, `send_star_gift`, `create_alipay_order`, `redeem_code`
  - `agent.*`: `list_tasks`, `create_task`, `update_task`, `list_skills`, `save_skill`, `get_memory_files`, `save_memory_file`, `list_mcp_tools`, `toggle_mcp_toolset`, `decide_approval`, `list_cron_jobs`, `save_cron_job`, `list_sessions`, `branch_session`, `rollback_session`, `list_magpie_providers`, `save_magpie_provider`, `nl_create_bot`, `list_miniapps`, `launch_miniapp`, `miniapp_cloud_storage`, `fabushi_miniapp_rpc`, `list_computers`, `exec_computer_action`, `approve_computer_command`, `get_tailnet_status`
- **Upstream `889246e4` Hardening & AuthZ Contract Invariants**:
  - **Hermes Toolsets (`F41`)**: Global `PUT /api/v1/hermes/toolsets` requires `is_support == 1` (`403` for normal users); `source/host/` stores per-user `ToolsetPreset` locally and supplies `toolset_preset` per-turn (`POST /api/v1/hermes/turns` / `/agent-runs`). Dropping/aborting streaming SSE tasks aborts upstream LLM generation within `<250ms` via server `AbortOnDropHandle`.
  - **Channel `Send-As` (`F31`)**: `send_as_peer_id` is forbidden in 1:1 Direct chats (`chat_type == 0` -> `403`) and requires `channel_chat_id as u64` (`chat_type == 3`) where caller holds `owner` or `CAN_POST_MESSAGES` (`1 << 2`). Hide `composer.sendAsButton` when `chat_type == 0`.
  - **E2EE & Proxy (`F38, F39`)**: Surface `E2eeError::ReplayDetected` (BLAKE3 X3DH handshake deduplication + archived `SenderKeyState` chains) deterministically (`"e2ee_replay_detected"`), and enforce fail-closed proxy configuration (`ClientProxyConfig::validate()` + `fallback_to_direct = false` with zero direct TCP/DNS leaks).
  - **WebSocket Errors (`F08, F17, F25, F37`)**: Forward `WsServerFrame::Error { code: 403 | 404, message }` (emitted by `GatewayHub` on unauthorized `SubscribeChat`, `VotePoll`, and `CallSignal`) through `EventBridgeRegistry` to Swift.
- **Push Event Stream (`TelegramEventStream`)**:
  - Thread-safe multi-subscriber broadcaster (`UUID -> AsyncStream<TelegramBridgeEvent>.Continuation` guarded by `NSLock`) delivering all 22 `ClientDomainEvent` variants (`new_message`, `edit_message`, `delete_messages`, `reactions_updated`, `poll_updated`, `todo_updated`, `read_history`, `typing`, `user_status_changed`, `pinned_messages_changed`, `draft_updated`, `forum_topic_changed`, `folder_changed`, `call_updated`, `story_updated`, `agent_floor_changed`, `agent_workspace_updated`, `agent_handoff_event`, `chat_action`, `message_ack`, `sync_gap_detected`, `connection_state_changed`).

### 2. App Launch Environment & Loopback E2E Harness Contract
The iOS app (`FabushiApp` / `FabushiSceneRoot` / `TelegramBridgeRuntime`) MUST honor these `ProcessInfo.processInfo.environment` keys on launch:
- `GPUI_TELEGRAM_SERVER_URL`: Overrides default cloud URL (`http://104.197.143.175:8080`), e.g. `http://127.0.0.1:18080` for `macos-15` loopback `gpui_telegram_server` E2E tests.
- `GPUI_TELEGRAM_STORAGE_DIR`: Overrides per-test isolated sandbox directory for clean multi-test isolation.
- `GPUI_TELEGRAM_SIMULATE_OFFLINE`: `"1"` starts `TelegramClient` in offline outbox-queueing mode (toggleable in Settings/Debug or via runtime notification for offline recovery E2E tests).
- `GPUI_TELEGRAM_LOCALE`: `"en"` or `"zh-Hans"` initial locale override.

### 3. Stable Accessibility Identifier Contract (`Presentation` ↔ `FabushiUITests/Parity`)
Every interactive control MUST attach deterministic `.accessibilityIdentifier(...)` strings (independent of `en`/`zh-CN` text):
- **Auth**: `auth.welcome.startButton`, `auth.serverPill`, `auth.serverUrlField`, `auth.serverApplyButton`, `auth.modePicker`, `auth.usernameField`, `auth.passwordField`, `auth.firstNameField`, `auth.submitButton`, `auth.switchRegisterToggle`, `auth.countryButton`, `auth.phoneField`, `auth.emailField`, `auth.sendCodeButton`, `auth.otpField`, `auth.2faPasswordField`, `auth.qrImage`, `auth.passkeyButton`, `auth.errorBanner`
- **Shell & Security**: `shell.root`, `shell.tab.contacts`, `shell.tab.calls`, `shell.tab.chats`, `shell.tab.settings`, `shell.connectionStatus`, `shell.passcodeLock.view`, `shell.passcodeLock.pinField`, `shell.accountSwitcher.sheet`
- **Chats, Folders, Stories & Search**: `chats.list`, `chats.editButton`, `chats.composeMenuButton`, `chats.compose.newMessage`, `chats.compose.newGroup`, `chats.compose.newChannel`, `chats.compose.newBot`, `chats.searchBar`, `chats.search.scope.<scope>`, `chats.search.tag.<tag>`, `chats.storiesStrip`, `chats.archiveRow`, `chats.folder.<folderId>`, `chats.row.<chatId>`, `chats.row.<chatId>.unreadBadge`
- **Conversation & MediaViewer**: `stage.collectionView`, `stage.header.title`, `stage.header.subtitle`, `stage.header.avatarButton`, `stage.header.callButton`, `stage.header.searchButton`, `stage.header.moreButton`, `stage.pinnedBanner`, `stage.scrollToBottomButton`, `stage.bubble.<msgId>`, `stage.reactionStrip.<emoji>`, `stage.contextMenu.<action>`, `mediaViewer.root`, `mediaViewer.closeButton`, `mediaViewer.editButton`
- **Composer**: `composer.textView`, `composer.attachButton`, `composer.stickerButton`, `composer.micOrSendButton`, `composer.replyBanner`, `composer.cancelReplyButton`, `composer.botMenuButton`, `composer.slashPopup`, `attach.tab.<tab>`, `stickerPanel.tab.<tab>`
- **Profile, Groups, Contacts & Calls**: `profile.root`, `profile.callButton`, `profile.videoButton`, `profile.muteButton`, `profile.tab.<tab>`, `groupAdmin.membersList`, `groupAdmin.topicsList`, `contacts.list`, `contacts.addButton`, `calls.list`, `calls.segmentPicker`, `callOverlay.root`, `callOverlay.hangupButton`, `callOverlay.emojiBadge`, `talkMode.orbView`
- **Settings, Wallet, Agent, MiniApps & Computers**: `settings.list`, `settings.row.<section>`, `settings.theme.<mode>`, `settings.language.<lang>`, `settings.logoutButton`, `wallet.balanceLabel`, `wallet.sendGiftButton`, `hermes.studio.root`, `hermes.tab.<tab>`, `miniapp.webview.sheet`, `computer.workbench.root`

---

## Code Layout

Exclusive file ownership per Milestone (Workers MUST stage ONLY owned paths via `git add -- <owned_paths>`, NEVER `git add -A` or `git add .`):

```text
/home/antigravity/fabushi-ios/fabushi-ios/
├── PROJECT.md                                         # [Project Orchestrator]
├── TEST_INFRA.md                                      # [E2E Track]
├── TEST_READY.md                                      # [E2E Track — removed in M1, published when E2E Track completes]
├── docs/specs/gpui-telegram-main-ios-parity.md        # [M1]
├── manifests/                                         # [M1]
├── docs/parity/                                       # [M1]
├── scripts/
│   ├── check-desktop-main-ios-architecture.py         # [M1]
│   ├── build-mahayana-ios.sh                          # [M1]
│   └── clean_build_garbage.py                         # [M1]
├── .github/workflows/
│   ├── gpui-telegram-ios-ci.yml                       # [M1]
│   ├── gpui-telegram-probe.yml                        # [M1]
│   ├── ios-desktop-main-architecture.yml              # [M1]
│   └── gpui-telegram-e2e-suite.yml                    # [E2E Track]
├── source/
│   ├── desktop_core/                                  # [M1] Exact bhrum/gpui-telegram@889246e4 crates
│   └── host/                                          # [M1] Real C-ABI FFI bridge over TelegramClient v1.2.0
├── frontend/src/production/
│   ├── FabushiSceneRoot.swift                         # [M2]
│   └── ProductionRenderer.view.swift                  # [M2]
├── mobile/ios/
│   ├── project.yml                                    # [M1]
│   ├── Fabushi/
│   │   ├── Bridge/                                    # [M1]
│   │   └── Presentation/
│   │       ├── DesignSystem/                          # [M2]
│   │       ├── Auth/                                  # [M2]
│   │       ├── Shell/                                 # [M2] (M1: 2-line Spacer compile fix only)
│   │       ├── Chats/                                 # [M2] (M1: ChatListRowMetrics dedup fix only)
│   │       ├── Stories/                               # [M2]
│   │       ├── Conversation/                          # [M3]
│   │       ├── Composer/                              # [M3]
│   │       ├── MediaViewer/                           # [M3]
│   │       ├── Profile/                               # [M4]
│   │       ├── Groups/                                # [M4]
│   │       ├── Contacts/                              # [M4]
│   │       ├── Calls/                                 # [M4]
│   │       ├── Security/                              # [M4]
│   │       ├── Settings/                              # [M5]
│   │       ├── Wallet/                                # [M5]
│   │       ├── Agent/                                 # [M5]
│   │       ├── MiniApps/                              # [M5]
│   │       └── Computers/                             # [M5]
│   ├── FabushiTests/
│   │   ├── TelegramBridgeFFITests.swift               # [M1]
│   │   ├── AuthShellChatsTests.swift                  # [M2]
│   │   ├── ConversationComposerTests.swift            # [M3]
│   │   ├── ProfileCallsE2EETests.swift                # [M4]
│   │   └── SettingsAgentMiniAppsTests.swift           # [M5]
│   └── FabushiUITests/
│       └── Parity/                                    # [E2E Track; M6 Tier 5 adds Tier5Adversarial*UITests.swift]
└── tests/
    ├── e2e/master_e2e_suite.swift                     # [M1: remove @main/collisions/hardcoded constants]
    └── ios_e2e/                                       # [E2E Track]
```

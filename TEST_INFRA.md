# GPUI-Telegram iOS Opaque-Box E2E Test Infrastructure (`TEST_INFRA.md`)

- **Canonical Upstream Authority**: `bhrum/gpui-telegram` @ `c40494379cd03f71a2c1f57bae5f85d677cb0e3e`
- **iOS Target Branch**: `feat/gpui-telegram-ios-parity`
- **Feature Scope**: `F01..F44` (`PROJECT.md` lines 69–114)
- **Execution Policy**: GitHub Actions `macos-15` runners ONLY (`AGENTS.md` — never execute builds, typechecks, or test runners inside Linux/local assistant containers).

---

## 1. Opaque-Box Architecture & Purity Rules

Every Swift source file under `mobile/ios/FabushiUITests/Parity/**` is compiled into the separate `FabushiUITests` UI test runner process and interacts with `Fabushi.app` **exclusively** through:
1. User-visible `XCUIApplication` / `XCUIElement` accessibility hierarchy and multi-touch `XCUICoordinate` gestures,
2. Launch environment variables (`PROJECT.md §2`), and
3. Companion HTTP REST (`/api/v1/*`) + RFC 6455 WebSocket (`/api/v1/ws`) traffic against a real loopback `gpui_telegram_server` daemon (`LoopbackPeerClient`).

### Strict Import & Anti-Facade Gate (`tests/ios_e2e/coverage_auditor.py`)
- **Allowed Swift Imports**: `ALLOWED_SWIFT_IMPORTS = {"XCTest", "Foundation"}` ONLY (`CGFloat` / `CGVector` are re-exported by `XCTest` on iOS Simulator).
- **Forbidden**: `@testable import`, `import Fabushi`, C-ABI symbols (`telegram_app_host_*`, `mahayana_app_host*`), `dlopen`/`dlsym`, `import SQLite3`, `XCTSkip*`, `XCTExpectFailure`, and constant tautologies (`XCTAssertTrue(true)`, `XCTAssertFalse(false)`, `XCTAssertNil(nil)`, `XCTAssertEqual(x, x)`).
- Every `func test...()` method must execute `>= 1` genuine `XCTAssert*` / `XCTUnwrap` assertion and interact with `XCUIApplication` / `LoopbackPeerClient`.

---

## 2. Loopback `gpui_telegram_server` & Multi-Peer Companion Harness

| File | Responsibility |
|---|---|
| `tests/ios_e2e/server_manager.py` | Resolves upgraded `source/desktop_core` (when `crates/gpui_telegram_server/src/routes/mod.rs` exists) OR shallow-fetches pinned `bhrum/gpui-telegram@c40494379cd03f71a2c1f57bae5f85d677cb0e3e` into `$RUNNER_TEMP/gpui-upstream-cache`. Stages binary into `$RUNNER_TEMP/gpui-e2e-bin/gpui_telegram_server` (safe from `scripts/clean_build_garbage.py`), allocates an ephemeral `127.0.0.1:<port>`, sets `GPUI_OFFLINE_TEST=1`, isolated SQLite DB, and isolated CAS directory, and polls `GET /health == "OK"` + `GET /api/v1/status` (`status == "healthy"`). |
| `tests/ios_e2e/companion_peer.py` | Pure Python 3.11+ stdlib HTTP REST + RFC 6455 WebSocket companion client + RFC 8032 Ed25519 `gpui-e2ee-spk-v1` prekey signer + `run_self_test()` verifying `/health`, `/api/v1/status`, `/auth/register`, `/e2ee/prekeys`, concurrent WS `Ping`/`Pong` + `SendMessage`/`PtsPush`, chunked CAS + HTTP `206` byte-range streaming, Tantivy `/search`, and session revocation (`401`). |
| `tests/ios_e2e/coverage_auditor.py` | AST-safe Swift comment/string stripper and brace-balanced test method auditor enforcing opaque-box imports, anti-skip/anti-tautology rules, and 4-tier per-feature quotas. |
| `tests/ios_e2e/run_ios_e2e.py` | Unified CLI entry point (`--verify-harness`, `--audit-coverage`, `--tier 1\|2\|3\|4\|all`, `--feature F01..F44`). |
| `mobile/ios/FabushiUITests/Parity/Support/AccessibilityIDs.swift` | Canonical `PROJECT.md §3` `.accessibilityIdentifier` constants independent of active locale (`en` / `zh-Hans`). |
| `mobile/ios/FabushiUITests/Parity/Support/LoopbackPeerClient.swift` | Swift 6 `Sendable` `JSONValue` + `LoopbackPeerClient` + `LoopbackWebSocketSession` speaking real `/api/v1/*` REST and `/api/v1/ws` frames from inside XCTest. |
| `mobile/ios/FabushiUITests/Parity/Support/ParityTestCase.swift` | Base `XCTestCase` provisioning per-test `GPUI_TELEGRAM_STORAGE_DIR` sandboxes, launch helpers, and canonical Telegram iOS gesture primitives. |

---

## 3. Application Launch Environment Contract (`PROJECT.md §2`)

| Environment Variable | Example Value | Behavioral Contract |
|---|---|---|
| `GPUI_TELEGRAM_SERVER_URL` | `http://127.0.0.1:49312` | Overrides default server URL (`https://chat.onlyquant.top`) on startup and activates deterministic Simulator synthetic mic/camera capture when hardware input is absent. |
| `GPUI_TELEGRAM_STORAGE_DIR` | `/Users/.../fabushi-parity-<uuid>` | Isolates SQLite DB, `session.json`, `server_config.json`, Keychain namespace, and CAS media cache per test. |
| `GPUI_TELEGRAM_SIMULATE_OFFLINE` | `"0"` / `"1"` | Forces `NetworkMonitor` offline (`"1"`) to test durable `OutboxWorker` queueing and reconnect `sync/get_difference` recovery. |
| `GPUI_TELEGRAM_LOCALE` | `"en"` / `"zh-Hans"` | Forces initial UI localization bundle (`en` or Simplified Chinese). |
| `GPUI_TELEGRAM_LATEST_RELEASE_TAG` | `"v99.0.0"` | Deterministic version-comparison override for `F44` non-intrusive TestFlight / App Store update banner verification. |

---

## 4. Deterministic Accessibility Identifier Taxonomy (`PROJECT.md §3`)

- **Auth (`F01..F07`)**: `auth.welcome.startButton`, `auth.serverPill`, `auth.serverUrlField`, `auth.serverApplyButton`, `auth.modePicker`, `auth.usernameField`, `auth.passwordField`, `auth.firstNameField`, `auth.submitButton`, `auth.switchRegisterToggle`, `auth.countryButton`, `auth.phoneField`, `auth.emailField`, `auth.sendCodeButton`, `auth.otpField`, `auth.2faPasswordField`, `auth.qrImage`, `auth.passkeyButton`, `auth.errorBanner`.
- **Shell & Security (`F08..F11`, `F44`)**: `shell.root`, `shell.tab.contacts`, `shell.tab.calls`, `shell.tab.chats`, `shell.tab.settings`, `shell.connectionStatus`, `shell.passcodeLock.view`, `shell.passcodeLock.pinField`, `shell.accountSwitcher.sheet`, `shell.updateBanner`, `shell.updateBanner.actionButton`, `shell.updateBanner.dismissButton`.
- **Chats, Folders, Stories & Search (`F12..F16`)**: `chats.list`, `chats.editButton`, `chats.composeMenuButton`, `chats.compose.newMessage`, `chats.compose.newGroup`, `chats.compose.newChannel`, `chats.compose.newBot`, `chats.searchBar`, `chats.search.scope.<scope>`, `chats.search.tag.<tag>`, `chats.storiesStrip`, `chats.archiveRow`, `chats.folder.<folderId>`, `chats.row.<chatId>`, `chats.row.<chatId>.unreadBadge`.
- **Conversation Stage & MediaViewer (`F17..F29`)**: `stage.collectionView`, `stage.header.title`, `stage.header.subtitle`, `stage.header.avatarButton`, `stage.header.callButton`, `stage.header.searchButton`, `stage.header.moreButton`, `stage.pinnedBanner`, `stage.scrollToBottomButton`, `stage.jumpMentionButton`, `stage.bubble.<msgId>`, `stage.reactionStrip.<emoji>`, `stage.contextMenu.<action>`, `mediaViewer.root`, `mediaViewer.closeButton`, `mediaViewer.editButton`.
- **Composer (`F30..F34`)**: `composer.textView`, `composer.attachButton`, `composer.stickerButton`, `composer.micOrSendButton`, `composer.replyBanner`, `composer.cancelReplyButton`, `composer.botMenuButton`, `composer.slashPopup`, `composer.sendAsButton`, `composer.voiceLockIndicator`, `composer.voicePauseButton`, `composer.voiceLockedSendButton`, `attach.tab.<tab>`, `stickerPanel.tab.<tab>`.
- **Profile, Groups, Contacts & Calls (`F35..F38`)**: `profile.root`, `profile.callButton`, `profile.videoButton`, `profile.muteButton`, `profile.tab.<tab>`, `groupAdmin.membersList`, `groupAdmin.topicsList`, `contacts.list`, `contacts.addButton`, `calls.list`, `calls.segmentPicker`, `callOverlay.root`, `callOverlay.hangupButton`, `callOverlay.emojiBadge`, `talkMode.orbView`.
- **Settings, Wallet, Agent, MiniApps & Computers (`F39..F44`)**: `settings.list`, `settings.row.<section>`, `settings.theme.<mode>`, `settings.language.<lang>`, `settings.logoutButton`, `wallet.balanceLabel`, `wallet.sendGiftButton`, `hermes.studio.root`, `hermes.tab.<tab>`, `miniapp.webview.sheet`, `computer.workbench.root`.

---

## 5. 4-Tier Coverage Architecture (`SCOPE.md` Formula: `N = 44`)

| Tier | Directory | Naming Pattern | Minimum Count | Milestone Owner |
|---|---|---|---|---|
| **Tier 1** | `Parity/Tier1/*.swift` | `testTier1_F<01..44>_<HappyPath>()` | **`>= 220`** (`>= 5 × 44`) | **`E2E-1`** (Implemented) |
| **Tier 2** | `Parity/Tier2/*.swift` | `testTier2_F<01..44>_<BoundaryOrNegative>()` | **`>= 220`** (`>= 5 × 44`) | `E2E-2` |
| **Tier 3** | `Parity/Tier3/*.swift` | `testTier3_Pair<NN>_F<XX>_F<YY>_<Interaction>()` | **`>= 44`** (touches all `F01..F44`) | `E2E-3` |
| **Tier 4** | `Parity/Tier4/*.swift` | `testTier4_Scenario<NN>_<Journey>()` | **`>= 22`** (end-to-end journeys) | `E2E-3` |
| **Total** | `Parity/Tier{1..4}/` | Unique across all tiers | **`>= 506`** | `E2E-3` (`TEST_READY.md`) |

### Tier 1 Feature-to-File Ledger (`F01..F44`, `220` Tests)

| Feature IDs | `PROJECT.md` Feature Scope | Test Suite File | Count |
|---|---|---|---|
| `F01..F05` | Launch/CI Authority, Password Auth, Alipay QR, Email OTP, Server URL Bar | `Parity/Tier1/Tier1_AuthOnboarding_F01_F05_UITests.swift` | `25` |
| `F06..F11` | Phone/QR Pair, 2FA/Passkeys, Sessions/Revocation, Multi-Account/Passcode, Themes/i18n, 4-Tab/Split Shell | `Parity/Tier1/Tier1_AuthSecurityShell_F06_F11_UITests.swift` | `30` |
| `F12..F16` | Dialog Rows/Swipe/Preview, Folders/Archive/Topics, 10-Tab Search, Stories, `+` Compose | `Parity/Tier1/Tier1_ChatsSearchStories_F12_F16_UITests.swift` | `25` |
| `F17..F22` | Stage Header/Pins, Inverted 120Hz Transcript, Outbox/PTS Sync, UTF-16 Entities, MediaViewer/Mosaic, Chunked CAS | `Parity/Tier1/Tier1_StageSyncMedia_F17_F22_UITests.swift` | `30` |
| `F23..F29` | 5-Bit Voice/VideoNote, Stickers/Dice/GIFs, Polls/Todo/Invoice/Map, Reply/Forward/Edit/Delete, Reaction Portal, Inline Bot/AI/Translate/IV, TTL Reaper | `Parity/Tier1/Tier1_VoiceCardsActions_F23_F29_UITests.swift` | `35` |
| `F30..F34` | Multi-Line Composer/CJK IME, Formatting/Silent/Schedule/Send-As, Attachment/Sticker/Bot Panels, Voice Gesture FSM, Cloud Drafts/61 `/slash` Autocomplete | `Parity/Tier1/Tier1_ComposerVoiceDrafts_F30_F34_UITests.swift` | `25` |
| `F35..F38` | Peer Profile/Admin/Stats/Export, Contacts/Resolve/Block, WebRTC Calls/AI Talk Mode, Signal E2EE/Secret Chats | `Parity/Tier1/Tier1_ProfileContactsCallsE2EE_F35_F38_UITests.swift` | `20` |
| `F39..F44` | 26-Section Settings/Storage/Proxy, Stars Wallet/Gifts/Alipay/Redeem, Hermes 7-Tab Studio/Magpie, BotFather/MiniApps/`全球法布施`, Remote Computers/Tailnet, Update Banner | `Parity/Tier1/Tier1_SettingsWalletAgentComputers_F39_F44_UITests.swift` | `30` |

---

## 6. Canonical Gesture Thresholds (`ParityTestCase.swift`)

- **Swipe-Left-to-Reply (`F26`, `F31`)**: `-64pt` horizontal drag on `stage.bubble.<msgId>` (crosses `44pt` spring threshold).
- **Mic Tap vs Hold-to-Record (`F33`)**: `0.12s` press toggles `voice <-> videoNote` (`< 0.5s`); `>= 0.65s` press records and sends on release.
- **Voice Slide-Left Cancel (`F33`)**: `-110pt` horizontal drag while holding mic (crosses `-80pt` cancel threshold).
- **Voice Slide-Up Lock (`F33`)**: `-72pt` vertical drag while holding mic (crosses `-50pt` lock threshold).
- **Bubble Reaction Portal (`F27`)**: `0.70s` center press on `stage.bubble.<msgId>`.
- **Settings Multi-Account Switcher (`F09`)**: `0.75s` press on `shell.tab.settings`.
- **Pull-Down Stories & Archive (`F13`, `F15`)**: `+180pt` downward drag from `chats.searchBar`.
- **Interactive Pop Back (`F11`)**: Drag from `dx = 0.03` to `dx = 0.82` across window width.

---

## 7. GitHub Actions `macos-15` Execution Commands

```bash
# 1. Verify harness + coverage audit + Swift 6 strict-concurrency typecheck + live loopback server self-test:
gh workflow run gpui-telegram-e2e-suite.yml \
  --ref feat/gpui-telegram-ios-parity \
  -f mode=verify-harness

# 2. Run full Tier 1 XCUITest suite on iPhone 16 Pro Simulator (once M2..M5 UI milestones land):
gh workflow run gpui-telegram-e2e-suite.yml \
  --ref feat/gpui-telegram-ios-parity \
  -f mode=tier1

# 3. Run single-feature slice (e.g. F19 Outbox & PTS Sync):
gh workflow run gpui-telegram-e2e-suite.yml \
  --ref feat/gpui-telegram-ios-parity \
  -f mode=tier1 \
  -f feature=F19
```

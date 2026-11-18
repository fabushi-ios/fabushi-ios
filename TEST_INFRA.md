# GPUI-Telegram iOS Parity — Opaque-Box E2E Test Infrastructure (`TEST_INFRA.md`)

- **Authority Upstream**: `https://github.com/bhrum/gpui-telegram` (`branch: main`, `commit: 889246e4621e3e8e58022895a887c63ea3130bdd`, `rootTreeSha: ca25541f1993ed981a409aa1fb24b5c2f56eae89`, dynamically locked via `manifests/desktop-main-authority.json` and superseding `c40494379cd03f71a2c1f57bae5f85d677cb0e3e`)
- **Target iOS Repo & Branch**: `fabushi-ios/fabushi-ios` (`feat/gpui-telegram-ios-parity`)
- **Spec-First Reference**: `PROJECT.md` (`N = 44` user-facing features `F01 .. F44`)
- **Execution Policy**: **100% GitHub Actions (`ubuntu-24.04` static audit + `macos-15` Xcode 16 + iOS Simulator)**. Zero local or container compilation/test execution (`AGENTS.md`).

---

## 1. Opaque-Box Architectural Boundary

Every test under `mobile/ios/FabushiUITests/Parity/` is a **strictly opaque-box XCUITest** that drives the compiled `Fabushi.app` binary from the outside, paired with a real loopback `gpui_telegram_server` instance and an independent HTTP + RFC 6455 WebSocket companion peer client (`LoopbackPeerClient`).

1. **Allowed Swift Imports (`coverage_auditor.py` enforced)**:
   - `import XCTest`
   - `import Foundation`
2. **Categorically Forbidden in `FabushiUITests/Parity/`**:
   - `@testable import Fabushi` or `import Fabushi`
   - Importing C-ABI headers (`telegram_app_host_*` / `mahayana_app_host.h`), `dlopen`/`dlsym`, or Rust staticlibs
   - Direct `SQLite3` inspection of the app's sandboxed `gpui_telegram_cache.db`
   - `XCTSkip`, `XCTSkipIf`, `XCTSkipUnless`, `XCTExpectFailure`, constant tautologies (`XCTAssertTrue(true)`, `XCTAssertEqual(x, x)`), vacuous `XCTAssertNotNil(*.objectValue)`, conditional happy-path guards (`if el.exists`, `if el.waitForExistence`), disjunctive `|| el.exists` tautologies, vacuous `waitForElement -> tap -> XCTAssertTrue(sameVar.exists)`, or hardcoded `"123456"` OTP literals.

---

## 2. Loopback Server & Companion Peer Architecture

```
┌───────────────────────────────────────────────────────────────────────────┐
│ GitHub Actions Runner (`macos-15`, Xcode 16, Rust stable)                │
│                                                                           │
│  ┌──────────────────────────────┐      HTTP + WS (`/api/v1/ws`)           │
│  │ `gpui_telegram_server`       │◄─────────────────────────────────────┐  │
│  │  • 127.0.0.1:<ephemeral>     │                                      │  │
│  │  • Ephemeral SQLite WAL DB   │◄──────────────────┐                  │  │
│  │  • Ephemeral Media CAS Dir   │                   │                  │  │
│  │  • GPUI_OFFLINE_TEST=1       │                   │                  │  │
│  └──────────────────────────────┘                   │                  │  │
│                 ▲                                   │                  │  │
│                 │ Launch Env                        │                  │  │
│                 │ `GPUI_TELEGRAM_SERVER_URL`        │                  │  │
│  ┌──────────────┴───────────────┐    ┌──────────────┴──────────────────┐  │
│  │ iOS Simulator (`Fabushi.app`)│    │ `FabushiUITests-Runner.app`     │  │
│  │  • SwiftUI + UIKit Stage     │◄───│  • `XCUIApplication` gestures   │  │
│  │  • `libgpui_ios_host.a`      │ UI │  • `LoopbackPeerClient` (Peer B,│  │
│  │  • Sandboxed Keychain/SQLite │    │    WebSocket frames, E2EE, CAS, │  │
│  └──────────────────────────────┘    │    Simulated Remote Computer)   │  │
│                                      └─────────────────────────────────┘  │
└───────────────────────────────────────────────────────────────────────────┘
```

- **Server Resolution (`tests/ios_e2e/server_manager.py`)**:
  - Automatically detects whether `source/desktop_core` has been synced to modular `routes/mod.rs` by Milestone `M1`.
  - Before `M1` merges, `server_manager.py` shallow-fetches canonical `bhrum/gpui-telegram` pinned via `manifests/desktop-main-authority.json` (defaulting to `889246e4621e3e8e58022895a887c63ea3130bdd`) into `$RUNNER_TEMP/gpui-upstream-cache` and compiles `gpui_telegram_server --profile ci` into `$RUNNER_TEMP/gpui-e2e-bin/gpui_telegram_server` (outside repo `target/` directories so `scripts/clean_build_garbage.py` never deletes the server binary).
  - Never hardcodes `/api/v1/status` `version == "1.2.0"` (upstream `889246e4` Cargo package version is `"0.1.5"`).
- **Per-Test State Isolation (`ParityTestCase.swift`)**:
  - Every `XCUIApplication` launch passes:
    - `GPUI_TELEGRAM_SERVER_URL=http://127.0.0.1:<port>`
    - `GPUI_TELEGRAM_E2E_ISOLATED_ROOT=<unique per-test directory>`
    - `GPUI_TELEGRAM_UI_TEST_MODE=1`
    - `GPUI_TELEGRAM_DISABLE_ANIMATIONS=1`
  - Every test provisions fresh unique usernames/emails/phones (`UUID` suffix) so tests never collide on server state and re-launches within the same test method reuse `sandboxRoot` to verify cold-start Keychain + SQLite persistence.

---

## 3. Launch Environment & Runtime State Contracts (`M2 .. M5`)

| Environment Variable / Runtime Contract | Expected `Fabushi.app` Behavior |
|---|---|
| `GPUI_TELEGRAM_SERVER_URL` | Overrides default `https://api.fabushi.org` endpoint on startup. |
| `GPUI_TELEGRAM_E2E_ISOLATED_ROOT` | Scopes Keychain service prefix, `UserDefaults` suite, and SQLite `gpui_telegram_cache.db` directory to this path so cold relaunches in the same test preserve state while separate tests start clean. |
| `GPUI_TELEGRAM_SIMULATE_OFFLINE=1` | Forces client transport offline so cached SQLite dialogs/messages render while `shell.connectionStatus` reports `accessibilityValue = "offline"`. |
| `GPUI_TELEGRAM_LATEST_RELEASE_TAG` | Overrides latest GitHub release tag check (`F44`); e.g. `"v99.0.0"` surfaces `shell.updateBanner`. |
| **Dynamic OTP Hint (`auth.debugCodeHint`)** | When `POST /api/v1/auth/email/send-code` or `POST /api/v1/auth/phone/send-code` returns non-empty `debug_code` (loopback server without `GPUI_HIDE_DEBUG_CODE`), `AuthScreen` renders `auth.debugCodeHint` whose `accessibilityValue` / `label` contains the exact 6-digit `debug_code`. |
| **Selection State (`isSelected` / `accessibilityValue`)** | Selectable tabs, scopes, theme buttons, wallpaper tiles, accent swatches, Todo rows, and Hermes presets MUST set `.isSelected` (`accessibilityAddTraits(.isSelected)`) or `accessibilityValue = "selected"` when active (`ParityTestCase.isElementMarkedSelected`). |
| **Hermes Toolsets (`889246e4` AuthZ)** | `GET /api/v1/hermes/toolsets` is readable by all authenticated users (`200 OK`), whereas `PUT /api/v1/hermes/toolsets` requires `is_support == 1`. Regular users persist `toolset_preset` (`safe`, etc.) locally and pass it per-turn on `POST /api/v1/hermes/turns`. |
| **Send-As Identity (`889246e4` AuthZ)** | `composer.sendAsButton` posts `send_as_peer_id = <owned_channel_chat_id>` only into Group/Supergroup/Channel targets (`chat_type != 0`). |

---

## 4. Complete Deterministic Accessibility Identifier Catalog (`AccessibilityIDs.swift`)

All identifiers are **100% locale-independent** (`en` and `zh-Hans`).

### 4.1 Auth & Onboarding (`F01 .. F09`)
- `auth.welcome.startButton` — Optional welcome splash CTA (if shown before Auth form; Password mode `auth.mode.password` is selected by default on Auth form)
- `auth.serverPill` — Top server endpoint pill on Auth screen (`label` / `value` shows normalized server host)
- `auth.serverUrlField`, `auth.serverApplyButton` — Server URL editor sheet text field and save/apply button
- `auth.modePicker` — Auth mode segmented container
- `auth.mode.<password|alipay|email|phone|qr>` — Individual locale-independent mode segment buttons (`password` default)
- `auth.usernameField`, `auth.passwordField`, `auth.firstNameField`, `auth.submitButton` — Username/password login & registration fields
- `auth.switchRegisterToggle` — Switches Password mode between Sign In and Create Account (revealing `auth.firstNameField`)
- `auth.countryButton`, `auth.countrySearchField` — Phone mode country selector button (`label` includes dial prefix e.g. `+81`) and country search field
- `auth.phoneField`, `auth.emailField`, `auth.sendCodeButton`, `auth.otpField`, `auth.debugCodeHint` — Phone/Email OTP request & verification controls (`auth.debugCodeHint` exposes dynamic 6-digit `debug_code` when returned by server)
- `auth.2faPasswordField`, `auth.2faHintLabel` — Cloud 2FA password prompt field and hint label (`F07`)
- `auth.qrImage` — Alipay OAuth QR (`accessibilityValue = attempt_id`) & Desktop QR Pairing (`accessibilityValue = tg://login?token=<pair_token>`)
- `auth.passkeyButton` — WebAuthn / Passkey sign-in button (`F07`)
- `auth.errorBanner` — Inline auth/validation error banner

### 4.2 Root Shell, Multi-Account, Passcode & Update Banner (`F08 .. F11`, `F44`)
- `shell.root` — Main authenticated container
- `shell.tab.contacts`, `shell.tab.calls`, `shell.tab.chats`, `shell.tab.settings` — 4-tab bottom bar items
- `shell.connectionStatus` — Inline sync/network indicator (`accessibilityValue = "connected" | "connecting" | "offline"`)
- `shell.navbarLockButton`, `shell.passcodeLock.view`, `shell.passcodeLock.pinField`, `passcode.disableButton` — App passcode lock button, lock shield, PIN field, and disable action
- `shell.accountSwitcher.sheet`, `accountSwitcher.addAccountButton` — Long-press Settings tab multi-account switcher sheet and add-account action
- `shell.updateBanner`, `shell.updateBanner.actionButton`, `shell.updateBanner.dismissButton` — Non-intrusive release update banner (`F44`)

### 4.3 Chats List, Folders, Search, Stories & Creation Flows (`F12 .. F16`)
- `chats.list`, `chats.editButton`, `chats.multiSelectBar`, `chats.archiveRow` — Dialogs list, Edit button, batch action bar, and Archived Chats row
- `chats.row.<chat_id>`, `chats.row.<chat_id>.unreadBadge`, `chats.row.<chat_id>.draftBadge` — Chat cell, unread pill, and red draft preview badge
- `chats.folder.<folder_id>`, `folders.createButton`, `folders.titleField`, `folders.saveButton` — Folder tabs (`all`, custom IDs) and Settings folder editor controls
- `forum.createTopicButton`, `forum.topicTitleField`, `forum.topicSaveButton` — Forum supergroup topic creation controls (`F13`)
- `chats.searchBar`, `chats.search.scope.<chats|channels|apps|posts|downloads|links|files|music|voice>`, `chats.search.tag.<unread|human|bot|channel>` — Global search bar, 9 category scopes, and filter tags (`F14`)
- `chats.storiesStrip`, `stories.addStoryButton`, `stories.captionField`, `stories.publishButton`, `stories.replyField`, `stories.replySendButton`, `stories.deleteButton` — Ephemeral 24h Stories strip, publisher, viewer reply, and delete controls (`F15`)
- `chats.composeMenuButton`, `chats.compose.newMessage`, `chats.compose.newGroup`, `chats.compose.newChannel`, `chats.compose.newBot` — Top-right compose menu actions (`F16`, `F42`)
- `composeGroup.titleField`, `composeGroup.encryptedToggle`, `composeGroup.createButton` — New Group / Encrypted SenderKey Group sheet (`F16`, `F38`)
- `composeChannel.titleField`, `composeChannel.createButton` — New Channel wizard (`F16`)
- `composeBot.nameField`, `composeBot.personaField`, `composeBot.createButton`, `botFather.nlPromptField`, `botFather.nlSubmitButton` — New Bot / BotFather NL creation sheet (`F16`, `F42`)

### 4.4 Conversation Stage, Media Viewer, Rich Cards & Context Portal (`F17 .. F29`)
- `stage.collectionView` — Inverted `UICollectionView` transcript stage
- `stage.header.title`, `stage.header.subtitle`, `stage.header.avatarButton`, `stage.header.callButton`, `stage.header.searchButton`, `stage.header.moreButton` — Stage navigation header controls
- `stage.inChatSearchField`, `stage.inChatSearchPrevButton` — In-chat search bar and match navigator (`F14`)
- `stage.more.autoDelete86400`, `stage.more.autoDeleteOff` — Per-chat TTL timer menu actions (`F29`)
- `stage.pinnedBanner`, `stage.scrollToBottomButton`, `stage.jumpMentionButton`, `stage.batchDeleteButton` — Stage floating banners/FABs and multi-select delete action
- `stage.bubble.<msg_id>` — Message cell container (`label` includes visible message/quote/code/filename text)
- `stage.bubble.<msg_id>.status` — Outgoing delivery badge (`accessibilityValue = "pending" | "sent" | "read"`)
- `stage.bubble.<msg_id>.reaction.<emoji>` — Active reaction pill on bubble (`label` includes count e.g. `"❤️ 2"`)
- `stage.bubble.<msg_id>.codeCopyButton` — Code block copy button (`accessibilityValue = "copied"` after tap)
- `stage.bubble.<msg_id>.textSpoilerOverlay`, `stage.bubble.<msg_id>.mediaSpoilerOverlay` — Tap-to-reveal text and media spoiler blur overlays
- `stage.bubble.<msg_id>.mention.<username>` — Tappable `@username` entity link inside bubble
- `stage.bubble.<msg_id>.voiceWaveform`, `stage.bubble.<msg_id>.voiceUnreadDot`, `stage.voiceSpeedPill` — Voice note waveform scrubber (`value` reflects seek progress), unheard dot, and `1x`/`1.5x`/`2x` speed pill (`F23`)
- `stage.bubble.<msg_id>.todo.<item_id>` — Interactive Todo checklist item button (`isSelected` reflects completion, `F25`)
- `stage.bubble.<msg_id>.invoicePayButton` — Telegram Stars `XTR` invoice pay CTA (`F25`)
- `stage.documentPreviewSheet`, `stage.locationMapSheet`, `stage.translateSheet`, `stage.instantViewButton`, `instantView.readerRoot`, `stage.callbackToast`, `stage.openMiniAppButton`, `stage.e2eeKeyBadge` — Document QuickLook sheet, Location map sheet, Translation sheet, Instant View button & reader, Bot callback toast, Mini App header/dock launcher, and Secret Chat key badge
- `stage.reactionStrip.<emoji|expand>` — Blurred portal quick-reaction emoji buttons and 8-column grid expand chevron (`F27`)
- `stage.contextMenu.<reply|copy|edit|pin|forward|translate|select|delete>` — Blurred portal context menu actions (`F26`, `F28`)
- `mediaViewer.root`, `mediaViewer.closeButton`, `mediaViewer.editButton`, `mediaEditor.captionField`, `mediaEditor.sendButton` — Full-screen photo/video viewer & editor (`F21`)

### 4.5 Composer, Attachments, Stickers & Voice Recorder (`F30 .. F34`)
- `composer.textView`, `composer.attachButton`, `composer.stickerButton`, `composer.micOrSendButton` — Core composer bar (`micOrSendButton` `value`/`label` distinguishes `"mic"`, `"videoCircle"`, `"send"`)
- `composer.format.bold`, `composer.sendWithoutSoundAction`, `composer.scheduleMessageAction`, `schedule.confirmButton` — Formatting action and long-press Send menu options (`F31`)
- `composer.replyBanner`, `composer.cancelReplyButton`, `composer.sendAsButton`, `composer.botMenuButton`, `composer.botCommandsSheet`, `composer.slashPopup` — Reply/edit strip, Send-As picker, Bot menu button & sheet, and `/slash` / `@mention` popup (`F31`, `F32`, `F34`)
- `composer.voiceLockIndicator`, `composer.voicePauseButton`, `composer.voiceLockedSendButton` — Hands-free locked voice/video recording controls (`F33`)
- `attach.tab.<gallery|file|location|poll|contact>`, `attach.gallery.tile.<index>`, `attach.spoilerToggle`, `attach.hdToggle`, `attach.captionField`, `attach.sendButton`, `attach.file.sampleLargeBundle` — Attachment half/full sheet controls (`F21`, `F22`, `F32`)
- `pollCreator.questionField`, `pollCreator.option.0`, `pollCreator.option.1`, `pollCreator.submitButton` — Attachment Poll creator (`F32`)
- `stickerPanel.tab.<emoji|stickers|gifs>`, `stickerPanel.cell.0`, `stickerPanel.browsePacksButton`, `stickerPack.install.0`, `stickerPanel.gifSearchField`, `stickerPanel.gifCell.0` — Emoji/Sticker/GIF keyboard panel & pack browser (`F24`, `F32`)

### 4.6 Peer Profile, Group Admin, Contacts, Calls & E2EE (`F35 .. F38`)
- `profile.root`, `profile.callButton`, `profile.videoButton`, `profile.muteButton`, `profile.blockUserButton`, `profile.unblockUserButton`, `profile.startSecretChatButton`, `profile.statisticsButton`, `statistics.chartView` — Peer/Channel profile actions and statistics screen
- `profile.tab.<media|files|links|voice|groups>` — 5 shared media tabs on Peer Profile (`F35`)
- `groupAdmin.membersList`, `groupAdmin.customTitleField`, `groupAdmin.saveRoleButton`, `groupAdmin.inviteLinksRow`, `groupAdmin.createInviteButton`, `groupAdmin.topicsList` — Group/Supergroup admin & Forum topics controls (`F35`)
- `contacts.list`, `contacts.addButton`, `contacts.addUsernameField`, `contacts.addSaveButton`, `contacts.searchField` — Contacts tab & global `@username` search (`F36`)
- `calls.list`, `calls.segmentPicker`, `calls.segment.<all|missed>` — Calls tab log list and segment buttons (`F37`)
- `callOverlay.root`, `callOverlay.acceptButton`, `callOverlay.screenShareButton`, `callOverlay.hangupButton`, `callOverlay.emojiBadge`, `callOverlay.ratingStar.5`, `callOverlay.ratingSubmitButton` — WebRTC voice/video call overlay & 5-star rating sheet (`F37`)
- `talkMode.orbView`, `talkMode.bargeInButton` — AI Talk Mode full-duplex orb (`value` / `label` exposes `"listening"` / `"speaking"`) and barge-in control (`F37`)
- `e2ee.fingerprint12x12Grid`, `e2ee.emojiKeyBadge` — Secret Chat X3DH key verification sheet (`F38`)

### 4.7 26-Section Settings, Wallet, Hermes Studio, Mini Apps & Remote Computers (`F39 .. F44`)
- `settings.list`, `settings.searchField`, `settings.logoutButton` — Settings root list, search bar, and logout button
- `settings.row.<profile|notifications|privacy|twoFactor|passcode|autoDelete|sessions|e2eeDevices|dataStorage|powerSaving|appearance|language|stickers|folders|proxy|server|wallet|hermesStudio|globalFabushi|computers|about>` — Settings section rows
- `settings.profile.firstNameField`, `settings.profile.usernameField`, `settings.profile.bioField`, `settings.profile.saveButton` — Edit My Profile controls (`F39`)
- `settings.theme.<night|tinted|day|classic>`, `appearance.accent.violet`, `appearance.wallpaper.2`, `appearance.textSizeSlider`, `settings.language.<en|zh-Hans>` — Appearance & Localization controls (`F10`, `F39`)
- `twoFactor.disableButton`, `sessions.terminateOthersButton`, `sessions.linkPairTokenField`, `sessions.linkPairConfirmButton`, `autoDelete.option.1w`, `notifications.privateSwitch`, `powerSaving.autoplayGifSwitch`, `storage.pieChartView`, `storage.clearCacheButton`, `proxy.addButton`, `proxy.hostField`, `proxy.portField`, `proxy.saveButton`, `about.versionLabel`, `about.checkForUpdatesButton` — Security, Sessions, TTL, Notifications, Power Saving, Storage, Proxy, and About sub-screens
- `wallet.balanceLabel`, `wallet.topup500Button`, `wallet.sendGiftButton`, `wallet.giftPack.100`, `wallet.confirmSendGiftButton`, `wallet.alipayOrderButton`, `wallet.alipayOrderSheet`, `wallet.redeemCodeButton`, `wallet.redeemCodeField`, `wallet.redeemSubmitButton` — Stars Wallet, Gifts, Alipay order, and Redeem Code controls (`F40`)
- `hermes.studio.root`, `hermes.tab.<tasks|skills|memory|tools|approvals|cron|sessions>`, `hermes.kanban.addCardButton`, `hermes.kanban.titleField`, `hermes.kanban.saveButton`, `hermes.skills.createButton`, `hermes.skills.nameField`, `hermes.skills.saveButton`, `hermes.soul.personaField`, `hermes.soul.saveButton`, `hermes.tools.preset.safe`, `hermes.cron.addButton`, `hermes.cron.nameField`, `hermes.cron.saveButton`, `hermes.magpieGatewayButton`, `magpie.groupNameField`, `magpie.saveGroupButton` — Hermes 7-Tab AI Agent Studio & Magpie Gateway controls (`F41`)
- `miniapp.tosAcceptButton`, `miniapp.webview.sheet`, `fabushiMiniapp.recordPracticeButton`, `fabushiMiniapp.transferMeritButton` — Native `WKWebView` Mini App container & `全球法布施` Dharma Mini App controls (`F42`)
- `computer.workbench.root`, `computer.screenCanvas`, `computer.terminalInput`, `computer.terminalRunButton`, `computer.terminalOutput`, `computer.filesTab`, `computer.fileListView`, `computer.approvalCard`, `computer.approveCommandButton`, `computer.denyCommandButton` — Remote Computer Workbench controls (`F43`; tested end-to-end against `LoopbackPeerClient.startSimulatedComputerHost` polling `/api/v1/computers/local/poll` with `X-Device-Id` and completing `/api/v1/computers/commands/{id}/complete`)

---

## 5. 4-Tier Progressive Opaque-Box Test Quota (`N = 44`)

| Tier | Directory | Naming Convention | Minimum Tests | Purpose |
|---|---|---|---|---|
| **Tier 1** | `Parity/Tier1/` | `testTier1_F<01..44>_<Case>()` | **`220`** (`5 × 44`) | Per-feature happy-path & core behavioral parity across all 44 features |
| **Tier 2** | `Parity/Tier2/` | `testTier2_F<01..44>_<BoundaryOrNegative>()` | **`220`** (`5 × 44`) | Boundary, validation, 401/403/404/429, offline, CJK/UTF-16 & race edge cases |
| **Tier 3** | `Parity/Tier3/` | `testTier3_Pair<01..44>_F<XX>_F<YY>_*()` | **`44`** (`>= N`, every `F01..F44` covered) | Cross-feature pairwise interaction matrix |
| **Tier 4** | `Parity/Tier4/` | `testTier4_Scenario<01..22>_<Journey>()` | **`22`** (`>= ceil(N/2)`) | Multi-step, multi-peer, cold-restart user journeys |
| **Total** | `Parity/` | Unique `XCTestCase` methods | **`506`** | Gate requirement before writing `TEST_READY.md` |

---

## 6. Runner CLI & GitHub Actions Workflow Usage

Workflow: `.github/workflows/gpui-telegram-e2e-suite.yml` (runs `audit-ubuntu` on `ubuntu-24.04` followed by `parity-e2e` on `macos-15`):

```bash
# 1. Static & anti-facade coverage audit (runs on both ubuntu-24.04 and macos-15)
python3 tests/ios_e2e/run_ios_e2e.py --audit-coverage --min-tier1 220 --min-total 220

# 2. Full harness verification (coverage audit + Swift 6 strict-concurrency typecheck + loopback server + multi-peer WS/CAS/E2EE self-test)
python3 tests/ios_e2e/run_ios_e2e.py --verify-harness --min-tier1 220 --min-total 220

# 3. Execute single feature or full Tier on iOS Simulator
python3 tests/ios_e2e/run_ios_e2e.py --tier 1 --feature F18
python3 tests/ios_e2e/run_ios_e2e.py --tier 1 --min-tier1 220 --min-total 220
python3 tests/ios_e2e/run_ios_e2e.py --tier all --min-tier1 220 --min-tier2 220 --min-tier3 44 --min-tier4 22 --min-total 506
```

import Foundation

/// Canonical Opaque-Box Accessibility Identifier Catalog (`TEST_INFRA.md § 2`).
public enum FabushiAID: Sendable {
    // MARK: - § 2.1 Root Shell, Tab Bar & Navigation Chrome (`F10–F15, F45, F46, F52`)
    public enum Root: Sendable {
        public static let splitView = "fabushi.root.splitView"
        public static let tabBarContainer = "fabushi.tabbar.container"
        public static let liquidLens = "fabushi.tabbar.liquidLens"
        public static let tabContacts = "fabushi.tabbar.tab.contacts"
        public static let tabCalls = "fabushi.tabbar.tab.calls"
        public static let tabChats = "fabushi.tabbar.tab.chats"
        public static let tabSettings = "fabushi.tabbar.tab.settings"
        public static let badgeChats = "fabushi.tabbar.badge.chats"
        public static let accountSwitcherSheet = "fabushi.accountSwitcher.sheet"
        public static func accountSlot(_ index: Int) -> String { "fabushi.accountSwitcher.slot.\(index)" }
        public static let addAccount = "fabushi.accountSwitcher.addAccount"
        public static let edgeEffectTop = "fabushi.chrome.edgeEffect.top"
        public static let edgeEffectBottom = "fabushi.chrome.edgeEffect.bottom"
        public static let overlaySheet = "fabushi.overlay.sheet"
        public static let overlayAlert = "fabushi.overlay.alert"
        public static let alertPrimaryButton = "fabushi.overlay.alert.primaryButton"
        public static let alertCancelButton = "fabushi.overlay.alert.cancelButton"
        public static let undoToast = "fabushi.overlay.undoToast"
        public static let undoButton = "fabushi.overlay.undoToast.undoButton"
        public static let undoTimer = "fabushi.overlay.undoToast.timer"
        public static let lockscreenOverlay = "fabushi.lockscreen.overlay"
        public static let lockscreenPinInput = "fabushi.lockscreen.pinInput"
        public static let lockscreenBiometricButton = "fabushi.lockscreen.biometricButton"
        public static let lockscreenErrorLabel = "fabushi.lockscreen.errorLabel"
        public static let semanticTree = "fabushi.debug.semanticTree"
        public static let exportSemanticTree = "fabushi.debug.exportSemanticTree"
    }

    // MARK: - § 2.2 Authorization & Onboarding (`F01–F09, F47`)
    public enum Auth: Sendable {
        public static let splashPager = "fabushi.auth.splash.pager"
        public static let splashStartButton = "fabushi.auth.splash.startButton"
        public static let languageToggle = "fabushi.auth.languageToggle"
        public static let serverProxyButton = "fabushi.auth.serverProxyButton"
        public static let modePicker = "fabushi.auth.modePicker"
        public static let usernameField = "fabushi.auth.usernameField"
        public static let passwordField = "fabushi.auth.passwordField"
        public static let firstNameField = "fabushi.auth.firstNameField"
        public static let lastNameField = "fabushi.auth.lastNameField"
        public static let autoWelcomeToggle = "fabushi.auth.autoWelcomeToggle"
        public static let submitButton = "fabushi.auth.submitButton"
        public static let switchRegisterMode = "fabushi.auth.switchRegisterMode"
        public static let countryButton = "fabushi.auth.countryButton"
        public static let countrySearchField = "fabushi.auth.countrySearchField"
        public static let phoneCodeField = "fabushi.auth.phoneCodeField"
        public static let phoneNumberField = "fabushi.auth.phoneNumberField"
        public static let continueButton = "fabushi.auth.continueButton"
        public static func otpBox(_ index: Int) -> String { "fabushi.auth.otp.box.\(index)" }
        public static let otpContainer = "fabushi.auth.otp.container"
        public static let otpResendTimer = "fabushi.auth.otp.resendTimer"
        public static let emailField = "fabushi.auth.emailField"
        public static let sendEmailCodeButton = "fabushi.auth.sendEmailCodeButton"
        public static let twoFactorPasswordField = "fabushi.auth.twoFactor.passwordField"
        public static let twoFactorEyeToggle = "fabushi.auth.twoFactor.eyeToggle"
        public static let twoFactorMonkey = "fabushi.auth.twoFactor.monkeyAnimation"
        public static let twoFactorHintLabel = "fabushi.auth.twoFactor.hintLabel"
        public static let twoFactorForgotButton = "fabushi.auth.twoFactor.forgotButton"
        public static let alipayQrImage = "fabushi.auth.alipay.qrImage"
        public static let alipayStatusLabel = "fabushi.auth.alipay.statusLabel"
        public static let alipayRefreshButton = "fabushi.auth.alipay.refreshButton"
        public static let pairQrImage = "fabushi.auth.pair.qrImage"
        public static let pairScanButton = "fabushi.auth.pair.scanButton"
        public static let passkeySignInButton = "fabushi.auth.passkey.signInButton"
        public static let serverUrlField = "fabushi.auth.server.urlField"
        public static let proxyModePicker = "fabushi.auth.proxy.modePicker"
        public static let proxyHostField = "fabushi.auth.proxy.hostField"
        public static let proxyPortField = "fabushi.auth.proxy.portField"
        public static let proxyProbeButton = "fabushi.auth.proxy.probeButton"
        public static let proxyHealthLabel = "fabushi.auth.proxy.healthLabel"
        public static let errorBanner = "fabushi.auth.errorBanner"
    }

    // MARK: - § 2.3 Chat List, Folders, Archive, Search, Stories & Creation (`F17–F22`)
    public enum ChatList: Sendable {
        public static let header = "fabushi.chatlist.header"
        public static let headerTitle = "fabushi.chatlist.header.title"
        public static let editButton = "fabushi.chatlist.editButton"
        public static let composeButton = "fabushi.chatlist.composeButton"
        public static let searchBar = "fabushi.chatlist.searchBar"
        public static let searchInput = "fabushi.chatlist.search.input"
        public static func searchScope(_ scope: String) -> String { "fabushi.chatlist.search.scope.\(scope)" }
        public static func searchResult(_ index: Int) -> String { "fabushi.chatlist.search.result.\(index)" }
        public static let foldersBar = "fabushi.chatlist.folders.bar"
        public static let folderTabAll = "fabushi.chatlist.folders.tab.all"
        public static func folderTab(_ folderId: Int64) -> String { "fabushi.chatlist.folders.tab.\(folderId)" }
        public static let archiveRow = "fabushi.chatlist.archive.row"
        public static let collectionView = "fabushi.chatlist.collectionView"
        public static func row(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId)" }
        public static func rowAvatar(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId).avatar" }
        public static func rowTitle(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId).title" }
        public static func rowPreview(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId).preview" }
        public static func rowDraftPrefix(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId).draftPrefix" }
        public static func rowTimestamp(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId).timestamp" }
        public static func rowUnreadBadge(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId).unreadBadge" }
        public static func rowMentionBadge(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId).mentionBadge" }
        public static func rowStatusCheck(_ chatId: Int64) -> String { "fabushi.chatlist.row.\(chatId).statusCheck" }
        public static let swipeRead = "fabushi.chatlist.swipe.read"
        public static let swipePin = "fabushi.chatlist.swipe.pin"
        public static let swipeMute = "fabushi.chatlist.swipe.mute"
        public static let swipeDelete = "fabushi.chatlist.swipe.delete"
        public static let swipeArchive = "fabushi.chatlist.swipe.archive"
        public static let batchToolbar = "fabushi.chatlist.batchToolbar"
        public static let peekPreview = "fabushi.chatlist.peekPreview"
        public static let storiesStrip = "fabushi.stories.strip"
        public static func storyRing(_ peerId: UInt64) -> String { "fabushi.stories.ring.\(peerId)" }
        public static let storiesViewer = "fabushi.stories.viewer"
        public static let storiesReplyField = "fabushi.stories.viewer.replyField"
        public static let storiesCloseButton = "fabushi.stories.viewer.closeButton"
        public static let composeNewPrivateChat = "fabushi.composeMenu.newPrivateChat"
        public static let composeNewGroup = "fabushi.composeMenu.newGroup"
        public static let composeNewChannel = "fabushi.composeMenu.newChannel"
        public static let composeNewBot = "fabushi.composeMenu.newBot"
        public static let composeJoinByInvite = "fabushi.composeMenu.joinByInvite"
        public static let createTitleField = "fabushi.createChat.titleField"
        public static let createForumToggle = "fabushi.createChat.forumToggle"
        public static let createPublicToggle = "fabushi.createChat.publicToggle"
        public static let createSlugField = "fabushi.createChat.slugField"
        public static let createInviteCodeField = "fabushi.createChat.inviteCodeField"
        public static let createSubmitButton = "fabushi.createChat.submitButton"
        public static let createValidationError = "fabushi.createChat.validationError"
    }

    // MARK: - § 2.4 Conversation Transcript, Bubbles, Media, Reactions & Context Menus (`F23–F33, F48, F49`)
    public enum Chat: Sendable {
        public static let header = "fabushi.chat.header"
        public static let headerTitle = "fabushi.chat.header.title"
        public static let headerSubtitle = "fabushi.chat.header.subtitle"
        public static let headerAvatar = "fabushi.chat.header.avatar"
        public static let headerCallButton = "fabushi.chat.header.callButton"
        public static let headerVideoCallButton = "fabushi.chat.header.videoCallButton"
        public static let pinnedBanner = "fabushi.chat.pinnedBanner"
        public static let miniAudioPlayer = "fabushi.chat.miniAudioPlayer"
        public static let miniAudioRateButton = "fabushi.chat.miniAudioPlayer.rateButton"
        public static let inChatSearchBar = "fabushi.chat.inChatSearchBar"
        public static let inChatSearchCounter = "fabushi.chat.inChatSearch.counter"
        public static let inChatSearchNext = "fabushi.chat.inChatSearch.nextButton"
        public static let inChatSearchPrev = "fabushi.chat.inChatSearch.prevButton"
        public static let transcript = "fabushi.chat.transcript"
        public static let stickyDatePill = "fabushi.chat.stickyDatePill"
        public static let unreadDivider = "fabushi.chat.unreadDivider"
        public static let scrollToBottomButton = "fabushi.chat.scrollToBottomButton"
        public static func message(_ id: Int64) -> String { "fabushi.chat.message.\(id)" }
        public static func messageText(_ id: Int64) -> String { "fabushi.chat.message.\(id).text" }
        public static func messageSpoiler(_ id: Int64) -> String { "fabushi.chat.message.\(id).spoiler" }
        public static func messageCodeCopy(_ id: Int64) -> String { "fabushi.chat.message.\(id).codeCopyButton" }
        public static func messageBlockquote(_ id: Int64) -> String { "fabushi.chat.message.\(id).blockquote" }
        public static func messageReplyHeader(_ id: Int64) -> String { "fabushi.chat.message.\(id).replyHeader" }
        public static func messageStatusIcon(_ id: Int64) -> String { "fabushi.chat.message.\(id).statusIcon" }
        public static func messageSwipeReplyRing(_ id: Int64) -> String { "fabushi.chat.message.\(id).swipeReplyRing" }
        public static func messageVoicePlay(_ id: Int64) -> String { "fabushi.chat.message.\(id).voicePlayButton" }
        public static func messageVoiceWaveform(_ id: Int64) -> String { "fabushi.chat.message.\(id).voiceWaveform" }
        public static func messageVideoNoteCircle(_ id: Int64) -> String { "fabushi.chat.message.\(id).videoNoteCircle" }
        public static func messagePollOption(_ id: Int64, _ idx: Int) -> String { "fabushi.chat.message.\(id).pollOption.\(idx)" }
        public static func messageTodoItem(_ id: Int64, _ idx: Int) -> String { "fabushi.chat.message.\(id).todoItem.\(idx)" }
        public static func messagePayInvoice(_ id: Int64) -> String { "fabushi.chat.message.\(id).payInvoiceButton" }
        public static func messageReactionsBar(_ id: Int64) -> String { "fabushi.chat.message.\(id).reactionsBar" }
        public static func messageReactionPill(_ id: Int64, _ emoji: String) -> String { "fabushi.chat.message.\(id).reactionPill.\(emoji)" }
        public static let contextMenuPortal = "fabushi.chat.contextMenu.portal"
        public static let contextMenuReactionStrip = "fabushi.chat.contextMenu.reactionStrip"
        public static func contextMenuAction(_ action: String) -> String { "fabushi.chat.contextMenu.action.\(action)" }
        public static let galleryContainer = "fabushi.gallery.container"
        public static let galleryImageView = "fabushi.gallery.imageView"
        public static let galleryRotateButton = "fabushi.gallery.rotateButton"
        public static let gallerySaveButton = "fabushi.gallery.saveButton"
        public static let galleryCloseButton = "fabushi.gallery.closeButton"
        public static let instantViewContainer = "fabushi.instantView.container"
        public static let instantViewThemeToggle = "fabushi.instantView.themeToggle"
        public static let instantViewZoomInButton = "fabushi.instantView.zoomInButton"
    }

    // MARK: - § 2.5 Glass Composer, Attachments, Media Editor & Agent Toolbar (`F34–F36, F50`)
    public enum Composer: Sendable {
        public static let container = "fabushi.composer.container"
        public static let attachButton = "fabushi.composer.attachButton"
        public static let aiRewriteButton = "fabushi.composer.aiRewriteButton"
        public static let textView = "fabushi.composer.textView"
        public static let expandEditorButton = "fabushi.composer.expandEditorButton"
        public static let emojiToggle = "fabushi.composer.emojiToggle"
        public static let sendButton = "fabushi.composer.sendButton"
        public static let voiceVideoOrb = "fabushi.composer.voiceVideoOrb"
        public static let recordingCancelLabel = "fabushi.composer.recordingCancelLabel"
        public static let recordingLockPill = "fabushi.composer.recordingLockPill"
        public static let recordingPreviewSendButton = "fabushi.composer.recordingPreviewSendButton"
        public static let replyBanner = "fabushi.composer.replyBanner"
        public static let editBanner = "fabushi.composer.editBanner"
        public static let autocompletePopup = "fabushi.composer.autocompletePopup"
        public static let agentToolbar = "fabushi.composer.agentToolbar"
        public static let agentModelChip = "fabushi.composer.agent.modelChip"
        public static let agentEffortChip = "fabushi.composer.agent.effortChip"
        public static let agentModelMenu = "fabushi.composer.agent.modelMenu"
        public static let agentEffortSlider = "fabushi.composer.agent.effortSlider"
        public static let attachmentSheet = "fabushi.attachment.sheet"
        public static func attachmentTab(_ tab: String) -> String { "fabushi.attachment.tab.\(tab)" }
        public static let mediaEditorContainer = "fabushi.mediaEditor.container"
        public static let mediaEditorCropButton = "fabushi.mediaEditor.cropButton"
        public static let mediaEditorRotateButton = "fabushi.mediaEditor.rotateButton"
        public static let mediaEditorSpoilerToggle = "fabushi.mediaEditor.spoilerToggle"
        public static let mediaEditorSendButton = "fabushi.mediaEditor.sendButton"
        public static let entityKeyboardContainer = "fabushi.entityKeyboard.container"
        public static func entityKeyboardTab(_ tab: String) -> String { "fabushi.entityKeyboard.tab.\(tab)" }
        public static let richArticleEditor = "fabushi.richArticleEditor.container"
    }

    // MARK: - § 2.6 Peer Info, Contacts, Calls, Settings & Wallet (`F16, F37–F40, F51`)
    public enum SettingsAndPeers: Sendable {
        public static let peerInfoScrollView = "fabushi.peerInfo.scrollView"
        public static let peerInfoAvatarHero = "fabushi.peerInfo.avatarHero"
        public static func peerInfoAction(_ action: String) -> String { "fabushi.peerInfo.action.\(action)" }
        public static func peerInfoTab(_ tab: String) -> String { "fabushi.peerInfo.tab.\(tab)" }
        public static let peerInfoExportButton = "fabushi.peerInfo.exportButton"
        public static let peerInfoAgentConnectRow = "fabushi.peerInfo.agentConnectRow"
        public static let agentConnectSheet = "fabushi.agentConnect.sheet"
        public static let agentConnectApiKeyField = "fabushi.agentConnect.apiKeyField"
        public static let agentConnectHeaderLabel = "fabushi.agentConnect.headerLabel"
        public static let agentConnectSaveButton = "fabushi.agentConnect.saveButton"
        public static let contactsList = "fabushi.contacts.list"
        public static let contactsAddButton = "fabushi.contacts.addButton"
        public static let contactsIndexBar = "fabushi.contacts.indexBar"
        public static let callsSegmentControl = "fabushi.calls.segmentControl"
        public static let callsNewCallButton = "fabushi.calls.newCallButton"
        public static let callOverlayContainer = "fabushi.callOverlay.container"
        public static let callOverlayEmojiBadge = "fabushi.callOverlay.emojiBadge"
        public static let callOverlayScreenShareButton = "fabushi.callOverlay.screenShareButton"
        public static let callOverlayHangupButton = "fabushi.callOverlay.hangupButton"
        public static let talkModeContainer = "fabushi.talkMode.container"
        public static let talkModeVadIndicator = "fabushi.talkMode.vadIndicator"
        public static let talkModeBargeInStatus = "fabushi.talkMode.bargeInStatus"
        public static let talkModeTranscript = "fabushi.talkMode.transcript"
        public static let settingsList = "fabushi.settings.list"
        public static func settingsSection(_ sectionName: String) -> String { "fabushi.settings.section.\(sectionName)" }
        public static func appearanceTheme(_ mode: String) -> String { "fabushi.settings.appearance.theme.\(mode)" }
        public static let appearanceBubbleFreeToggle = "fabushi.settings.appearance.bubbleFreeToggle"
        public static let appearanceFontSizeSlider = "fabushi.settings.appearance.fontSizeSlider"
        public static let storageClearCacheButton = "fabushi.settings.storage.clearCacheButton"
        public static let aboutCheckUpdatesButton = "fabushi.settings.about.checkUpdatesButton"
        public static let walletContainer = "fabushi.wallet.container"
        public static let walletCreateMnemonicButton = "fabushi.wallet.createMnemonicButton"
        public static let walletImportMnemonicField = "fabushi.wallet.importMnemonicField"
        public static let walletAddressLabel = "fabushi.wallet.addressLabel"
        public static let walletStarsBalance = "fabushi.wallet.starsBalance"
        public static let walletStarsTopupButton = "fabushi.wallet.starsTopupButton"
        public static let walletAlipayVipButton = "fabushi.wallet.alipayVipButton"
        public static let walletRedeemCodeField = "fabushi.wallet.redeemCodeField"
    }

    // MARK: - § 2.7 Hermes Studio, Magpie Gateway, BotFather, Mini App & Tailnet (`F41–F44`)
    public enum Platform: Sendable {
        public static let hermesStudio = "fabushi.hermes.studio"
        public static func hermesTab(_ tab: String) -> String { "fabushi.hermes.tab.\(tab)" }
        public static let hermesKanbanBoard = "fabushi.hermes.kanban.board"
        public static let hermesMemoryEditor = "fabushi.hermes.memory.editor"
        public static let hermesSessionsSearchField = "fabushi.hermes.sessions.searchField"
        public static func hermesApproveOnce(_ id: String) -> String { "fabushi.hermes.approvals.approveOnce.\(id)" }
        public static func hermesDeny(_ id: String) -> String { "fabushi.hermes.approvals.deny.\(id)" }
        public static let magpieDashboard = "fabushi.magpie.dashboard"
        public static let magpieAddProviderButton = "fabushi.magpie.addProviderButton"
        public static let magpiePiiMaskToggle = "fabushi.magpie.piiMaskToggle"
        public static let magpieImportUriField = "fabushi.magpie.importUriField"
        public static let magpieCircuitStatus = "fabushi.magpie.circuitStatus"
        public static let botfatherCreateField = "fabushi.botfather.createField"
        public static let miniappsCenter = "fabushi.miniapps.center"
        public static let miniappsWebview = "fabushi.miniapps.webview"
        public static let dharmaGlobeCanvas = "fabushi.miniapps.dharma.globeCanvas"
        public static let dharmaZenCounterButton = "fabushi.miniapps.dharma.zenCounterButton"
        public static let dharmaLeaderboardTab = "fabushi.miniapps.dharma.leaderboardTab"
        public static let browserAddressBar = "fabushi.browser.addressBar"
        public static let computersWorkbench = "fabushi.computers.workbench"
        public static func computerNode(_ nodeId: String) -> String { "fabushi.computers.node.\(nodeId)" }
        public static let computersDesktopCanvas = "fabushi.computers.desktopCanvas"
        public static let computersTerminalInput = "fabushi.computers.terminalInput"
        public static let computersEmergencyTakeoverButton = "fabushi.computers.emergencyTakeoverButton"
        public static let computersApprovalBanner = "fabushi.computers.approvalBanner"
    }
}

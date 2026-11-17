import Foundation
import XCTest

/// Canonical opaque-box accessibility identifier catalog (`PROJECT.md §3`).
/// Independent of active locale (`en` / `zh-Hans`) and internal view types.
public enum AccessibilityIDs: Sendable {
    public enum Auth: Sendable {
        public static let welcomeStartButton = "auth.welcome.startButton"
        public static let serverPill = "auth.serverPill"
        public static let serverUrlField = "auth.serverUrlField"
        public static let serverApplyButton = "auth.serverApplyButton"
        public static let modePicker = "auth.modePicker"
        public static let usernameField = "auth.usernameField"
        public static let passwordField = "auth.passwordField"
        public static let firstNameField = "auth.firstNameField"
        public static let submitButton = "auth.submitButton"
        public static let switchRegisterToggle = "auth.switchRegisterToggle"
        public static let countryButton = "auth.countryButton"
        public static let phoneField = "auth.phoneField"
        public static let emailField = "auth.emailField"
        public static let sendCodeButton = "auth.sendCodeButton"
        public static let otpField = "auth.otpField"
        public static let twoFactorPasswordField = "auth.2faPasswordField"
        public static let qrImage = "auth.qrImage"
        public static let passkeyButton = "auth.passkeyButton"
        public static let errorBanner = "auth.errorBanner"
    }

    public enum Shell: Sendable {
        public static let root = "shell.root"
        public static let tabContacts = "shell.tab.contacts"
        public static let tabCalls = "shell.tab.calls"
        public static let tabChats = "shell.tab.chats"
        public static let tabSettings = "shell.tab.settings"
        public static let connectionStatus = "shell.connectionStatus"
        public static let passcodeLockView = "shell.passcodeLock.view"
        public static let passcodeLockPinField = "shell.passcodeLock.pinField"
        public static let accountSwitcherSheet = "shell.accountSwitcher.sheet"
        public static let updateBanner = "shell.updateBanner"
        public static let updateActionButton = "shell.updateBanner.actionButton"
        public static let updateDismissButton = "shell.updateBanner.dismissButton"
    }

    public enum Chats: Sendable {
        public static let list = "chats.list"
        public static let editButton = "chats.editButton"
        public static let composeMenuButton = "chats.composeMenuButton"
        public static let composeNewMessage = "chats.compose.newMessage"
        public static let composeNewGroup = "chats.compose.newGroup"
        public static let composeNewChannel = "chats.compose.newChannel"
        public static let composeNewBot = "chats.compose.newBot"
        public static let searchBar = "chats.searchBar"
        public static let storiesStrip = "chats.storiesStrip"
        public static let archiveRow = "chats.archiveRow"

        public static func searchScope(_ scope: String) -> String {
            "chats.search.scope.\(scope)"
        }

        public static func searchTag(_ tag: String) -> String {
            "chats.search.tag.\(tag)"
        }

        public static func folder(_ folderId: String) -> String {
            "chats.folder.\(folderId)"
        }

        public static func row(_ chatId: Int64) -> String {
            "chats.row.\(chatId)"
        }

        public static func rowUnreadBadge(_ chatId: Int64) -> String {
            "chats.row.\(chatId).unreadBadge"
        }
    }

    public enum Stage: Sendable {
        public static let collectionView = "stage.collectionView"
        public static let headerTitle = "stage.header.title"
        public static let headerSubtitle = "stage.header.subtitle"
        public static let headerAvatarButton = "stage.header.avatarButton"
        public static let headerCallButton = "stage.header.callButton"
        public static let headerSearchButton = "stage.header.searchButton"
        public static let headerMoreButton = "stage.header.moreButton"
        public static let pinnedBanner = "stage.pinnedBanner"
        public static let scrollToBottomButton = "stage.scrollToBottomButton"
        public static let jumpMentionButton = "stage.jumpMentionButton"

        public static func bubble(_ msgId: Int64) -> String {
            "stage.bubble.\(msgId)"
        }

        public static func reactionStrip(_ emoji: String) -> String {
            "stage.reactionStrip.\(emoji)"
        }

        public static func contextMenu(_ action: String) -> String {
            "stage.contextMenu.\(action)"
        }
    }

    public enum MediaViewer: Sendable {
        public static let root = "mediaViewer.root"
        public static let closeButton = "mediaViewer.closeButton"
        public static let editButton = "mediaViewer.editButton"
    }

    public enum Composer: Sendable {
        public static let textView = "composer.textView"
        public static let attachButton = "composer.attachButton"
        public static let stickerButton = "composer.stickerButton"
        public static let micOrSendButton = "composer.micOrSendButton"
        public static let replyBanner = "composer.replyBanner"
        public static let cancelReplyButton = "composer.cancelReplyButton"
        public static let botMenuButton = "composer.botMenuButton"
        public static let slashPopup = "composer.slashPopup"
        public static let sendAsButton = "composer.sendAsButton"
        public static let voiceLockIndicator = "composer.voiceLockIndicator"
        public static let voicePauseButton = "composer.voicePauseButton"
        public static let voiceLockedSendButton = "composer.voiceLockedSendButton"

        public static func attachTab(_ tab: String) -> String {
            "attach.tab.\(tab)"
        }

        public static func stickerPanelTab(_ tab: String) -> String {
            "stickerPanel.tab.\(tab)"
        }
    }

    public enum Profile: Sendable {
        public static let root = "profile.root"
        public static let callButton = "profile.callButton"
        public static let videoButton = "profile.videoButton"
        public static let muteButton = "profile.muteButton"
        public static let groupAdminMembersList = "groupAdmin.membersList"
        public static let groupAdminTopicsList = "groupAdmin.topicsList"
        public static let contactsList = "contacts.list"
        public static let contactsAddButton = "contacts.addButton"
        public static let callsList = "calls.list"
        public static let callsSegmentPicker = "calls.segmentPicker"
        public static let callOverlayRoot = "callOverlay.root"
        public static let callOverlayHangupButton = "callOverlay.hangupButton"
        public static let callOverlayEmojiBadge = "callOverlay.emojiBadge"
        public static let talkModeOrbView = "talkMode.orbView"

        public static func tab(_ tab: String) -> String {
            "profile.tab.\(tab)"
        }
    }

    public enum Settings: Sendable {
        public static let list = "settings.list"
        public static let logoutButton = "settings.logoutButton"
        public static let walletBalanceLabel = "wallet.balanceLabel"
        public static let walletSendGiftButton = "wallet.sendGiftButton"
        public static let hermesStudioRoot = "hermes.studio.root"
        public static let miniappWebviewSheet = "miniapp.webview.sheet"
        public static let computerWorkbenchRoot = "computer.workbench.root"

        public static func row(_ section: String) -> String {
            "settings.row.\(section)"
        }

        public static func theme(_ mode: String) -> String {
            "settings.theme.\(mode)"
        }

        public static func language(_ lang: String) -> String {
            "settings.language.\(lang)"
        }

        public static func hermesTab(_ tab: String) -> String {
            "hermes.tab.\(tab)"
        }
    }
}

import SwiftUI
import UniformTypeIdentifiers
import UIKit

enum MobileDestination {
    case home
    case marketplace
    case remoteComputer
}

enum MobileSection: String, CaseIterable, Identifiable {
    case chats, contacts, bots, groups, channels, calls, saved, archive, folders, miniapps, payments, settings
    var id: String { rawValue }
    var label: String {
        switch self {
        case .chats: "聊天"
        case .contacts: "联系人"
        case .bots: "Bots"
        case .groups: "群组"
        case .channels: "频道"
        case .calls: "通话"
        case .saved: "收藏"
        case .archive: "归档"
        case .folders: "文件夹"
        case .miniapps: "Mini Apps"
        case .payments: "支付"
        case .settings: "设置"
        }
    }
    var symbol: String {
        switch self {
        case .chats: "bubble.left.and.bubble.right.fill"
        case .contacts: "person.2.fill"
        case .bots: "sparkles"
        case .groups: "person.3.fill"
        case .channels: "megaphone.fill"
        case .calls: "phone.fill"
        case .saved: "bookmark.fill"
        case .archive: "archivebox.fill"
        case .folders: "folder.fill"
        case .miniapps: "square.grid.2x2.fill"
        case .payments: "wallet.bifold.fill"
        case .settings: "gearshape.fill"
        }
    }
}

struct LoginBlob: View {
    let color: Color
    let width: CGFloat
    let height: CGFloat
    let rotation: Double

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: min(width, height) * 0.42, style: .continuous)
                .fill(color)
                .rotationEffect(.degrees(rotation))
            HStack(spacing: max(8, width * 0.08)) {
                Capsule().fill(.white).frame(width: max(8, width * 0.11), height: max(18, height * 0.25))
                Capsule().fill(.white).frame(width: max(8, width * 0.11), height: max(18, height * 0.25))
            }
            .rotationEffect(.degrees(rotation * 0.35))
        }
        .frame(width: width, height: height)
        .accessibilityHidden(true)
    }
}

struct ContentView: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    let appAgentSurface: FabushiAppAgentSurface
    let bridge: IOSPreloadBridge?
    let reconnectGeneration: Int
    let onShellBack: (() -> Void)?

    init(
        model: MarketplaceModel,
        messaging: MessagingModel,
        appAgentSurface: FabushiAppAgentSurface,
        bridge: IOSPreloadBridge? = nil,
        reconnectGeneration: Int = 0,
        initialConversation: ConversationSummary? = nil,
        initialMessageID: String? = nil,
        initialSection: MobileSection? = nil,
        onShellBack: (() -> Void)? = nil
    ) {
        self.model = model
        self.messaging = messaging
        self.appAgentSurface = appAgentSurface
        self.bridge = bridge
        self.reconnectGeneration = reconnectGeneration
        self.onShellBack = onShellBack
        _selectedConversation = State(initialValue: initialConversation)
        _pendingInitialMessageID = State(initialValue: initialMessageID)
        _activeSection = State(initialValue: initialSection)
    }

    @State var openedMiniApp: MarketplacePlugin?
    @State var destination: MobileDestination = .home
    @State var agentChatPresented = false
    @State var isSearching = false
    @State var homeQuery = ""
    @State var selectedConversation: ConversationSummary?
    @State var pendingInitialMessageID: String?
    @State var messageDraft = ""
    @State var draftSyncTask: Task<Void, Never>?
    @State var replyTarget: ChatMessage?
    @State var editingMessage: ChatMessage?
    @State var forwardMessage: ChatMessage?
    @State var mediaViewerMessage: ChatMessage?
    @State var conversationInfoPresented = false
    @State var chatSearchPresented = false
    @State var chatSearchQuery = ""
    @State var chatSearchMatchIndex: Int?
    @State var chatSearchTargetID: String?
    @State var attachmentPickerPresented = false
    @State var locationSharePresented = false
    @State var locationService = LocationService()
    @State var voiceRecorder = VoiceRecorder()
    @State var voicePlayback = VoicePlaybackController()
    @State var contactSharePresented = false
    @State var pollComposerPresented = false
    @State var pollQuestion = ""
    @State var pollOption1 = ""
    @State var pollOption2 = ""
    @State var pollOption3 = ""
    @State var composeMenuPresented = false
    @State var profileMenuPresented = false
    @State var signOutConfirmationPresented = false
    @State var composeKind: ConversationKind?
    @State var composeName = ""
    @State var composeDescription = ""
    @State var composeParticipantIds: Set<String> = []
    @State var activeSection: MobileSection?
    @State var contactGroupsPresented = false
    @State var folderEditorPresented = false
    @State var folderTitle = ""
    @State var folderConversationIds: Set<String> = []
    @State var folderIncludeGroups = false
    @State var folderIncludeChannels = false
}

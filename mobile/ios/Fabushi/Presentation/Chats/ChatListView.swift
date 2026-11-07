//
//  ChatListView.swift
//  Fabushi
//
//  Native iOS 1:1 Telegram Chat List Stage with Horizontal Folder Strip,
//  Virtualized List, and Search Pull-Down Reveal.
//

import SwiftUI
import Combine
#if canImport(UIKit)
import UIKit
#endif

// MARK: - Folder Tab Definition
public struct ChatFolderItem: Identifiable, Hashable, Sendable {
    public let id: String
    public let title: String
    public let unreadCount: Int
    public let hasUnmutedUnreads: Bool
    public let isSystem: Bool

    public init(
        id: String,
        title: String,
        unreadCount: Int = 0,
        hasUnmutedUnreads: Bool = false,
        isSystem: Bool = true
    ) {
        self.id = id
        self.title = title
        self.unreadCount = unreadCount
        self.hasUnmutedUnreads = hasUnmutedUnreads
        self.isSystem = isSystem
    }

    public static let defaultFolders: [ChatFolderItem] = [
        ChatFolderItem(id: "all", title: "All"),
        ChatFolderItem(id: "unread", title: "Unread"),
        ChatFolderItem(id: "personal", title: "Personal"),
        ChatFolderItem(id: "work", title: "Work"),
        ChatFolderItem(id: "groups", title: "Groups"),
        ChatFolderItem(id: "channels", title: "Channels")
    ]
}

// MARK: - ChatListView
struct ChatListView: View {
    @Bindable var messaging: MessagingModel
    @ObservedObject var coordinator: TelegramTabNavigationCoordinator
    let bridge: IOSPreloadBridge?
    @StateObject private var viewModel = ChatListViewModel()

    @State private var selectedFolderId: String = "all"
    @State private var isSearchActive: Bool = false
    @State private var searchText: String = ""
    @State private var selectedSearchScope: SearchScopeFilter = .chats
    @State private var selectedChatForNavigation: ConversationSummary? = nil

    @Namespace private var folderIndicatorNamespace
    #if canImport(UIKit)
    private let selectionFeedback = UISelectionFeedbackGenerator()
    #endif

    init(
        messaging: MessagingModel,
        coordinator: TelegramTabNavigationCoordinator,
        bridge: IOSPreloadBridge? = nil
    ) {
        self.messaging = messaging
        self.coordinator = coordinator
        self.bridge = bridge
    }

    // Dynamic folder calculation based on ChatListViewModel.chats
    private var folders: [ChatFolderItem] {
        let all = viewModel.chats
        let allUnread = all.filter { $0.unreadCount > 0 }
        let allUnmuted = allUnread.contains(where: { !$0.isMuted })
        let allCount = allUnread.reduce(0) { $0 + max(1, $1.unreadCount) }

        var items: [ChatFolderItem] = [
            ChatFolderItem(id: "all", title: "All", unreadCount: allCount, hasUnmutedUnreads: allUnmuted)
        ]

        if allCount > 0 {
            items.append(ChatFolderItem(id: "unread", title: "Unread", unreadCount: allCount, hasUnmutedUnreads: allUnmuted))
        }

        let personal = all.filter { $0.kind == "direct" }
        let personalUnread = personal.filter { $0.unreadCount > 0 }
        items.append(ChatFolderItem(
            id: "personal",
            title: "Personal",
            unreadCount: personalUnread.reduce(0) { $0 + max(1, $1.unreadCount) },
            hasUnmutedUnreads: personalUnread.contains(where: { !$0.isMuted })
        ))

        let groups = all.filter { $0.kind == "group" }
        let groupsUnread = groups.filter { $0.unreadCount > 0 }
        items.append(ChatFolderItem(
            id: "groups",
            title: "Groups",
            unreadCount: groupsUnread.reduce(0) { $0 + max(1, $1.unreadCount) },
            hasUnmutedUnreads: groupsUnread.contains(where: { !$0.isMuted })
        ))

        let channels = all.filter { $0.kind == "channel" }
        let channelsUnread = channels.filter { $0.unreadCount > 0 }
        items.append(ChatFolderItem(
            id: "channels",
            title: "Channels",
            unreadCount: channelsUnread.reduce(0) { $0 + max(1, $1.unreadCount) },
            hasUnmutedUnreads: channelsUnread.contains(where: { !$0.isMuted })
        ))

        return items
    }

    // Pinned vs Unpinned partitioned chats from viewModel.filteredChats
    private var pinnedChats: [TelegramChatSummary] {
        viewModel.filteredChats.filter { $0.isPinned }
    }

    private var unpinnedChats: [TelegramChatSummary] {
        viewModel.filteredChats.filter { !$0.isPinned }
    }

    var body: some View {
        ScrollViewReader { scrollProxy in
            ZStack(alignment: .top) {
                // Main Virtualized Chat List
                List {
                    // Pull-Down Search Header Anchor
                    Color.clear
                        .frame(height: 0)
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .id("top_anchor")

                    // Pinned Chats Section
                    if !pinnedChats.isEmpty {
                        ForEach(pinnedChats) { chat in
                            chatRowView(for: chat)
                                .id(chat.id)
                        }
                    }

                    // Unpinned Regular Chats Section
                    ForEach(unpinnedChats) { chat in
                        chatRowView(for: chat)
                            .id(chat.id)
                    }
                }
                .listStyle(.plain)
                .safeAreaInset(edge: .top, spacing: 0) {
                    // Fixed Horizontal Folder Strip Header
                    folderStripHeader(scrollProxy: scrollProxy)
                }
                .refreshable {
                    await viewModel.loadChats()
                }
                .navigationTitle("Chats")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Edit") {
                            // Edit chat list / reorder folders
                        }
                        .font(.system(size: 17, weight: .regular))
                        .foregroundStyle(Color.accentColor)
                    }

                    ToolbarItem(placement: .topBarTrailing) {
                        Button {
                            // New message / compose sheet
                        } label: {
                            Image(systemName: "square.and.pencil")
                                .font(.system(size: 17, weight: .medium))
                                .foregroundStyle(Color.accentColor)
                        }
                    }
                }
                .searchable(
                    text: $searchText,
                    isPresented: $isSearchActive,
                    placement: .navigationBarDrawer(displayMode: .automatic),
                    prompt: "Search for chats or messages"
                )

                // Universal Search Overlay when search is actively engaged
                if isSearchActive && !searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    UniversalSearchView(
                        query: searchText,
                        selectedScope: $selectedSearchScope,
                        messaging: messaging,
                        bridge: bridge,
                        onSelectChat: { chat in
                            isSearchActive = false
                            selectedChatForNavigation = chat
                        }
                    )
                    .transition(.opacity.combined(with: .move(edge: .top)))
                    .zIndex(10)
                }
            }
            .task {
                await viewModel.start()
            }
            .onAppear {
                #if canImport(UIKit)
                selectionFeedback.prepare()
                #endif
            }
            .onChange(of: coordinator.reselectTrigger[.chats]?.token) { _, _ in
                guard let trigger = coordinator.reselectTrigger[.chats] else { return }
                switch trigger.action {
                case .scrollToTop:
                    withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                        scrollProxy.scrollTo("top_anchor", anchor: .top)
                    }
                case .scrollToFirstUnread:
                    if let firstUnread = viewModel.filteredChats.first(where: { $0.unreadCount > 0 }) {
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                            scrollProxy.scrollTo(firstUnread.id, anchor: .center)
                        }
                    } else {
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                            scrollProxy.scrollTo("top_anchor", anchor: .top)
                        }
                    }
                }
            }
            .onChange(of: searchText) { newValue in
                viewModel.searchText = newValue
            }
        }
    }

    // MARK: - Dynamic Colors & Metrics
    private var hairlineHeight: CGFloat {
        #if canImport(UIKit)
        return 1.0 / UIScreen.main.scale
        #else
        return 0.5
        #endif
    }

    private var separatorColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .separator)
        #else
        return Color.gray.opacity(0.3)
        #endif
    }

    private var secondaryLabelColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .secondaryLabel)
        #else
        return Color.secondary
        #endif
    }

    private var systemRedColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemRed)
        #else
        return Color(red: 1.0, green: 0.231, blue: 0.188)
        #endif
    }

    private var systemGrayColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemGray)
        #else
        return Color(red: 0.557, green: 0.557, blue: 0.576)
        #endif
    }

    // MARK: - Folder Strip Header
    private func folderStripHeader(scrollProxy: ScrollViewProxy) -> some View {
        VStack(spacing: 0) {
            ScrollViewReader { folderScrollProxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 16) {
                        ForEach(folders) { folder in
                            folderTabButton(folder: folder, scrollProxy: folderScrollProxy)
                                .id("folder_\(folder.id)")
                        }
                    }
                    .padding(.horizontal, 16)
                    .frame(height: 40)
                }
            }

            // Bottom hairline border
            Rectangle()
                .fill(separatorColor.opacity(0.6))
                .frame(height: hairlineHeight)
        }
        .background(.ultraThinMaterial)
    }

    private func folderTabButton(folder: ChatFolderItem, scrollProxy: ScrollViewProxy) -> some View {
        let isSelected = selectedFolderId == folder.id

        return Button {
            guard selectedFolderId != folder.id else { return }
            #if canImport(UIKit)
            selectionFeedback.selectionChanged()
            selectionFeedback.prepare()
            #endif
            withAnimation(.spring(response: 0.32, dampingFraction: 0.78)) {
                selectedFolderId = folder.id
                switch folder.id {
                case "all": viewModel.selectedFolder = .all
                case "unread": viewModel.selectedFolder = .unread
                case "personal": viewModel.selectedFolder = .personal
                case "groups": viewModel.selectedFolder = .groups
                case "channels": viewModel.selectedFolder = .channels
                default: viewModel.selectedFolder = .all
                }
            }
            withAnimation(.easeInOut(duration: 0.25)) {
                scrollProxy.scrollTo("folder_\(folder.id)", anchor: .center)
            }
        } label: {
            VStack(spacing: 0) {
                Spacer(minLength: 0)
                HStack(spacing: 6) {
                    Text(folder.title)
                        .font(.system(size: 15, weight: isSelected ? .bold : .medium))
                        .foregroundStyle(isSelected ? Color.accentColor : secondaryLabelColor)

                    if folder.unreadCount > 0 {
                        Text("\(folder.unreadCount > 999 ? "999+" : "\(folder.unreadCount)")")
                            .font(.system(size: 11, weight: .bold, design: .rounded))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 5)
                            .frame(height: 16)
                            .background(
                                Capsule()
                                    .fill(folder.hasUnmutedUnreads ? systemRedColor : systemGrayColor)
                            )
                    }
                }
                .padding(.bottom, 6)

                // Spring Animated Underline
                ZStack {
                    if isSelected {
                        Capsule()
                            .fill(Color.accentColor)
                            .frame(height: 3)
                            .matchedGeometryEffect(id: "active_folder_underline", in: folderIndicatorNamespace)
                    } else {
                        Color.clear
                            .frame(height: 3)
                    }
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("folder-tab-\(folder.id)")
    }

    // MARK: - Row Construction
    private func chatRowView(for chat: TelegramChatSummary) -> some View {
        ChatListRowView(
            chat: chat,
            typingActor: viewModel.typingIndicators[chat.id],
            onOpenChat: {
                viewModel.markAsRead(chatId: chat.id)
            },
            onTogglePin: {
                viewModel.togglePin(chatId: chat.id)
            },
            onToggleMute: {
                viewModel.toggleMute(chatId: chat.id)
            },
            onToggleMarkRead: {
                if chat.unreadCount > 0 {
                    viewModel.markAsRead(chatId: chat.id)
                }
            },
            onArchive: {
                // Future archive toggle
            },
            onDelete: {
                viewModel.deleteChat(chatId: chat.id)
            }
        )
        .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        .listRowSeparatorLeading(76.0)
    }
}

// Custom alignment guide helper
private extension View {
    func listRowSeparatorLeading(_ inset: CGFloat) -> some View {
        alignmentGuide(.listRowSeparatorLeading) { _ in inset }
    }
}

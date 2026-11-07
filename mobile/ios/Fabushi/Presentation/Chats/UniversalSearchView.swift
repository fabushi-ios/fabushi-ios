//
//  UniversalSearchView.swift
//  Fabushi
//
//  Native iOS 1:1 Telegram Universal Search with Scope Filters,
//  Tokenized Text Highlighting, and Tantivy Full-Text Search Integration.
//

import SwiftUI
import Combine
#if canImport(UIKit)
import UIKit
#endif

// MARK: - Search Scope Filters
public enum SearchScopeFilter: String, CaseIterable, Identifiable, Sendable {
    case chats = "Chats"
    case media = "Media"
    case links = "Links"
    case files = "Files"
    case music = "Music"
    case voice = "Voice"

    public var id: String { rawValue }

    public var icon: String {
        switch self {
        case .chats: return "bubble.left.and.bubble.right"
        case .media: return "photo.on.rectangle"
        case .links: return "link"
        case .files: return "doc"
        case .music: return "music.note"
        case .voice: return "mic"
        }
    }
}

// MARK: - Search Result Models
public struct UniversalSearchResultItem: Identifiable, Hashable, Sendable {
    public let id: String
    public let conversationId: String
    public let title: String
    public let snippet: String
    public let timestamp: Int64
    public let kind: String // "chat" | "message" | "media" | "file"

    public init(
        id: String,
        conversationId: String,
        title: String,
        snippet: String,
        timestamp: Int64 = 0,
        kind: String = "message"
    ) {
        self.id = id
        self.conversationId = conversationId
        self.title = title
        self.snippet = snippet
        self.timestamp = timestamp
        self.kind = kind
    }
}

// MARK: - UniversalSearchView
struct UniversalSearchView: View {
    let query: String
    @Binding var selectedScope: SearchScopeFilter
    @Bindable var messaging: MessagingModel
    let bridge: IOSPreloadBridge?
    let onSelectChat: (ConversationSummary) -> Void

    @State private var searchResults: [UniversalSearchResultItem] = []
    @State private var isSearching: Bool = false
    @State private var searchTask: Task<Void, Never>? = nil

    #if canImport(UIKit)
    private let selectionFeedback = UISelectionFeedbackGenerator()
    #endif

    init(
        query: String,
        selectedScope: Binding<SearchScopeFilter>,
        messaging: MessagingModel,
        bridge: IOSPreloadBridge? = nil,
        onSelectChat: @escaping (ConversationSummary) -> Void
    ) {
        self.query = query
        self._selectedScope = selectedScope
        self.messaging = messaging
        self.bridge = bridge
        self.onSelectChat = onSelectChat
    }

    // Matching local chats
    private var matchingChats: [ConversationSummary] {
        guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return [] }
        let clean = query.lowercased().trimmingCharacters(in: .whitespacesAndNewlines)
        return messaging.conversations.filter { conv in
            conv.title.lowercased().contains(clean) || conv.preview.lowercased().contains(clean)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            // Scope Filter Segmented Pill Strip
            scopeFilterPillStrip

            // Results Container
            if isSearching {
                VStack(spacing: 12) {
                    Spacer()
                    ProgressView()
                        .scaleEffect(1.2)
                    Text("Searching Telegram Tantivy index...")
                        .font(.system(size: 14))
                        .foregroundStyle(.secondary)
                    Spacer()
                }
            } else if matchingChats.isEmpty && searchResults.isEmpty {
                emptyResultsView
            } else {
                resultsListView
            }
        }
        .background(systemBackgroundColor)
        .onAppear {
            #if canImport(UIKit)
            selectionFeedback.prepare()
            #endif
            performSearch(for: query, scope: selectedScope)
        }
        .onChange(of: query) { _, newQuery in
            performSearch(for: newQuery, scope: selectedScope)
        }
        .onChange(of: selectedScope) { _, newScope in
            performSearch(for: query, scope: newScope)
        }
    }

    // MARK: - Dynamic Colors
    private var systemBackgroundColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemBackground)
        #else
        return Color.black
        #endif
    }

    private var secondarySystemFillColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .secondarySystemFill)
        #else
        return Color.gray.opacity(0.2)
        #endif
    }

    private var tertiaryLabelColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .tertiaryLabel)
        #else
        return Color.secondary.opacity(0.6)
        #endif
    }

    // MARK: - Scope Filter Pill Strip
    private var scopeFilterPillStrip: some View {
        VStack(spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(SearchScopeFilter.allCases) { scope in
                        let isSelected = selectedScope == scope
                        Button {
                            guard selectedScope != scope else { return }
                            #if canImport(UIKit)
                            selectionFeedback.selectionChanged()
                            selectionFeedback.prepare()
                            #endif
                            withAnimation(.spring(response: 0.28, dampingFraction: 0.8)) {
                                selectedScope = scope
                            }
                        } label: {
                            HStack(spacing: 5) {
                                Image(systemName: scope.icon)
                                    .font(.system(size: 12, weight: .semibold))
                                Text(scope.rawValue)
                                    .font(.system(size: 14, weight: isSelected ? .semibold : .medium))
                            }
                            .padding(.horizontal, 12)
                            .padding(.vertical, 7)
                            .foregroundStyle(isSelected ? .white : Color.primary)
                            .background(
                                Capsule()
                                    .fill(isSelected ? Color.accentColor : secondarySystemFillColor)
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }

            Divider()
        }
        .background(.ultraThinMaterial)
    }

    // MARK: - Results List
    private var resultsListView: some View {
        List {
            // Section 1: Chats & Contacts (if scope == .chats)
            if selectedScope == .chats && !matchingChats.isEmpty {
                Section(header: Text("CHATS AND CONTACTS").font(.system(size: 12, weight: .bold))) {
                    ForEach(matchingChats) { chat in
                        Button {
                            onSelectChat(chat)
                        } label: {
                            HStack(spacing: 12) {
                                Circle()
                                    .fill(Color.accentColor.opacity(0.85))
                                    .frame(width: 44, height: 44)
                                    .overlay {
                                        Text(String(chat.title.prefix(1)).uppercased())
                                            .font(.headline)
                                            .foregroundStyle(.white)
                                    }

                                VStack(alignment: .leading, spacing: 3) {
                                    highlightedText(text: chat.title, query: query, baseFont: .system(size: 16, weight: .semibold))
                                    highlightedText(text: chat.preview, query: query, baseFont: .system(size: 14, weight: .regular), defaultColor: .secondary)
                                }
                                Spacer()
                            }
                            .padding(.vertical, 4)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }

            // Section 2: Messages with Tantivy Search Highlighting
            if !searchResults.isEmpty {
                Section(header: Text("MESSAGES").font(.system(size: 12, weight: .bold))) {
                    ForEach(searchResults) { result in
                        Button {
                            if let matchedChat = messaging.conversations.first(where: { $0.id == result.conversationId }) {
                                onSelectChat(matchedChat)
                            }
                        } label: {
                            VStack(alignment: .leading, spacing: 5) {
                                HStack {
                                    Text(result.title)
                                        .font(.system(size: 15, weight: .semibold))
                                        .foregroundStyle(.primary)

                                    Spacer()

                                    if result.timestamp > 0 {
                                        Text(formatTimestamp(result.timestamp))
                                            .font(.system(size: 12))
                                            .foregroundStyle(.secondary)
                                    }
                                }

                                highlightedText(
                                    text: result.snippet,
                                    query: query,
                                    baseFont: .system(size: 14, weight: .regular),
                                    defaultColor: .secondary
                                )
                                .lineLimit(3)
                            }
                            .padding(.vertical, 6)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .listStyle(.plain)
    }

    // MARK: - Empty View
    private var emptyResultsView: some View {
        VStack(spacing: 12) {
            Spacer()
            Image(systemName: "magnifyingglass")
                .font(.system(size: 48))
                .foregroundStyle(tertiaryLabelColor)
            Text("No Results Found")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(.secondary)
            Text("Try searching for people, messages, or media")
                .font(.system(size: 14))
                .foregroundStyle(tertiaryLabelColor)
            Spacer()
        }
    }

    // MARK: - Search Execution & Tantivy Dispatch
    private func performSearch(for searchString: String, scope: SearchScopeFilter) {
        searchTask?.cancel()
        let trimmed = searchString.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            searchResults = []
            isSearching = false
            return
        }

        isSearching = true
        searchTask = Task {
            // Debounce 250ms
            try? await Task.sleep(nanoseconds: 250_000_000)
            guard !Task.isCancelled else { return }

            var results: [UniversalSearchResultItem] = []

            // 1. Search via Rust Tantivy Content Search Engine
            do {
                if let bridge {
                    let accountKey = (try? await messaging.contentSearchAccountKey()) ?? "default"
                    let searchClient = DurableContentSearchClient(bridge: bridge)
                    let durableHits = try await searchClient.searchGlobal(
                        accountKeys: [accountKey],
                        query: trimmed
                    )

                    for hit in durableHits {
                        let convTitle = messaging.conversations.first(where: { $0.id == hit.conversationId })?.title ?? "Chat"
                        results.append(UniversalSearchResultItem(
                            id: hit.entryId,
                            conversationId: hit.conversationId,
                            title: convTitle,
                            snippet: hit.snippet,
                            timestamp: hit.timestampMs ?? 0,
                            kind: "message"
                        ))
                    }
                } else {
                    // Dispatch via TelegramBridgeRuntime JSON-RPC
                    struct SearchHit: Decodable {
                        let id: String
                        let conversation_id: String
                        let title: String?
                        let snippet: String
                        let timestamp_ms: Int64?
                    }
                    struct SearchPayload: Encodable {
                        let query: String
                        let scope: String
                    }

                    let hits: [SearchHit] = try await TelegramBridgeRuntime.shared.dispatch(
                        command: "telegram.search",
                        params: SearchPayload(query: trimmed, scope: scope.rawValue.lowercased())
                    )

                    for hit in hits {
                        results.append(UniversalSearchResultItem(
                            id: hit.id,
                            conversationId: hit.conversation_id,
                            title: hit.title ?? "Chat",
                            snippet: hit.snippet,
                            timestamp: hit.timestamp_ms ?? 0,
                            kind: "message"
                        ))
                    }
                }
            } catch {
                // Fallback to local memory message scan
                for (convId, messages) in messaging.messagesByConversation {
                    let convTitle = messaging.conversations.first(where: { $0.id == convId })?.title ?? "Chat"
                    for msg in messages where msg.text.localizedCaseInsensitiveContains(trimmed) {
                        results.append(UniversalSearchResultItem(
                            id: msg.id,
                            conversationId: convId,
                            title: convTitle,
                            snippet: msg.text,
                            timestamp: 0,
                            kind: "message"
                        ))
                    }
                }
            }

            guard !Task.isCancelled else { return }
            await MainActor.run {
                self.searchResults = results
                self.isSearching = false
            }
        }
    }

    // MARK: - Tokenized Text Highlighting
    private func highlightedText(
        text: String,
        query: String,
        baseFont: Font,
        defaultColor: Color = .primary
    ) -> some View {
        let cleanQuery = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanQuery.isEmpty else {
            return Text(text)
                .font(baseFont)
                .foregroundStyle(defaultColor)
        }

        var attributed = AttributedString(text)
        let tokens = cleanQuery.components(separatedBy: .whitespaces).filter { !$0.isEmpty }

        for token in tokens {
            var searchRange = attributed.startIndex..<attributed.endIndex
            while let range = attributed[searchRange].range(of: token, options: .caseInsensitive) {
                attributed[range].foregroundColor = Color.accentColor
                attributed[range].font = baseFont.bold()
                attributed[range].backgroundColor = Color.accentColor.opacity(0.15)
                guard range.upperBound < attributed.endIndex else { break }
                searchRange = range.upperBound..<attributed.endIndex
            }
        }

        return Text(attributed)
            .font(baseFont)
    }

    private func formatTimestamp(_ timestampMs: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(timestampMs) / 1000.0)
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }
}

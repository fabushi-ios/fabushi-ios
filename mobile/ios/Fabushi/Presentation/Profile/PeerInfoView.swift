//
//  PeerInfoView.swift
//  Fabushi
//
//  Feature 27: Collapsing Parallax Peer Profile (1:1 Telegram iOS Native).
//  Parallax header shrinking avatar into navigation bar, quick action buttons,
//  shared media segmented tabs, notification mute presets, and block user controls.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

// MARK: - Enums & Models

public enum SharedMediaType: String, CaseIterable, Identifiable, Sendable {
    case media = "Media"
    case files = "Files"
    case links = "Links"
    case voice = "Voice"
    case groups = "Groups"

    public var id: String { rawValue }

    public var icon: String {
        switch self {
        case .media: return "photo.on.rectangle"
        case .files: return "doc"
        case .links: return "link"
        case .voice: return "waveform"
        case .groups: return "person.2"
        }
    }
}

public enum NotificationMuteDuration: Int, CaseIterable, Identifiable, Sendable {
    case oneHour = 3600
    case eightHours = 28800
    case twoDays = 172800
    case forever = -1
    case unmute = 0

    public var id: Int { rawValue }

    public var title: String {
        switch self {
        case .oneHour: return "Mute for 1 hour"
        case .eightHours: return "Mute for 8 hours"
        case .twoDays: return "Mute for 2 days"
        case .forever: return "Mute Forever"
        case .unmute: return "Unmute"
        }
    }
}

public struct PeerProfile: Identifiable, Sendable, Equatable {
    public let id: String
    public var displayName: String
    public var username: String?
    public var phone: String?
    public var bio: String
    public var avatarUrl: URL?
    public var isMuted: Bool
    public var isBlocked: Bool

    public init(
        id: String,
        displayName: String,
        username: String? = nil,
        phone: String? = nil,
        bio: String = "",
        avatarUrl: URL? = nil,
        isMuted: Bool = false,
        isBlocked: Bool = false
    ) {
        self.id = id
        self.displayName = displayName
        self.username = username
        self.phone = phone
        self.bio = bio
        self.avatarUrl = avatarUrl
        self.isMuted = isMuted
        self.isBlocked = isBlocked
    }
}

public struct SharedMediaItem: Identifiable, Sendable, Equatable {
    public let id: String
    public let type: SharedMediaType
    public let title: String
    public let subtitle: String
    public let url: URL?
    public let sizeBytes: Int64
    public let timestamp: Date

    public init(
        id: String,
        type: SharedMediaType,
        title: String,
        subtitle: String = "",
        url: URL? = nil,
        sizeBytes: Int64 = 0,
        timestamp: Date = Date()
    ) {
        self.id = id
        self.type = type
        self.title = title
        self.subtitle = subtitle
        self.url = url
        self.sizeBytes = sizeBytes
        self.timestamp = timestamp
    }
}

// MARK: - Parallax Header Layout Calculator

public enum PeerInfoLayoutCalculator {
    public static let initialAvatarSize: CGFloat = 100.0
    public static let collapsedAvatarSize: CGFloat = 36.0
    public static let collapseThreshold: CGFloat = 120.0

    /// Calculates avatar scale factor from scroll offset.
    /// - Negative offset (pull-down overscroll): elastic rubber-band zoom >= 1.0.
    /// - Positive offset (scroll up): collapse monotonically down to 0.36.
    public static func calculateAvatarScale(offset: CGFloat) -> CGFloat {
        if offset < 0 {
            let pull = abs(offset)
            return max(1.0, 1.0 + (pull / 200.0))
        } else {
            let clampedOffset = min(collapseThreshold, offset)
            let progress = clampedOffset / collapseThreshold
            let minScale = collapsedAvatarSize / initialAvatarSize // 0.36
            return max(minScale, 1.0 - progress * (1.0 - minScale))
        }
    }

    /// Title fade-in threshold for navigation bar
    public static func calculateNavBarTitleOpacity(offset: CGFloat) -> Double {
        if offset <= 60.0 {
            return 0.0
        }
        let progress = (offset - 60.0) / 60.0
        return min(1.0, max(0.0, Double(progress)))
    }
}

// MARK: - View Model

@MainActor
public final class PeerInfoViewModel: ObservableObject {
    @Published public var profile: PeerProfile
    @Published public var selectedTab: SharedMediaType = .media
    @Published public var sharedItems: [SharedMediaItem] = []
    @Published public var isMuteSheetPresented: Bool = false
    @Published public var didCopyUsername: Bool = false
    @Published public var toastMessage: String? = nil
    @Published public var scrollOffset: CGFloat = 0

    public init(profile: PeerProfile, sharedItems: [SharedMediaItem] = []) {
        self.profile = profile
        self.sharedItems = sharedItems
    }

    public func setMuted(duration: NotificationMuteDuration) async throws {
        if duration == .unmute {
            profile.isMuted = false
        } else {
            profile.isMuted = true
        }
    }

    public func toggleBlockUser() async throws {
        profile.isBlocked.toggle()
    }

    public func copyToClipboard(_ text: String) {
        #if canImport(UIKit)
        UIPasteboard.general.string = text
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
        #endif
        didCopyUsername = true
        toastMessage = "Copied to clipboard"
    }

    public func filteredSharedItems(for type: SharedMediaType) -> [SharedMediaItem] {
        sharedItems.filter { $0.type == type }
    }

    public func addSharedItem(_ item: SharedMediaItem) {
        sharedItems.append(item)
    }
}

// MARK: - Presentation View

public struct PeerInfoView: View {
    @ObservedObject public var viewModel: PeerInfoViewModel

    public init(viewModel: PeerInfoViewModel) {
        self.viewModel = viewModel
    }

    public var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                // Parallax Header
                headerView
                    .padding(.top, 16)
                    .padding(.bottom, 20)

                // Action Buttons
                actionButtonsBar
                    .padding(.horizontal, 20)
                    .padding(.bottom, 20)

                // Info Rows Section
                infoRowsSection
                    .padding(.horizontal, 16)
                    .padding(.bottom, 20)

                // Shared Media Segmented Section
                sharedMediaSection
                    .padding(.horizontal, 16)
                    .padding(.bottom, 20)

                // Block / Destructive Action Section
                destructiveSection
                    .padding(.horizontal, 16)
                    .padding(.bottom, 40)
            }
            .background(GeometryReader { geo in
                Color.clear.preference(
                    key: ScrollOffsetPreferenceKey.self,
                    value: geo.frame(in: .named("PeerInfoScroll")).minY
                )
            })
        }
        .coordinateSpace(name: "PeerInfoScroll")
        .onPreferenceChange(ScrollOffsetPreferenceKey.self) { value in
            viewModel.scrollOffset = -value
        }
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar {
            ToolbarItem(placement: .principal) {
                HStack(spacing: 8) {
                    Circle()
                        .fill(Color.blue.gradient)
                        .frame(width: 28, height: 28)
                        .overlay(
                            Text(String(viewModel.profile.displayName.prefix(1)))
                                .font(.system(size: 14, weight: .bold))
                                .foregroundStyle(.white)
                        )
                    Text(viewModel.profile.displayName)
                        .font(.system(size: 16, weight: .semibold))
                }
                .opacity(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: viewModel.scrollOffset))
            }
        }
        .confirmationDialog(
            "Notifications",
            isPresented: $viewModel.isMuteSheetPresented,
            titleVisibility: .visible
        ) {
            ForEach(NotificationMuteDuration.allCases) { duration in
                Button(duration.title) {
                    Task {
                        try? await viewModel.setMuted(duration: duration)
                    }
                }
            }
            Button("Cancel", role: .cancel) {}
        }
        .overlay(alignment: .bottom) {
            if let toast = viewModel.toastMessage {
                Text(toast)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .background(Color.black.opacity(0.8), in: Capsule())
                    .padding(.bottom, 20)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .onAppear {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                            withAnimation {
                                viewModel.toastMessage = nil
                            }
                        }
                    }
            }
        }
    }

    // MARK: - Header
    private var headerView: some View {
        let scale = PeerInfoLayoutCalculator.calculateAvatarScale(offset: viewModel.scrollOffset)
        return VStack(spacing: 12) {
            Circle()
                .fill(LinearGradient(colors: [Color.blue, Color.cyan], startPoint: .topLeading, endPoint: .bottomTrailing))
                .frame(
                    width: PeerInfoLayoutCalculator.initialAvatarSize * scale,
                    height: PeerInfoLayoutCalculator.initialAvatarSize * scale
                )
                .overlay(
                    Text(String(viewModel.profile.displayName.prefix(1)))
                        .font(.system(size: 40 * scale, weight: .bold))
                        .foregroundStyle(.white)
                )
                .shadow(color: .black.opacity(0.12), radius: 8, x: 0, y: 4)

            VStack(spacing: 4) {
                Text(viewModel.profile.displayName)
                    .font(.system(size: 22, weight: .bold))

                if let username = viewModel.profile.username {
                    Text("@\(username)")
                        .font(.system(size: 14))
                        .foregroundStyle(.secondary)
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: - Action Buttons Bar
    private var actionButtonsBar: some View {
        HStack(spacing: 24) {
            PeerActionButton(title: "Call", icon: "phone.fill") {
                // Call action
            }
            PeerActionButton(title: "Video", icon: "video.fill") {
                // Video action
            }
            PeerActionButton(
                title: viewModel.profile.isMuted ? "Unmute" : "Mute",
                icon: viewModel.profile.isMuted ? "bell.slash.fill" : "bell.fill"
            ) {
                viewModel.isMuteSheetPresented = true
            }
            PeerActionButton(title: "Search", icon: "magnifyingglass") {
                // Search action
            }
        }
    }

    // MARK: - Info Rows
    private var infoRowsSection: some View {
        VStack(spacing: 0) {
            if let phone = viewModel.profile.phone, !phone.isEmpty {
                infoRow(title: phone, subtitle: "Mobile") {
                    viewModel.copyToClipboard(phone)
                }
                Divider().padding(.leading, 16)
            }

            if let username = viewModel.profile.username, !username.isEmpty {
                infoRow(title: "@\(username)", subtitle: "Username") {
                    viewModel.copyToClipboard("@\(username)")
                }
                Divider().padding(.leading, 16)
            }

            if !viewModel.profile.bio.isEmpty {
                infoRow(title: viewModel.profile.bio, subtitle: "Bio") {
                    viewModel.copyToClipboard(viewModel.profile.bio)
                }
            }
        }
        .background(Color.peerSecondaryGroupedBackground, in: RoundedRectangle(cornerRadius: 12))
    }

    private func infoRow(title: String, subtitle: String, onCopy: @escaping () -> Void) -> some View {
        Button(action: onCopy) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 16))
                    .foregroundStyle(.primary)
                Text(subtitle)
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .buttonStyle(.plain)
    }

    // MARK: - Shared Media Section
    private var sharedMediaSection: some View {
        VStack(spacing: 12) {
            Picker("Shared Media", selection: $viewModel.selectedTab) {
                ForEach(SharedMediaType.allCases) { type in
                    Text(type.rawValue).tag(type)
                }
            }
            .pickerStyle(.segmented)

            let items = viewModel.filteredSharedItems(for: viewModel.selectedTab)
            if items.isEmpty {
                emptySharedMediaView(type: viewModel.selectedTab)
                    .padding(.vertical, 32)
            } else {
                LazyVStack(spacing: 8) {
                    ForEach(items) { item in
                        HStack(spacing: 12) {
                            Image(systemName: item.type.icon)
                                .font(.system(size: 20))
                                .foregroundStyle(Color.accentColor)
                                .frame(width: 36, height: 36)
                                .background(Color.accentColor.opacity(0.12), in: Circle())

                            VStack(alignment: .leading, spacing: 2) {
                                Text(item.title)
                                    .font(.system(size: 15, weight: .medium))
                                if !item.subtitle.isEmpty {
                                    Text(item.subtitle)
                                        .font(.system(size: 13))
                                        .foregroundStyle(.secondary)
                                }
                            }
                            Spacer()
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                        .background(Color.peerSecondaryGroupedBackground, in: RoundedRectangle(cornerRadius: 10))
                    }
                }
            }
        }
    }

    private func emptySharedMediaView(type: SharedMediaType) -> some View {
        VStack(spacing: 8) {
            Image(systemName: type.icon)
                .font(.system(size: 40))
                .foregroundStyle(.secondary)
            Text("No \(type.rawValue)")
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(.secondary)
            Text("Shared \(type.rawValue.lowercased()) with this chat will appear here.")
                .font(.system(size: 13))
                .foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
    }

    // MARK: - Destructive Section
    private var destructiveSection: some View {
        VStack(spacing: 0) {
            Button {
                Task {
                    try? await viewModel.toggleBlockUser()
                }
            } label: {
                Text(viewModel.profile.isBlocked ? "Unblock User" : "Block User")
                    .font(.system(size: 16, weight: .medium))
                    .foregroundStyle(viewModel.profile.isBlocked ? .blue : .red)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
            }
            .background(Color.peerSecondaryGroupedBackground, in: RoundedRectangle(cornerRadius: 12))
        }
    }
}

// MARK: - Action Button
private struct PeerActionButton: View {
    let title: String
    let icon: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Circle()
                    .fill(Color.peerSecondaryGroupedBackground)
                    .frame(width: 50, height: 50)
                    .overlay(
                        Image(systemName: icon)
                            .font(.system(size: 20))
                            .foregroundStyle(Color.accentColor)
                    )
                    .shadow(color: .black.opacity(0.04), radius: 4, x: 0, y: 2)
                Text(title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(Color.accentColor)
            }
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Scroll Offset Preference Key
private struct ScrollOffsetPreferenceKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

private extension Color {
    static var peerSecondaryGroupedBackground: Color {
        #if canImport(UIKit)
        return Color(uiColor: .secondarySystemGroupedBackground)
        #else
        return Color.secondary.opacity(0.12)
        #endif
    }
}

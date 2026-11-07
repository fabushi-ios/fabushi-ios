//
//  GroupChannelAdminView.swift
//  Fabushi
//
//  Feature 28: Group & Channel Management (1:1 Telegram iOS Native).
//  Admin permissions matrix, banned users list with cascade deletion,
//  invite links with QR code generator, slow mode slider, and join requests.
//

import SwiftUI
#if canImport(CoreImage)
import CoreImage
import CoreImage.CIFilterBuiltins
#endif
#if canImport(UIKit)
import UIKit
#endif

// MARK: - Enums & Errors

public enum GroupMemberRole: String, Sendable, Codable, CaseIterable {
    case owner = "owner"
    case admin = "admin"
    case member = "member"
    case restricted = "restricted"
    case banned = "banned"

    public var title: String {
        switch self {
        case .owner: return "Owner"
        case .admin: return "Admin"
        case .member: return "Member"
        case .restricted: return "Restricted"
        case .banned: return "Banned"
        }
    }
}

public enum GroupAdminError: Error, Equatable, LocalizedError {
    case cannotDemoteOwner(String)
    case inviteLinkExpired
    case permissionDenied(String)
    case userNotFound

    public var errorDescription: String? {
        switch self {
        case .cannotDemoteOwner(let msg): return msg
        case .inviteLinkExpired: return "This invite link has expired"
        case .permissionDenied(let msg): return msg
        case .userNotFound: return "User not found"
        }
    }
}

public struct GroupAdminPermissions: OptionSet, Sendable, Codable, Equatable {
    public let rawValue: Int

    public init(rawValue: Int) {
        self.rawValue = rawValue
    }

    public static let changeInfo = GroupAdminPermissions(rawValue: 1 << 0)
    public static let postMessages = GroupAdminPermissions(rawValue: 1 << 1)
    public static let editMessages = GroupAdminPermissions(rawValue: 1 << 2)
    public static let deleteMessages = GroupAdminPermissions(rawValue: 1 << 3)
    public static let banUsers = GroupAdminPermissions(rawValue: 1 << 4)
    public static let inviteUsers = GroupAdminPermissions(rawValue: 1 << 5)
    public static let pinMessages = GroupAdminPermissions(rawValue: 1 << 6)

    public static let all: GroupAdminPermissions = [
        .changeInfo, .postMessages, .editMessages, .deleteMessages, .banUsers, .inviteUsers, .pinMessages
    ]
}

public enum SlowModeInterval: Int, CaseIterable, Sendable, Identifiable {
    case off = 0
    case s10 = 10
    case s30 = 30
    case m1 = 60
    case m5 = 300
    case m15 = 900
    case h1 = 3600

    public var id: Int { rawValue }

    public var title: String {
        switch self {
        case .off: return "Off"
        case .s10: return "10s"
        case .s30: return "30s"
        case .m1: return "1m"
        case .m5: return "5m"
        case .m15: return "15m"
        case .h1: return "1h"
        }
    }
}

// MARK: - Models

public struct GroupMemberItem: Identifiable, Sendable, Equatable {
    public let id: String
    public var name: String
    public var username: String?
    public var role: GroupMemberRole

    public init(id: String, name: String, username: String? = nil, role: GroupMemberRole = .member) {
        self.id = id
        self.name = name
        self.username = username
        self.role = role
    }
}

public struct InviteLinkInfo: Identifiable, Sendable, Equatable {
    public let id: String
    public let url: URL
    public let isExpired: Bool
    public var usageCount: Int
    public var usageLimit: Int?

    public init(
        id: String,
        url: URL,
        isExpired: Bool = false,
        usageCount: Int = 0,
        usageLimit: Int? = nil
    ) {
        self.id = id
        self.url = url
        self.isExpired = isExpired
        self.usageCount = usageCount
        self.usageLimit = usageLimit
    }

    public var statusDescription: String {
        if isExpired {
            return "Expired"
        } else {
            return "Active • Used \(usageCount) times"
        }
    }
}

public struct JoinRequestItem: Identifiable, Sendable, Equatable {
    public let id: String
    public let name: String
    public let username: String?
    public let requestedAt: Date

    public init(id: String, name: String, username: String? = nil, requestedAt: Date = Date()) {
        self.id = id
        self.name = name
        self.username = username
        self.requestedAt = requestedAt
    }
}

// MARK: - QR Code Helper

public enum QRCodeGenerator {
    public static func generateQRCodeData(from url: URL) -> Data? {
        #if canImport(CoreImage)
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return nil }
        filter.setValue(url.absoluteString.data(using: .utf8), forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")
        guard let outputImage = filter.outputImage else { return nil }
        let context = CIContext()
        guard let cgImage = context.createCGImage(outputImage, from: outputImage.extent) else { return nil }
        #if canImport(UIKit)
        return UIImage(cgImage: cgImage).pngData()
        #else
        return nil
        #endif
        #else
        return nil
        #endif
    }
}

// MARK: - View Model

@MainActor
public final class GroupChannelAdminViewModel: ObservableObject {
    @Published public var permissions: GroupAdminPermissions = .all
    @Published public var members: [GroupMemberItem] = []
    @Published public var slowMode: SlowModeInterval = .off
    @Published public var pendingRequests: [JoinRequestItem] = []
    @Published public var inviteLinks: [InviteLinkInfo] = []
    @Published public var purgedUserSenderIds: Set<String> = []
    @Published public var channelViewsCount: Int = 1420

    public init(
        members: [GroupMemberItem] = [],
        inviteLinks: [InviteLinkInfo] = [],
        pendingRequests: [JoinRequestItem] = []
    ) {
        self.members = members
        self.inviteLinks = inviteLinks
        self.pendingRequests = pendingRequests
    }

    public func demoteMember(id: String) throws {
        guard let index = members.firstIndex(where: { $0.id == id }) else {
            throw GroupAdminError.userNotFound
        }

        if members[index].role == .owner {
            throw GroupAdminError.cannotDemoteOwner("Cannot demote group owner")
        }

        members[index].role = .member
    }

    public func promoteMember(id: String) throws {
        guard let index = members.firstIndex(where: { $0.id == id }) else {
            throw GroupAdminError.userNotFound
        }
        if members[index].role != .owner {
            members[index].role = .admin
        }
    }

    public func banMember(id: String, deleteAllMessages: Bool) async throws {
        guard let index = members.firstIndex(where: { $0.id == id }) else {
            throw GroupAdminError.userNotFound
        }

        if members[index].role == .owner {
            throw GroupAdminError.permissionDenied("Cannot ban group owner")
        }

        members[index].role = .banned

        if deleteAllMessages {
            purgedUserSenderIds.insert(id)
        }
    }

    public func approveJoinRequest(id: String) async throws {
        guard let index = pendingRequests.firstIndex(where: { $0.id == id }) else {
            throw GroupAdminError.userNotFound
        }
        let req = pendingRequests.remove(at: index)
        members.append(GroupMemberItem(id: req.id, name: req.name, username: req.username, role: .member))
    }

    public func rejectJoinRequest(id: String) async throws {
        pendingRequests.removeAll { $0.id == id }
    }

    public func joinViaInviteLink(_ link: InviteLinkInfo) throws {
        if link.isExpired {
            throw GroupAdminError.inviteLinkExpired
        }
    }

    @Published public var showingQRCodeForLink: InviteLinkInfo? = nil
    @Published public var banCandidate: GroupMemberItem? = nil
    @Published public var deleteMessagesOnBan: Bool = true
    @Published public var errorMessage: String? = nil

    public var formattedChannelViews: String {
        if channelViewsCount >= 1000 {
            let thousands = Double(channelViewsCount) / 1000.0
            return String(format: "%.1fK", thousands)
        } else {
            return "\(channelViewsCount)"
        }
    }
}

// MARK: - Presentation View

public struct GroupChannelAdminView: View {
    @ObservedObject public var viewModel: GroupChannelAdminViewModel

    public init(viewModel: GroupChannelAdminViewModel) {
        self.viewModel = viewModel
    }

    public var body: some View {
        Form {
            // Channel Broadcast Stats Bar
            Section("Channel Broadcast Stats") {
                HStack {
                    Label("Total Views", systemImage: "eye.fill")
                    Spacer()
                    Text("👁 \(viewModel.formattedChannelViews)")
                        .foregroundStyle(.secondary)
                        .font(.system(size: 15, weight: .medium))
                }
            }

            // Permissions Matrix Section
            Section("Admin Permissions") {
                permissionToggle(title: "Change Group Info", permission: .changeInfo)
                permissionToggle(title: "Post Messages", permission: .postMessages)
                permissionToggle(title: "Edit Messages", permission: .editMessages)
                permissionToggle(title: "Delete Messages", permission: .deleteMessages)
                permissionToggle(title: "Ban Users", permission: .banUsers)
                permissionToggle(title: "Invite Users via Link", permission: .inviteUsers)
                permissionToggle(title: "Pin Messages", permission: .pinMessages)
            }

            // Slow Mode Section
            Section("Slow Mode") {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text("Interval")
                        Spacer()
                        Text(viewModel.slowMode.title)
                            .foregroundStyle(.secondary)
                    }

                    Picker("Slow Mode Interval", selection: $viewModel.slowMode) {
                        ForEach(SlowModeInterval.allCases) { interval in
                            Text(interval.title).tag(interval)
                        }
                    }
                    .pickerStyle(.segmented)
                }
                .padding(.vertical, 4)
            }

            // Join Requests Section
            if !viewModel.pendingRequests.isEmpty {
                Section("Join Requests (\(viewModel.pendingRequests.count))") {
                    ForEach(viewModel.pendingRequests) { req in
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(req.name)
                                    .font(.system(size: 16, weight: .medium))
                                if let username = req.username {
                                    Text("@\(username)")
                                        .font(.system(size: 13))
                                        .foregroundStyle(.secondary)
                                }
                            }
                            Spacer()
                            Button("Approve") {
                                Task {
                                    try? await viewModel.approveJoinRequest(id: req.id)
                                }
                            }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.small)

                            Button("Reject") {
                                Task {
                                    try? await viewModel.rejectJoinRequest(id: req.id)
                                }
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .tint(.red)
                        }
                    }
                }
            }

            // Invite Links Section
            Section("Invite Links") {
                ForEach(viewModel.inviteLinks) { link in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(link.url.absoluteString)
                                .font(.system(size: 14))
                                .lineLimit(1)
                            Text(link.statusDescription)
                                .font(.system(size: 12))
                                .foregroundStyle(link.isExpired ? .red : .secondary)
                        }
                        Spacer()
                        Button {
                            viewModel.showingQRCodeForLink = link
                        } label: {
                            Image(systemName: "qrcode")
                                .font(.system(size: 18))
                        }
                    }
                }
            }

            // Members & Administrators Section
            Section("Members (\(viewModel.members.count))") {
                ForEach(viewModel.members) { member in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text(member.name)
                                    .font(.system(size: 16, weight: .medium))
                                roleBadge(for: member.role)
                            }
                            if let username = member.username {
                                Text("@\(username)")
                                    .font(.system(size: 13))
                                    .foregroundStyle(.secondary)
                            }
                        }
                        Spacer()
                    }
                    .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                        if member.role != .owner {
                            Button(role: .destructive) {
                                viewModel.banCandidate = member
                            } label: {
                                Label("Ban", systemImage: "slash.circle")
                            }

                            if member.role == .admin {
                                Button {
                                    do {
                                        try viewModel.demoteMember(id: member.id)
                                    } catch {
                                        viewModel.errorMessage = error.localizedDescription
                                    }
                                } label: {
                                    Label("Demote", systemImage: "arrow.down")
                                }
                                .tint(.orange)
                            } else if member.role == .member {
                                Button {
                                    try? viewModel.promoteMember(id: member.id)
                                } label: {
                                    Label("Promote", systemImage: "arrow.up")
                                }
                                .tint(.blue)
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("Group & Channel Admin")
        .sheet(item: $viewModel.showingQRCodeForLink) { link in
            NavigationStack {
                VStack(spacing: 24) {
                    Text("Invite Link QR Code")
                        .font(.title2.bold())

                    #if canImport(UIKit)
                    if let qrData = QRCodeGenerator.generateQRCodeData(from: link.url),
                       let uiImage = UIImage(data: qrData) {
                        Image(uiImage: uiImage)
                            .interpolation(.none)
                            .resizable()
                            .scaledToFit()
                            .frame(width: 220, height: 220)
                            .padding()
                            .background(Color.white, in: RoundedRectangle(cornerRadius: 16))
                            .shadow(radius: 6)
                    } else {
                        Rectangle()
                            .fill(Color.secondary.opacity(0.2))
                            .frame(width: 220, height: 220)
                            .overlay(Text("QR Code").font(.headline))
                    }
                    #endif

                    Text(link.url.absoluteString)
                        .font(.system(size: 13, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal)
                }
                .padding()
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done") {
                            viewModel.showingQRCodeForLink = nil
                        }
                    }
                }
            }
        }
        .confirmationDialog(
            "Ban User",
            isPresented: Binding(
                get: { viewModel.banCandidate != nil },
                set: { if !$0 { viewModel.banCandidate = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Ban & Delete All Messages", role: .destructive) {
                if let candidate = viewModel.banCandidate {
                    Task {
                        try? await viewModel.banMember(id: candidate.id, deleteAllMessages: true)
                    }
                }
                viewModel.banCandidate = nil
            }
            Button("Ban User", role: .destructive) {
                if let candidate = viewModel.banCandidate {
                    Task {
                        try? await viewModel.banMember(id: candidate.id, deleteAllMessages: false)
                    }
                }
                viewModel.banCandidate = nil
            }
            Button("Cancel", role: .cancel) {
                viewModel.banCandidate = nil
            }
        }
        .alert("Admin Error", isPresented: Binding(
            get: { viewModel.errorMessage != nil },
            set: { if !$0 { viewModel.errorMessage = nil } }
        )) {
            Button("OK") { viewModel.errorMessage = nil }
        } message: {
            if let err = viewModel.errorMessage {
                Text(err)
            }
        }
    }

    // MARK: - Helpers
    private func permissionToggle(title: String, permission: GroupAdminPermissions) -> some View {
        Toggle(title, isOn: Binding(
            get: { viewModel.permissions.contains(permission) },
            set: { enabled in
                if enabled {
                    viewModel.permissions.insert(permission)
                } else {
                    viewModel.permissions.remove(permission)
                }
            }
        ))
    }

    @ViewBuilder
    private func roleBadge(for role: GroupMemberRole) -> some View {
        switch role {
        case .owner:
            HStack(spacing: 2) {
                Image(systemName: "star.fill")
                    .font(.system(size: 10))
                Text("Owner")
                    .font(.system(size: 11, weight: .bold))
            }
            .foregroundStyle(.yellow)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.yellow.opacity(0.15), in: Capsule())
        case .admin:
            HStack(spacing: 2) {
                Image(systemName: "star.fill")
                    .font(.system(size: 10))
                Text("Admin")
                    .font(.system(size: 11, weight: .semibold))
            }
            .foregroundStyle(.blue)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.blue.opacity(0.15), in: Capsule())
        case .banned:
            Text("Banned")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.red)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Color.red.opacity(0.15), in: Capsule())
        case .restricted:
            Text("Restricted")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.orange)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Color.orange.opacity(0.15), in: Capsule())
        case .member:
            EmptyView()
        }
    }
}

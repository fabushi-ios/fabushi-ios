import Foundation

internal enum GrokMobileSharedRoomModel {
    struct Member: Equatable, Identifiable {
        enum Kind: String {
            case human
            case agent
        }

        let kind: Kind
        let authId: String
        let agentId: String?
        let displayName: String
        let avatarURL: String?

        var id: String {
            [kind.rawValue, authId, agentId ?? ""].joined(separator: ":")
        }
    }

    struct Room: Equatable, Identifiable {
        let roomId: String
        let name: String
        let hostAuthId: String
        let members: [Member]
        let avatarDataURL: String?

        var id: String { roomId }
    }

    struct JoinRequest: Equatable, Identifiable {
        let requestId: String
        let roomId: String
        let requesterAuthId: String
        let requesterName: String
        let requesterAvatarURL: String?

        var id: String { requestId }
    }

    struct TypingUser: Equatable, Identifiable {
        let roomId: String
        let authId: String
        let name: String
        let avatarURL: String?
        let expiresAtMs: Double

        var id: String { [roomId, authId].joined(separator: ":") }
    }

    struct SharingState: Equatable {
        let isEnabled: Bool
        let selfAuthId: String?
        let pendingJoinRequests: [JoinRequest]
        let rooms: [Room]
        let typingUsers: [TypingUser]
    }

    enum InviteResult: Equatable {
        case ok(shareURL: String, expiresAtMs: Double, roomId: String)
        case error(String)
    }

    struct Snapshot: Equatable {
        let state: SharingState
        let room: Room?
        let isHost: Bool
        let selfAgentIds: [String]
        let candidates: [MobileBotSummary]
        let requests: [JoinRequest]
    }

    struct LifecycleFence: Equatable {
        let accountScopeKey: String
        let agentId: String
        let generation: Int
    }

    static func projectSharingState(_ value: Any) -> SharingState? {
        guard let row = value as? [String: Any],
              let isEnabled = row["isEnabled"] as? Bool,
              let rawRequests = row["pendingJoinRequests"] as? [Any],
              let rawRooms = row["rooms"] as? [Any],
              let rawTyping = row["typingUsers"] as? [Any]
        else { return nil }

        let selfAuthId: String?
        if row["selfAuthId"] is NSNull || row["selfAuthId"] == nil {
            selfAuthId = nil
        } else if let value = nonEmptyString(row["selfAuthId"]) {
            selfAuthId = value
        } else {
            return nil
        }

        let requests = rawRequests.compactMap(projectJoinRequest)
        let rooms = rawRooms.compactMap(projectRoom)
        let typing = rawTyping.compactMap(projectTypingUser)
        guard requests.count == rawRequests.count,
              rooms.count == rawRooms.count,
              typing.count == rawTyping.count
        else { return nil }

        return SharingState(
            isEnabled: isEnabled,
            selfAuthId: selfAuthId,
            pendingJoinRequests: requests,
            rooms: rooms,
            typingUsers: typing
        )
    }

    static func projectInviteResult(_ value: Any) -> InviteResult? {
        guard let row = value as? [String: Any],
              let status = row["status"] as? String
        else { return nil }
        if status == "error" {
            guard let message = nonEmptyString(row["message"]) else { return nil }
            return .error(message)
        }
        guard status == "ok",
              let shareURL = nonEmptyString(row["shareUrl"]),
              let roomId = nonEmptyString(row["roomId"]),
              let expiresAtMs = finiteDouble(row["expiresAtMs"])
        else { return nil }
        return .ok(shareURL: shareURL, expiresAtMs: expiresAtMs, roomId: roomId)
    }

    static func roomId(for agent: MobileBotSummary, in state: SharingState) -> String? {
        if agent.isSharedRoom,
           let exact = state.rooms.first(where: { $0.roomId == agent.id }) {
            return exact.roomId
        }
        return state.rooms.first(where: { room in
            room.members.contains { member in
                member.kind == .agent && member.agentId == agent.id
            }
        })?.roomId
    }

    static func snapshot(
        agent: MobileBotSummary,
        roster: [MobileBotSummary],
        state: SharingState
    ) -> Snapshot {
        let roomId = roomId(for: agent, in: state)
        let room = roomId.flatMap { id in state.rooms.first(where: { $0.roomId == id }) }
        let isHost = room != nil
            && state.selfAuthId != nil
            && room?.hostAuthId == state.selfAuthId
        let selfAgentIds = room?.members.compactMap { member -> String? in
            guard member.kind == .agent,
                  member.authId == state.selfAuthId
            else { return nil }
            return member.agentId
        } ?? []
        let memberIds = Set(selfAgentIds)
        let candidates = roster.filter {
            !$0.isGroup
                && !$0.isSharedRoom
                && !memberIds.contains($0.id)
        }
        let requests = roomId.map { id in
            state.pendingJoinRequests.filter { $0.roomId == id }
        } ?? []
        return Snapshot(
            state: state,
            room: room,
            isHost: isHost,
            selfAgentIds: selfAgentIds,
            candidates: candidates,
            requests: requests
        )
    }

    static func accepts(
        _ fence: LifecycleFence,
        accountScopeKey: String,
        agentId: String,
        generation: Int
    ) -> Bool {
        fence.accountScopeKey == accountScopeKey
            && fence.agentId == agentId
            && fence.generation == generation
    }

    private static func projectMember(_ value: Any) -> Member? {
        guard let row = value as? [String: Any],
              let kindValue = nonEmptyString(row["kind"]),
              let kind = Member.Kind(rawValue: kindValue),
              let authId = nonEmptyString(row["authId"])
        else { return nil }

        let agentId: String?
        if row["agentId"] == nil || row["agentId"] is NSNull {
            agentId = nil
        } else if let value = nonEmptyString(row["agentId"]) {
            agentId = value
        } else {
            return nil
        }
        let displayName = (row["displayName"] as? String) ?? ""
        guard kind != .human || !displayName.isEmpty else { return nil }
        let avatar = nonEmptyString(row["avatarDataUrl"])
            ?? nonEmptyString(row["avatarUrl"])
        return Member(
            kind: kind,
            authId: authId,
            agentId: agentId,
            displayName: displayName,
            avatarURL: avatar
        )
    }

    private static func projectRoom(_ value: Any) -> Room? {
        guard let row = value as? [String: Any],
              let roomId = nonEmptyString(row["roomId"]),
              let name = nonEmptyString(row["name"]),
              let hostAuthId = nonEmptyString(row["hostAuthId"]),
              let rawMembers = row["members"] as? [Any]
        else { return nil }
        let members = rawMembers.compactMap(projectMember)
        guard members.count == rawMembers.count else { return nil }
        return Room(
            roomId: roomId,
            name: name,
            hostAuthId: hostAuthId,
            members: members,
            avatarDataURL: nonEmptyString(row["avatarDataUrl"])
        )
    }

    private static func projectJoinRequest(_ value: Any) -> JoinRequest? {
        guard let row = value as? [String: Any],
              let requestId = nonEmptyString(row["requestId"]),
              let roomId = nonEmptyString(row["roomId"]),
              let requesterAuthId = nonEmptyString(row["requesterAuthId"]),
              let requesterName = nonEmptyString(row["requesterName"])
        else { return nil }
        return JoinRequest(
            requestId: requestId,
            roomId: roomId,
            requesterAuthId: requesterAuthId,
            requesterName: requesterName,
            requesterAvatarURL: nonEmptyString(row["requesterAvatarUrl"])
        )
    }

    private static func projectTypingUser(_ value: Any) -> TypingUser? {
        guard let row = value as? [String: Any],
              let roomId = nonEmptyString(row["roomId"]),
              let authId = nonEmptyString(row["authId"]),
              let name = nonEmptyString(row["name"]),
              let expiresAtMs = finiteDouble(row["expiresAtMs"])
        else { return nil }
        return TypingUser(
            roomId: roomId,
            authId: authId,
            name: name,
            avatarURL: nonEmptyString(row["avatarUrl"]),
            expiresAtMs: expiresAtMs
        )
    }

    private static func nonEmptyString(_ value: Any?) -> String? {
        guard let value = value as? String, !value.isEmpty else { return nil }
        return value
    }

    private static func finiteDouble(_ value: Any?) -> Double? {
        let number: Double?
        if let value = value as? Double {
            number = value
        } else if let value = value as? NSNumber {
            number = value.doubleValue
        } else {
            number = nil
        }
        guard let number, number.isFinite else { return nil }
        return number
    }
}

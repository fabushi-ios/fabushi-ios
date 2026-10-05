import Foundation

internal enum GrokMobileGroupMembersModel {
    struct MutationFence: Equatable {
        let accountScopeKey: String
        let generation: Int
    }

    static let maximumMembers = 6

    static func accepts(
        _ fence: MutationFence,
        accountScopeKey: String,
        generation: Int
    ) -> Bool {
        fence.accountScopeKey == accountScopeKey && fence.generation == generation
    }

    static func group(id: String, fallback: MobileBotSummary, roster: [MobileBotSummary]) -> MobileBotSummary? {
        let candidate = roster.first(where: { $0.id == id }) ?? fallback
        return candidate.isGroup && !candidate.isSharedRoom ? candidate : nil
    }

    static func members(group: MobileBotSummary, roster: [MobileBotSummary]) -> [MobileBotSummary] {
        let byId = Dictionary(uniqueKeysWithValues: roster.map { ($0.id, $0) })
        return group.memberIds.compactMap { byId[$0] }.filter { !$0.isGroup }
    }

    static func candidates(group: MobileBotSummary, roster: [MobileBotSummary]) -> [MobileBotSummary] {
        let memberIds = Set(group.memberIds)
        return roster.filter {
            !$0.isGroup && $0.id != group.id && !memberIds.contains($0.id)
        }
    }

    static func canAdd(group: MobileBotSummary, roster: [MobileBotSummary], pending: Bool) -> Bool {
        !pending && group.memberIds.count < maximumMembers && !candidates(group: group, roster: roster).isEmpty
    }

    static func canRemove(group: MobileBotSummary, pending: Bool) -> Bool {
        !pending && group.memberIds.count > 1
    }

    static func adding(memberId: String, to group: MobileBotSummary, roster: [MobileBotSummary]) -> [String]? {
        guard group.isGroup,
              !group.isSharedRoom,
              group.memberIds.count < maximumMembers,
              candidates(group: group, roster: roster).contains(where: { $0.id == memberId })
        else { return nil }
        return group.memberIds + [memberId]
    }

    static func removing(memberId: String, from group: MobileBotSummary) -> [String]? {
        guard group.isGroup,
              !group.isSharedRoom,
              group.memberIds.count > 1,
              group.memberIds.contains(memberId)
        else { return nil }
        let next = group.memberIds.filter { $0 != memberId }
        return next.isEmpty ? nil : next
    }
}

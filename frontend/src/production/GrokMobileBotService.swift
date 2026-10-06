import Foundation

/// Renderer-side Bot/Agent roster adapter.
///
/// The SwiftUI shell owns presentation state only. Individual Bots come from
/// `bot.list`; Bot/Agent groups come from the same canonical Rust Host through
/// `group.list`. The adapter projects both into one native roster without
/// creating a second group store on iOS.
@MainActor
struct GrokMobileBotService {
    static let groupMaximumMembers = 6

    let bridge: IOSPreloadBridge

    func loadBots() async -> [MobileBotSummary] {
        let canonical = (try? await GlobalDharmaMiniAppBridge(bridge: bridge).installedMiniAppBots()) ?? []
        let installedBots = canonical.map {
            MobileBotSummary(
                id: $0.id,
                name: $0.name,
                description: $0.description,
                miniAppId: $0.miniAppId,
                menuButtonText: $0.menuButtonText
            )
        }

        let surfaceBots = await loadIndividualBots()
        let groups = await loadGroups()
        return Self.mergeBots(installedBots, surfaceBots + groups)
    }

    func createBot(name: String, description: String) async throws -> [MobileBotSummary] {
        let requestId = "ios-mobile-bot-create-\(UUID().uuidString.lowercased())"
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": [
                    "type": "bot.create",
                    "requestId": requestId,
                    "name": String(name.prefix(72)),
                    "description": String(description.prefix(240)),
                ],
            ]
        )
        return await loadBots()
    }

    func renameBot(id: String, name: String) async throws -> [MobileBotSummary] {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "Bot 名称不能为空"]
            )
        }
        return try await executeBotMutation(
            Self.renameCommand(
                id: id,
                name: trimmed,
                requestId: "ios-mobile-bot-rename-\(UUID().uuidString.lowercased())"
            )
        )
    }

    func duplicateBot(id: String) async throws -> [MobileBotSummary] {
        try await executeBotMutation(
            Self.duplicateCommand(
                id: id,
                requestId: "ios-mobile-bot-clone-\(UUID().uuidString.lowercased())"
            )
        )
    }

    func deleteBot(id: String) async throws -> [MobileBotSummary] {
        try await executeBotMutation(
            Self.deleteCommand(
                id: id,
                requestId: "ios-mobile-bot-delete-\(UUID().uuidString.lowercased())"
            )
        )
    }

    func updateAgentProfile(
        id: String,
        isGroup: Bool,
        name: String,
        title: String?,
        description: String
    ) async throws -> [MobileBotSummary] {
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.agentProfileUpdateCommand(
                    id: id,
                    isGroup: isGroup,
                    name: name,
                    title: title,
                    description: description,
                    requestId: "ios-mobile-agent-settings-profile-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
        try Task.checkCancellation()
        let updated = await loadBots()
        guard updated.contains(where: { $0.id == id }) else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 3,
                userInfo: [NSLocalizedDescriptionKey: "更新后无法从 Host roster 重新读取该 Agent"]
            )
        }
        return updated
    }

    func setAgentNotifyOnUpdates(id: String, isEnabled: Bool) async throws -> [MobileBotSummary] {
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.agentNotificationUpdateCommand(
                    id: id,
                    isEnabled: isEnabled,
                    requestId: "ios-mobile-agent-settings-notify-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
        try Task.checkCancellation()
        let updated = await loadBots()
        guard updated.contains(where: { $0.id == id }) else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 4,
                userInfo: [NSLocalizedDescriptionKey: "更新通知设置后无法从 Host roster 重新读取该 Agent"]
            )
        }
        return updated
    }

    func updateGroupMembers(groupId: String, memberIds: [String]) async throws -> [MobileBotSummary] {
        let normalized = memberIds.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        guard normalized.count == memberIds.count,
              normalized.allSatisfy({ !$0.isEmpty }),
              Set(normalized).count == normalized.count,
              (1...Self.groupMaximumMembers).contains(normalized.count)
        else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 2,
                userInfo: [NSLocalizedDescriptionKey: "Bot 群组必须包含 1 到 6 个不同成员"]
            )
        }
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.groupUpdateCommand(
                    id: groupId,
                    memberIds: normalized,
                    requestId: "ios-mobile-group-members-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
        try Task.checkCancellation()
        return await loadBots()
    }

    static func agentProfileUpdateCommand(
        id: String,
        isGroup: Bool,
        name: String,
        title: String?,
        description: String,
        requestId: String
    ) -> [String: Any] {
        var command: [String: Any] = [
            "type": isGroup ? "group.update" : "bot.update",
            "requestId": requestId,
            "id": id,
            "name": name,
            "description": description,
        ]
        if !isGroup, let title {
            command["title"] = title
        }
        return command
    }

    static func agentNotificationUpdateCommand(
        id: String,
        isEnabled: Bool,
        requestId: String
    ) -> [String: Any] {
        [
            "type": "bot.update",
            "requestId": requestId,
            "id": id,
            "notifyOnUpdates": isEnabled,
        ]
    }

    static func renameCommand(id: String, name: String, requestId: String) -> [String: Any] {
        [
            "type": "bot.update",
            "requestId": requestId,
            "id": id,
            "name": String(name.prefix(72)),
        ]
    }

    static func duplicateCommand(id: String, requestId: String) -> [String: Any] {
        [
            "type": "bot.clone",
            "requestId": requestId,
            "id": id,
        ]
    }

    static func deleteCommand(id: String, requestId: String) -> [String: Any] {
        [
            "type": "bot.delete",
            "requestId": requestId,
            "id": id,
        ]
    }

    static func groupUpdateCommand(id: String, memberIds: [String], requestId: String) -> [String: Any] {
        [
            "type": "group.update",
            "requestId": requestId,
            "id": id,
            "memberIds": memberIds,
        ]
    }

    private func executeBotMutation(_ command: [String: Any]) async throws -> [MobileBotSummary] {
        _ = try await bridge.request(method: "feature.execute", params: ["command": command])
        return await loadBots()
    }

    private func loadIndividualBots() async -> [MobileBotSummary] {
        let requestId = "ios-mobile-bot-list-\(UUID().uuidString.lowercased())"
        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: ["command": ["type": "bot.list", "requestId": requestId]]
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 2_560
            ) { event in
                event["type"] as? String == "bot.listed"
            }
            if let event = result.value as? [String: Any],
               let rows = event["bots"] as? [[String: Any]]
            {
                return rows.compactMap(Self.parseBot)
                    .filter { $0.id != "mahayana-assistant" }
            }
        } catch {
            return []
        }
        return []
    }

    private func loadGroups() async -> [MobileBotSummary] {
        let requestId = "ios-mobile-group-list-\(UUID().uuidString.lowercased())"
        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: ["command": ["type": "group.list", "requestId": requestId]]
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 2_560
            ) { event in
                event["type"] as? String == "group.listed"
            }
            if let event = result.value as? [String: Any],
               let rows = event["groups"] as? [[String: Any]]
            {
                return rows.compactMap(Self.parseGroup)
            }
        } catch {
            return []
        }
        return []
    }

    static func mergeBots(_ installed: [MobileBotSummary], _ surface: [MobileBotSummary]) -> [MobileBotSummary] {
        var byId: [String: MobileBotSummary] = [:]
        for bot in surface { byId[bot.id] = bot }
        for installedBot in installed {
            if let canonical = byId[installedBot.id] {
                byId[installedBot.id] = MobileBotSummary(
                    id: installedBot.id,
                    name: installedBot.name,
                    description: installedBot.description,
                    title: canonical.title,
                    notifyOnUpdatesEnabled: canonical.notifyOnUpdatesEnabled,
                    miniAppId: installedBot.miniAppId ?? canonical.miniAppId,
                    menuButtonText: installedBot.menuButtonText ?? canonical.menuButtonText,
                    isGroup: canonical.isGroup,
                    memberIds: canonical.memberIds,
                    isSharedRoom: canonical.isSharedRoom
                )
            } else {
                byId[installedBot.id] = installedBot
            }
        }
        return byId.values.sorted {
            if ($0.miniAppId != nil) != ($1.miniAppId != nil) {
                return $0.miniAppId != nil
            }
            if $0.isGroup != $1.isGroup {
                return !$0.isGroup
            }
            return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
    }

    static func parseBot(_ row: [String: Any]) -> MobileBotSummary? {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        let explicitMiniAppId = (row["miniAppId"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let miniAppId = explicitMiniAppId?.isEmpty == false
            ? explicitMiniAppId
            : (id == "global-dharma-bot" ? GlobalDharmaMiniAppBridge.globalDharmaId : nil)
        let menuText = (row["menuButtonText"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return MobileBotSummary(
            id: id,
            name: (row["name"] as? String) ?? (row["displayName"] as? String) ?? id,
            description: row["description"] as? String ?? "",
            title: row["title"] as? String,
            notifyOnUpdatesEnabled: row["notifyOnUpdates"] as? Bool ?? false,
            miniAppId: miniAppId,
            menuButtonText: menuText?.isEmpty == false ? menuText : (miniAppId == nil ? nil : "打开应用")
        )
    }

    static func parseGroup(_ row: [String: Any]) -> MobileBotSummary? {
        guard let id = row["id"] as? String,
              !id.isEmpty,
              let memberIds = row["memberIds"] as? [String],
              !memberIds.isEmpty,
              memberIds.count <= Self.groupMaximumMembers,
              memberIds.allSatisfy({ !$0.isEmpty }),
              Set(memberIds).count == memberIds.count
        else { return nil }
        return MobileBotSummary(
            id: id,
            name: (row["name"] as? String) ?? id,
            description: row["description"] as? String ?? "",
            isGroup: true,
            memberIds: memberIds,
            isSharedRoom: false
        )
    }
}

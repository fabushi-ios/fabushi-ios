import Foundation

/// Renderer-side Bot/Agent roster adapter.
///
/// The SwiftUI shell owns presentation state only. Individual Bots come from
/// `bot.list`; Bot/Agent groups come from the same canonical Rust Host through
/// `group.list`. The adapter projects both into one native roster without
/// creating a second group store on iOS.
@MainActor
struct MobileAgentAsyncTask: Identifiable, Equatable {
    let id: String
    let kind: String
    let label: String
    let detail: String?
    let resourceId: String?
}

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

    func loadOnboardingAgents() async throws -> [MobileBotSummary] {
        try await loadIndividualBotsStrict()
    }

    static func humanHandoffPrompt(
        conversationTitle: String,
        transcriptLines: [String]
    ) -> String? {
        let normalizedTitle = conversationTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        let recent = transcriptLines
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .suffix(20)
        guard !recent.isEmpty else { return nil }
        let title = normalizedTitle.isEmpty ? "Human conversation" : normalizedTitle
        return """
        Continue from this Human conversation and help with the user's explicit handoff request. Preserve the Human/Agent distinction and use tools or artifacts when useful.

        Human conversation: \(title)

        \(recent.joined(separator: "\n"))
        """
    }

    static func humanHandoffCommand(
        requestId: String,
        agentId: String,
        humanConversationId: String,
        prompt: String
    ) -> [String: Any]? {
        let requestId = requestId.trimmingCharacters(in: .whitespacesAndNewlines)
        let agentId = agentId.trimmingCharacters(in: .whitespacesAndNewlines)
        let humanConversationId = humanConversationId.trimmingCharacters(in: .whitespacesAndNewlines)
        let prompt = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !requestId.isEmpty, !agentId.isEmpty, !humanConversationId.isEmpty, !prompt.isEmpty else {
            return nil
        }
        return [
            "type": "chat.handoffHuman",
            "requestId": requestId,
            "agentId": agentId,
            "humanConversationId": humanConversationId,
            "text": prompt,
        ]
    }

    func handoffHumanConversation(
        agentId: String,
        humanConversationId: String,
        prompt: String,
        requestId: String = "ios-human-handoff-\(UUID().uuidString.lowercased())"
    ) async throws {
        guard let command = Self.humanHandoffCommand(
            requestId: requestId,
            agentId: agentId,
            humanConversationId: humanConversationId,
            prompt: prompt
        ) else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 30,
                userInfo: [NSLocalizedDescriptionKey: "Human handoff requires an Agent, conversation, and transcript."]
            )
        }
        let accepted = try await bridge.request(
            method: "feature.execute",
            params: ["command": command]
        )
        guard let object = accepted.value as? [String: Any],
              let operationId = object["operationId"] as? String,
              !operationId.isEmpty
        else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 31,
                userInfo: [NSLocalizedDescriptionKey: "Host did not bind the Human handoff to an Agent operation."]
            )
        }

        let terminal = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 180_000
        ) { event in
            guard event["operationId"] as? String == operationId,
                  let type = event["type"] as? String
            else { return false }
            return type == "operation.completed"
                || type == "operation.failed"
                || type == "operation.interrupted"
        }
        guard let event = terminal.value as? [String: Any],
              event["type"] as? String == "operation.completed"
        else {
            let event = terminal.value as? [String: Any]
            let reason = (event?["error"] as? String)
                ?? (event?["reason"] as? String)
                ?? "Agent handoff did not complete."
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 32,
                userInfo: [NSLocalizedDescriptionKey: reason]
            )
        }
    }

    func waitForOnboardingComputer(
        timeoutNanoseconds: UInt64 = 60_000_000_000,
        retryNanoseconds: UInt64 = 2_500_000_000
    ) async throws -> [MobileBotSummary] {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .nanoseconds(Int64(clamping: timeoutNanoseconds)))
        var lastError: Error?
        repeat {
            try Task.checkCancellation()
            do {
                return try await loadOnboardingAgents()
            } catch {
                lastError = error
                guard clock.now < deadline else { break }
                try await Task.sleep(nanoseconds: retryNanoseconds)
            }
        } while clock.now < deadline
        throw lastError ?? NSError(
            domain: "Fabushi.MobileSignedInOnboarding",
            code: 2,
            userInfo: [NSLocalizedDescriptionKey: "The Fabushi computer did not become ready in time."]
        )
    }

    func loadCanonicalRoster() async throws -> [MobileBotSummary] {
        let individualBots = try await loadIndividualBotsStrict()
        let groupBots = try await loadGroupsStrict()
        let surface = individualBots + groupBots
        let installed = (try? await GlobalDharmaMiniAppBridge(bridge: bridge).installedMiniAppBots()) ?? []
        let installedBots = installed.map {
            MobileBotSummary(
                id: $0.id,
                name: $0.name,
                description: $0.description,
                miniAppId: $0.miniAppId,
                menuButtonText: $0.menuButtonText
            )
        }
        return Self.mergeBots(installedBots, surface)
    }

    func createBot(
        name: String,
        description: String,
        avatarShape: String,
        avatarColor: String,
        requestId: String? = nil
    ) async throws -> [MobileBotSummary] {
        let command = try Self.createCommand(
            name: name,
            description: description,
            avatarShape: avatarShape,
            avatarColor: avatarColor,
            requestId: requestId ?? "ios-mobile-bot-create-\(UUID().uuidString.lowercased())"
        )
        _ = try await bridge.request(
            method: "feature.execute",
            params: ["command": command]
        )
        return await loadBots()
    }

    func createOnboardingBot(
        name: String,
        description: String,
        avatarShape: String,
        avatarColor: String,
        requestId: String
    ) async throws -> [MobileBotSummary] {
        // Desktop's coordinator-level origin/template/kickstart metadata has no
        // field in the canonical iOS Host bot.create protocol. The native
        // replacement applies the selected template into these concrete Bot
        // fields before this call and performs the computer-readiness/kickstart
        // responsibility before bot.create rather than sending ignored JSON.
        let command = try Self.createCommand(
            name: name,
            description: description,
            avatarShape: avatarShape,
            avatarColor: avatarColor,
            requestId: requestId
        )
        _ = try await bridge.request(
            method: "feature.execute",
            params: ["command": command]
        )
        return try await loadOnboardingAgents()
    }

    static func createCommand(
        name: String,
        description: String,
        avatarShape: String,
        avatarColor: String,
        requestId: String
    ) throws -> [String: Any] {
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedDescription = description.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedName.isEmpty else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 9,
                userInfo: [NSLocalizedDescriptionKey: "Bot 名称不能为空"]
            )
        }
        guard AvatarImagePolicy.shapes.contains(avatarShape),
              AvatarImagePolicy.colors.contains(where: { $0.id == avatarColor })
        else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 10,
                userInfo: [NSLocalizedDescriptionKey: "Agent 角色无效"]
            )
        }
        return [
            "type": "bot.create",
            "requestId": requestId,
            "name": String(trimmedName.prefix(72)),
            "description": String(trimmedDescription.prefix(2_000)),
            "avatarShape": avatarShape,
            "avatarColor": avatarColor,
        ]
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
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.renameCommand(
                    id: id,
                    name: trimmed,
                    requestId: "ios-mobile-bot-rename-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
        return await loadBots()
    }

    func duplicateBot(id: String) async throws -> [MobileBotSummary] {
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.duplicateCommand(
                    id: id,
                    requestId: "ios-mobile-bot-clone-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
        return await loadBots()
    }

    func deleteBot(id: String) async throws -> [MobileBotSummary] {
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.deleteCommand(
                    id: id,
                    requestId: "ios-mobile-bot-delete-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
        return await loadBots()
    }

    func loadPinnedBotIds() async throws -> [String] {
        let result = try await bridge.request(method: "getHostPinnedAgents")
        guard let ids = Self.canonicalPinnedBotIds(from: result.value) else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 4,
                userInfo: [NSLocalizedDescriptionKey: "Host returned malformed pinned Bot state"]
            )
        }
        return ids
    }

    func setPinnedBotIds(_ ids: [String]) async throws -> [String] {
        let requested = Self.canonicalPinnedBotIds(from: ids as [Any]) ?? []
        let result = try await bridge.request(
            method: "setHostPinnedAgents",
            params: ["pinnedAgentIds": requested]
        )
        guard let authoritative = Self.canonicalPinnedBotIds(from: result.value) else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 5,
                userInfo: [NSLocalizedDescriptionKey: "Host returned malformed pinned Bot state"]
            )
        }
        return authoritative
    }

    static func canonicalPinnedBotIds(from value: Any) -> [String]? {
        guard let rows = value as? [Any] else { return nil }
        var seen = Set<String>()
        var result: [String] = []
        for row in rows {
            guard let id = row as? String, !id.isEmpty else { return nil }
            if seen.insert(id).inserted {
                result.append(id)
            }
        }
        return result
    }

    static func movedPinnedBotIds(_ ids: [String], movedId: String, offset: Int) -> [String] {
        guard offset != 0,
              let source = ids.firstIndex(of: movedId)
        else { return ids }
        let target = source + offset
        guard ids.indices.contains(target) else { return ids }
        var next = ids
        next.swapAt(source, target)
        return next
    }

    func setBotHiddenMutation(id: String, hidden: Bool) async throws {
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.setHiddenCommand(
                    id: id,
                    hidden: hidden,
                    requestId: "ios-mobile-bot-hidden-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
    }

    func setBotHidden(id: String, hidden: Bool) async throws -> [MobileBotSummary] {
        try await setBotHiddenMutation(id: id, hidden: hidden)
        return await loadBots()
    }

    func setBotUnread(id: String, unread: Bool) async throws -> [MobileBotSummary] {
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.setUnreadCommand(
                    id: id,
                    unread: unread,
                    requestId: "ios-mobile-bot-unread-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
        return await loadBots()
    }

    func asyncTasks(agentId: String) async throws -> [MobileAgentAsyncTask] {
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": [
                    "type": "asyncTask.list",
                    "requestId": "ios-mobile-async-task-list-\(UUID().uuidString.lowercased())",
                    "agentId": agentId,
                ],
            ]
        )
        let result = try await bridge.receiveFeatureEvent(deadlineMilliseconds: 2_560) { event in
            event["type"] as? String == "asyncTask.listed"
                && event["agentId"] as? String == agentId
        }
        guard let event = result.value as? [String: Any],
              let rows = event["tasks"] as? [[String: Any]]
        else { return [] }
        return rows.compactMap(Self.parseAsyncTask)
    }

    static func parseAsyncTask(_ row: [String: Any]) -> MobileAgentAsyncTask? {
        guard let id = row["id"] as? String, !id.isEmpty,
              let kind = row["kind"] as? String, !kind.isEmpty,
              let label = row["label"] as? String, !label.isEmpty
        else { return nil }
        return MobileAgentAsyncTask(
            id: id,
            kind: kind,
            label: label,
            detail: row["detail"] as? String,
            resourceId: row["resourceId"] as? String
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

    func updateAgentAvatar(
        id: String,
        isGroup: Bool,
        avatarDataURL: String? = nil,
        clearAvatar: Bool = false,
        avatarShape: String? = nil,
        avatarColor: String? = nil
    ) async throws -> [MobileBotSummary] {
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": Self.avatarUpdateCommand(
                    id: id,
                    isGroup: isGroup,
                    avatarDataURL: avatarDataURL,
                    clearAvatar: clearAvatar,
                    avatarShape: avatarShape,
                    avatarColor: avatarColor,
                    requestId: "ios-mobile-avatar-\(UUID().uuidString.lowercased())"
                ),
            ]
        )
        try Task.checkCancellation()
        let updated = await loadBots()
        guard updated.contains(where: { $0.id == id }) else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 8,
                userInfo: [NSLocalizedDescriptionKey: "头像更新后无法从 Host roster 重新读取该 Agent"]
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

    static func avatarUpdateCommand(
        id: String,
        isGroup: Bool,
        avatarDataURL: String?,
        clearAvatar: Bool,
        avatarShape: String?,
        avatarColor: String?,
        requestId: String
    ) -> [String: Any] {
        var command: [String: Any] = [
            "type": isGroup ? "group.update" : "bot.update",
            "requestId": requestId,
            "id": id,
        ]
        if let avatarDataURL {
            command["avatar"] = avatarDataURL
        } else if clearAvatar {
            command["avatar"] = ""
        }
        if let avatarShape { command["avatarShape"] = avatarShape }
        if let avatarColor { command["avatarColor"] = avatarColor }
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

    static func setHiddenCommand(
        id: String,
        hidden: Bool,
        requestId: String
    ) -> [String: Any] {
        [
            "type": "bot.setHidden",
            "requestId": requestId,
            "id": id,
            "hidden": hidden,
        ]
    }

    static func setUnreadCommand(
        id: String,
        unread: Bool,
        requestId: String
    ) -> [String: Any] {
        [
            "type": "bot.update",
            "requestId": requestId,
            "id": id,
            "unread": unread,
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

    private func loadIndividualBotsStrict() async throws -> [MobileBotSummary] {
        let requestId = "ios-mobile-bot-list-\(UUID().uuidString.lowercased())"
        _ = try await bridge.request(
            method: "feature.execute",
            params: ["command": ["type": "bot.list", "requestId": requestId]]
        )
        let result = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 2_560
        ) { event in
            event["type"] as? String == "bot.listed"
        }
        guard let event = result.value as? [String: Any],
              let rows = event["bots"] as? [[String: Any]]
        else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 20,
                userInfo: [NSLocalizedDescriptionKey: "Host returned a malformed Bot roster"]
            )
        }
        let parsed = rows.compactMap(Self.parseBot)
            .filter { $0.id != "mahayana-assistant" }
        guard parsed.count == rows.filter({ ($0["id"] as? String) != "mahayana-assistant" }).count else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 21,
                userInfo: [NSLocalizedDescriptionKey: "Host Bot roster contained malformed rows"]
            )
        }
        return parsed
    }

    private func loadGroupsStrict() async throws -> [MobileBotSummary] {
        let requestId = "ios-mobile-group-list-\(UUID().uuidString.lowercased())"
        _ = try await bridge.request(
            method: "feature.execute",
            params: ["command": ["type": "group.list", "requestId": requestId]]
        )
        let result = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 2_560
        ) { event in
            event["type"] as? String == "group.listed"
        }
        guard let event = result.value as? [String: Any],
              let rows = event["groups"] as? [[String: Any]]
        else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 22,
                userInfo: [NSLocalizedDescriptionKey: "Host returned a malformed group roster"]
            )
        }
        let parsed = rows.compactMap(Self.parseGroup)
        guard parsed.count == rows.count else {
            throw NSError(
                domain: "Fabushi.GrokMobileBotService",
                code: 23,
                userInfo: [NSLocalizedDescriptionKey: "Host group roster contained malformed rows"]
            )
        }
        return parsed
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
                    avatarDataURL: canonical.avatarDataURL,
                    avatarShape: canonical.avatarShape,
                    avatarColor: canonical.avatarColor,
                    notifyOnUpdatesEnabled: canonical.notifyOnUpdatesEnabled,
                    hidden: canonical.hidden,
                    unread: canonical.unread,
                    conversationId: canonical.conversationId,
                    lastEntry: canonical.lastEntry,
                    lastMessageId: canonical.lastMessageId,
                    lastMessagePreview: canonical.lastMessagePreview,
                    updatedAtMs: canonical.updatedAtMs,
                    isComposingMessage: canonical.isComposingMessage,
                    waitingReason: canonical.waitingReason,
                    isRunning: canonical.isRunning,
                    avatarState: canonical.avatarState,
                    draftPrompt: canonical.draftPrompt,
                    miniAppId: installedBot.miniAppId ?? canonical.miniAppId,
                    menuButtonText: installedBot.menuButtonText ?? canonical.menuButtonText,
                    isGroup: canonical.isGroup,
                    memberIds: canonical.memberIds,
                    conversationPartnerIds: canonical.conversationPartnerIds,
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

    static func parseLastEntry(_ value: Any?) -> MobileBotLastEntry? {
        guard let row = value as? [String: Any] else { return nil }
        if row["kind"] as? String == "text", let text = row["text"] as? String {
            return .text(text)
        }
        if row["kind"] as? String == "attachment",
           let count = row["count"] as? Int,
           count > 0,
           let rawKinds = row["kinds"] as? [String: Any]
        {
            var kinds: [String: Int] = [:]
            for (kind, rawCount) in rawKinds {
                guard !kind.isEmpty, let amount = rawCount as? Int, amount > 0 else { return nil }
                kinds[kind] = amount
            }
            return .attachment(count: count, kinds: kinds)
        }
        if row["kind"] as? String == "link",
           let url = row["url"] as? String,
           !url.isEmpty
        {
            return .link(url)
        }

        if let content = row["content"] as? String { return .text(content) }
        if let text = row["text"] as? String { return .text(text) }
        if let message = row["message"] as? [String: Any],
           let content = message["content"] as? String
        {
            return .text(content)
        }
        return nil
    }

    static func derivedLastMessage(
        lastEntry: MobileBotLastEntry?,
        fallback: Any?
    ) -> String? {
        switch lastEntry {
        case .text(let text):
            return text
        case .link(let url):
            if let fallback = fallback as? String { return fallback }
            return "Sent a link · \(url)"
        case .attachment(let count, _):
            if let fallback = fallback as? String { return fallback }
            return count == 1 ? "Sent 1 file" : "Sent \(count) files"
        case nil:
            return fallback as? String
        }
    }

    static func int64Value(_ value: Any?) -> Int64? {
        if let value = value as? Int64 { return value }
        if let value = value as? Int { return Int64(value) }
        if let value = value as? Double, value.isFinite { return Int64(value) }
        if let value = value as? NSNumber { return value.int64Value }
        return nil
    }

    static func avatarState(
        currentActivity: Any?,
        awaitingUserResponsePresent: Bool,
        isComposingMessage: Bool,
        isRunning: Bool
    ) -> MobileAgentAvatarState {
        if awaitingUserResponsePresent { return .idle }
        if let activity = currentActivity as? [String: Any] {
            let kind = activity["kind"] as? String
            let tool = activity["tool"] as? String
            let verb = activity["verb"] as? String

            if kind == "thinking" { return .thinking }
            if kind == "tool", tool == "SendToAgent" { return .sending }

            switch verb {
            case "thinking": return .thinking
            case "searching", "browsing", "reading", "connecting": return .searching
            case "writing", "coding", "running-commands", "on-its-computer",
                 "on-your-computer", "working":
                return .working
            case "generating": return .loading
            case "messaging", "waiting": return .orbit
            case "sending": return .sending
            default: break
            }

            if let tool {
                if tool == "WebSearch" || tool == "WebFetch" || tool.hasPrefix("browser_") {
                    return .searching
                }
                if tool == "GenerateImage" { return .loading }
                if tool == "SendToAgent" || tool == "UpdateAgent" { return .sending }
                if tool == "Task" || tool == "Await" || tool == "CheckSubagent" {
                    return .orbit
                }
                return .working
            }
        }
        if isComposingMessage { return .thinking }
        if isRunning { return .working }
        return .idle
    }

    static func summaryProjection(_ row: [String: Any]) -> (
        lastEntry: MobileBotLastEntry?,
        lastMessageId: String?,
        lastMessagePreview: String?,
        updatedAtMs: Int64?,
        isComposingMessage: Bool,
        waitingReason: String?,
        isRunning: Bool,
        avatarState: MobileAgentAvatarState,
        draftPrompt: String?
    ) {
        let lastEntry = parseLastEntry(row["lastEntry"])
        let awaitingRaw = row["awaitingUserResponse"]
        let awaiting = awaitingRaw as? [String: Any]
        let awaitingPresent = awaitingRaw != nil && !(awaitingRaw is NSNull)
        let isComposingMessage = row["isComposingMessage"] as? Bool ?? false
        let isRunning = row["isRunning"] as? Bool ?? false
        let waitingReason = (awaiting?["reason"] as? String) ?? (row["waitingReason"] as? String)
        return (
            lastEntry,
            row["lastMessageId"] as? String,
            derivedLastMessage(lastEntry: lastEntry, fallback: row["lastMessagePreview"]),
            int64Value(row["updatedAt"]),
            isComposingMessage,
            waitingReason,
            isRunning,
            avatarState(
                currentActivity: row["currentActivity"],
                awaitingUserResponsePresent: awaitingPresent,
                isComposingMessage: isComposingMessage,
                isRunning: isRunning
            ),
            row["draftPrompt"] as? String
        )
    }

    static func canonicalProductDisplayName(_ configuredName: String) -> String {
        let legacy = configuredName.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return legacy == "grok" || legacy == "grok bot" ? "Fabushi" : configuredName
    }

    static func parseConversationPartnerIds(_ row: [String: Any]) -> [String] {
        guard let raw = row["conversationPartnerIds"] as? [String] else { return [] }
        var seen = Set<String>()
        return raw.compactMap { value in
            let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !normalized.isEmpty, seen.insert(normalized).inserted else { return nil }
            return normalized
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
        let summary = summaryProjection(row)
        let configuredName = (row["name"] as? String) ?? (row["displayName"] as? String) ?? id
        return MobileBotSummary(
            id: id,
            name: canonicalProductDisplayName(configuredName),
            description: row["description"] as? String ?? "",
            title: row["title"] as? String,
            avatarDataURL: row["avatar"] as? String,
            avatarShape: row["avatarShape"] as? String,
            avatarColor: row["avatarColor"] as? String,
            notifyOnUpdatesEnabled: row["notifyOnUpdates"] as? Bool ?? false,
            hidden: row["hidden"] as? Bool ?? false,
            unread: (row["hasUnread"] as? Bool) ?? (row["unread"] as? Bool) ?? false,
            conversationId: row["conversationId"] as? String,
            lastEntry: summary.lastEntry,
            lastMessageId: summary.lastMessageId,
            lastMessagePreview: summary.lastMessagePreview,
            updatedAtMs: summary.updatedAtMs,
            isComposingMessage: summary.isComposingMessage,
            waitingReason: summary.waitingReason,
            isRunning: summary.isRunning,
            avatarState: summary.avatarState,
            draftPrompt: summary.draftPrompt,
            miniAppId: miniAppId,
            menuButtonText: menuText?.isEmpty == false ? menuText : (miniAppId == nil ? nil : "打开应用"),
            conversationPartnerIds: parseConversationPartnerIds(row),
            isSharedRoom: row["isSharedRoom"] as? Bool ?? false
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
        let summary = summaryProjection(row)
        let configuredName = (row["name"] as? String) ?? id
        return MobileBotSummary(
            id: id,
            name: canonicalProductDisplayName(configuredName),
            description: row["description"] as? String ?? "",
            avatarDataURL: row["avatar"] as? String,
            avatarShape: row["avatarShape"] as? String,
            avatarColor: row["avatarColor"] as? String,
            unread: (row["hasUnread"] as? Bool) ?? (row["unread"] as? Bool) ?? false,
            conversationId: row["conversationId"] as? String,
            lastEntry: summary.lastEntry,
            lastMessageId: summary.lastMessageId,
            lastMessagePreview: summary.lastMessagePreview,
            updatedAtMs: summary.updatedAtMs,
            isComposingMessage: summary.isComposingMessage,
            waitingReason: summary.waitingReason,
            isRunning: summary.isRunning,
            avatarState: summary.avatarState,
            draftPrompt: summary.draftPrompt,
            isGroup: true,
            memberIds: memberIds,
            conversationPartnerIds: parseConversationPartnerIds(row),
            isSharedRoom: row["isSharedRoom"] as? Bool ?? false
        )
    }
}

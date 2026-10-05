import Foundation

/// Renderer-side bot data adapter.
///
/// The SwiftUI shell owns presentation state only. All Bot list/create I/O is
/// isolated here and crosses the trusted iOS preload boundary rather than
/// embedding protocol requests in a View.
@MainActor
struct GrokMobileBotService {
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
        let requestId = "ios-mobile-bot-list-\(UUID().uuidString.lowercased())"
        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: ["command": ["type": "bot.list", "requestId": requestId]]
            )
            for _ in 0..<32 {
                let result = try await bridge.request(
                    method: "feature.receive",
                    params: ["timeoutMs": 80]
                )
                guard let event = result.value as? [String: Any],
                      let type = event["type"] as? String
                else { continue }
                if type == "bot.listed", let rows = event["bots"] as? [[String: Any]] {
                    let surfaceBots = rows
                        .compactMap(Self.parseBot)
                        .filter { $0.id != "mahayana-assistant" }
                    return Self.mergeBots(installedBots, surfaceBots)
                }
            }
        } catch {
            return installedBots
        }
        return installedBots
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

    private func executeBotMutation(_ command: [String: Any]) async throws -> [MobileBotSummary] {
        _ = try await bridge.request(
            method: "feature.execute",
            params: ["command": command]
        )
        return await loadBots()
    }

    static func mergeBots(
        _ installed: [MobileBotSummary],
        _ surface: [MobileBotSummary]
    ) -> [MobileBotSummary] {
        var byId: [String: MobileBotSummary] = [:]
        for bot in surface { byId[bot.id] = bot }
        for bot in installed { byId[bot.id] = bot }
        return byId.values.sorted {
            if ($0.miniAppId != nil) != ($1.miniAppId != nil) {
                return $0.miniAppId != nil
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
            miniAppId: miniAppId,
            menuButtonText: menuText?.isEmpty == false
                ? menuText
                : (miniAppId == nil ? nil : "打开应用")
        )
    }
}

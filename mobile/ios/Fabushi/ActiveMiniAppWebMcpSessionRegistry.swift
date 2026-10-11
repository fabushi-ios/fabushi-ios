import Foundation

@MainActor
final class ActiveMiniAppWebMcpSessionRegistry {
    static let shared = ActiveMiniAppWebMcpSessionRegistry()

    struct Invocation {
        let value: Any
    }

    struct Snapshot: Equatable {
        let accountKey: String
        let pluginId: String
        let version: String
        let artifactSha256: String
        let pluginInstanceId: String
        let nonce: String
        let generation: UInt64
        let toolNames: [String]
    }

    private struct Entry {
        let snapshot: Snapshot
        let tools: [String: MiniAppToolContract]
        let invoke: @MainActor (String, [String: Any]) async throws -> Any
    }

    private var entriesByPluginId: [String: Entry] = [:]

    private init() {}

    func register(
        accountKey: String,
        pluginId: String,
        version: String,
        artifactSha256: String,
        pluginInstanceId: String,
        nonce: String,
        generation: UInt64,
        tools: [MiniAppToolContract],
        invoke: @escaping @MainActor (String, [String: Any]) async throws -> Any
    ) {
        let normalizedAccount = accountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedPluginId = pluginId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedAccount.isEmpty,
              !normalizedPluginId.isEmpty,
              !version.isEmpty,
              artifactSha256.range(
                  of: #"^[0-9a-fA-F]{64}$"#,
                  options: .regularExpression
              ) != nil,
              !pluginInstanceId.isEmpty,
              !nonce.isEmpty,
              generation > 0,
              !tools.isEmpty
        else { return }

        let uniqueTools = Dictionary(
            tools.map { ($0.name, $0) },
            uniquingKeysWith: { _, newer in newer }
        )
        guard !uniqueTools.isEmpty else { return }

        entriesByPluginId[normalizedPluginId] = Entry(
            snapshot: Snapshot(
                accountKey: normalizedAccount,
                pluginId: normalizedPluginId,
                version: version,
                artifactSha256: artifactSha256.lowercased(),
                pluginInstanceId: pluginInstanceId,
                nonce: nonce,
                generation: generation,
                toolNames: uniqueTools.keys.sorted()
            ),
            tools: uniqueTools,
            invoke: invoke
        )
    }

    func unregister(
        pluginId: String,
        pluginInstanceId: String,
        generation: UInt64
    ) {
        guard let entry = entriesByPluginId[pluginId],
              entry.snapshot.pluginInstanceId == pluginInstanceId,
              entry.snapshot.generation == generation
        else { return }
        entriesByPluginId.removeValue(forKey: pluginId)
    }

    func snapshot(
        accountKey: String,
        pluginId: String
    ) -> Snapshot? {
        guard let entry = entriesByPluginId[pluginId],
              entry.snapshot.accountKey == accountKey
        else { return nil }
        return entry.snapshot
    }

    func tools(
        accountKey: String,
        pluginId: String
    ) -> [MiniAppToolContract]? {
        guard let entry = entriesByPluginId[pluginId],
              entry.snapshot.accountKey == accountKey
        else { return nil }
        return entry.snapshot.toolNames.compactMap { entry.tools[$0] }
    }

    func callIfActive(
        accountKey: String,
        pluginId: String,
        name: String,
        arguments: [String: Any]
    ) async throws -> Invocation? {
        guard let entry = entriesByPluginId[pluginId],
              entry.snapshot.accountKey == accountKey,
              entry.tools[name] != nil
        else { return nil }

        let expectedInstanceId = entry.snapshot.pluginInstanceId
        let expectedGeneration = entry.snapshot.generation
        let value = try await entry.invoke(name, arguments)

        guard let current = entriesByPluginId[pluginId],
              current.snapshot.accountKey == accountKey,
              current.snapshot.pluginInstanceId == expectedInstanceId,
              current.snapshot.generation == expectedGeneration
        else {
            throw CancellationError()
        }
        return Invocation(value: value)
    }

    func resetForTesting() {
        entriesByPluginId.removeAll()
    }
}

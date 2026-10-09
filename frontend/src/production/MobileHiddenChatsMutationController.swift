import Foundation
import Observation

@MainActor
@Observable
final class MobileHiddenChatsMutationController {
    struct Mutation: Equatable {
        let id: String
        var value: Bool
        let previousValue: Bool
        var inFlight: Int
        var confirmed: Bool?
    }

    private(set) var accountScopeKey: String?
    private(set) var activeAgentId: String?
    private(set) var generation = 0
    private(set) var disposed = false
    private(set) var mutations: [String: Mutation] = [:]
    private(set) var held: [String: Bool] = [:]

    func setScope(accountScopeKey: String?, activeAgentId: String?) {
        guard !disposed else { return }
        guard self.accountScopeKey != accountScopeKey || self.activeAgentId != activeAgentId else { return }
        generation &+= 1
        self.accountScopeKey = accountScopeKey
        self.activeAgentId = activeAgentId
        mutations.removeAll()
        held.removeAll()
    }

    func ingestAgents(_ agents: [MobileBotSummary]) {
        guard !disposed else { return }
        let byID = Dictionary(uniqueKeysWithValues: agents.map { ($0.id, $0) })
        for (id, mutation) in Array(mutations) {
            guard mutation.inFlight == 0, let confirmed = mutation.confirmed else { continue }
            if byID[id]?.hidden == confirmed {
                mutations.removeValue(forKey: id)
                held.removeValue(forKey: id)
            }
        }
    }

    func isPending(_ agentId: String) -> Bool {
        guard !disposed else { return false }
        return held[agentId] != nil || (mutations[agentId]?.inFlight ?? 0) > 0
    }

    func projectAgents(_ agents: [MobileBotSummary]) -> [MobileBotSummary] {
        guard !disposed else { return agents }
        return agents.map { agent in
            guard let mutation = mutations[agent.id] else { return agent }
            return agent.replacingHidden(mutation.value)
        }
    }

    func setAgentHidden(
        agentId: String,
        isHidden: Bool,
        readAgent: (String) -> MobileBotSummary?,
        onOptimisticChange: (String, Bool) -> Void,
        call: @escaping (String, Bool) async throws -> Void,
        onRollback: @escaping (String, Bool, Bool) -> Void
    ) async throws {
        guard !disposed,
              accountScopeKey != nil,
              !agentId.isEmpty,
              held[agentId] == nil,
              (mutations[agentId]?.inFlight ?? 0) == 0,
              let agent = readAgent(agentId)
        else { return }

        let expectedGeneration = generation
        let mutation = Mutation(
            id: agentId,
            value: isHidden,
            previousValue: agent.hidden,
            inFlight: 1,
            confirmed: nil
        )
        mutations[agentId] = mutation
        onOptimisticChange(agentId, isHidden)

        do {
            try await call(agentId, isHidden)
            guard isCurrent(expectedGeneration) else { return }
            confirm(agentId)
            held.removeValue(forKey: agentId)
        } catch {
            guard isCurrent(expectedGeneration) else { return }
            if Self.isTransportFailure(error) {
                confirm(agentId)
                held[agentId] = isHidden
            } else {
                rollback(agentId, onRollback: onRollback)
            }
            throw error
        }
    }

    func noteReconnect(
        call: @escaping (String, Bool) async throws -> Void,
        onRollback: @escaping (String, Bool, Bool) -> Void
    ) {
        guard !disposed else { return }
        let expectedGeneration = generation
        for (id, value) in held {
            guard var mutation = mutations[id], mutation.inFlight == 0 else { continue }
            mutation.value = value
            mutation.inFlight = 1
            mutation.confirmed = nil
            mutations[id] = mutation
            Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    try await call(id, value)
                    guard self.isCurrent(expectedGeneration) else { return }
                    self.confirm(id)
                    self.held.removeValue(forKey: id)
                } catch {
                    guard self.isCurrent(expectedGeneration) else { return }
                    if Self.isTransportFailure(error) {
                        self.confirm(id)
                        self.held[id] = value
                    } else {
                        self.rollback(id, onRollback: onRollback)
                    }
                }
            }
        }
    }

    func reset() {
        generation &+= 1
        accountScopeKey = nil
        activeAgentId = nil
        mutations.removeAll()
        held.removeAll()
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        reset()
    }

    nonisolated static func isTransportFailure(_ error: Error) -> Bool {
        if let portError = error as? IOSCoordinatorPortClient.PortError {
            return portError.code == "source/transport-failure"
                || portError.code == "port-settled"
                || portError.code == "port-not-ready"
                || portError.code.hasPrefix("transport-")
        }
        return isTransientConnectError(error)
    }

    private func isCurrent(_ expectedGeneration: Int) -> Bool {
        !disposed && generation == expectedGeneration
    }

    private func confirm(_ agentId: String) {
        guard var mutation = mutations[agentId] else { return }
        if mutation.inFlight > 1 {
            mutation.inFlight -= 1
        } else {
            mutation.inFlight = 0
            mutation.confirmed = mutation.value
        }
        mutations[agentId] = mutation
    }

    private func rollback(
        _ agentId: String,
        onRollback: (String, Bool, Bool) -> Void
    ) {
        guard let mutation = mutations.removeValue(forKey: agentId) else { return }
        held.removeValue(forKey: agentId)
        onRollback(agentId, mutation.value, mutation.previousValue)
    }
}

extension MobileBotSummary {
    func replacingHidden(_ hidden: Bool) -> MobileBotSummary {
        MobileBotSummary(
            id: id,
            name: name,
            description: description,
            title: title,
            avatarDataURL: avatarDataURL,
            avatarShape: avatarShape,
            avatarColor: avatarColor,
            notifyOnUpdatesEnabled: notifyOnUpdatesEnabled,
            hidden: hidden,
            unread: unread,
            conversationId: conversationId,
            lastEntry: lastEntry,
            lastMessageId: lastMessageId,
            lastMessagePreview: lastMessagePreview,
            updatedAtMs: updatedAtMs,
            isComposingMessage: isComposingMessage,
            waitingReason: waitingReason,
            isRunning: isRunning,
            draftPrompt: draftPrompt,
            miniAppId: miniAppId,
            menuButtonText: menuButtonText,
            isGroup: isGroup,
            memberIds: memberIds,
            isSharedRoom: isSharedRoom
        )
    }
}

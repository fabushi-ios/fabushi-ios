import Foundation
import SwiftUI

internal enum MobileAgentNetworkAvailability: Equatable {
    case unavailable, retainedEmptyRoster, available
    static func resolve(gateEnabled: Bool, hasAgents: Bool) -> Self {
        guard gateEnabled else { return .unavailable }
        return hasAgents ? .available : .retainedEmptyRoster
    }
}

internal enum MobileAgentNetworkEdgeKind: String, Equatable, Sendable { case membership, message }
internal enum MobileAgentNetworkEdgeActivity: String, Equatable, Sendable { case idle, recent, talking }
internal enum MobileAgentNetworkAgentActivity: String, Equatable, Sendable { case idle, typing, working, waiting }

internal struct MobileAgentNetworkEdge: Identifiable, Equatable, Sendable {
    let id: String
    let sourceId: String
    let targetId: String
    let kind: MobileAgentNetworkEdgeKind
}

internal struct MobileAgentNetworkSelection: Equatable, Sendable { let agentId: String }

internal struct MobileAgentNetworkFence: Equatable, Sendable {
    let accountScopeKey: String
    let reconnectGeneration: Int
    let rosterIds: [String]
    static func capture(accountScopeKey: String, reconnectGeneration: Int, roster: [MobileBotSummary]) -> Self {
        .init(accountScopeKey: accountScopeKey, reconnectGeneration: reconnectGeneration, rosterIds: roster.map(\.id).sorted())
    }
    func matches(accountScopeKey: String, reconnectGeneration: Int, roster: [MobileBotSummary]) -> Bool {
        self.accountScopeKey == accountScopeKey
            && self.reconnectGeneration == reconnectGeneration
            && rosterIds == roster.map(\.id).sorted()
    }
}

internal enum MobileAgentNetworkModel {
    static let recentWindowMs: Int64 = 120_000

    static func edges(_ agents: [MobileBotSummary]) -> [MobileAgentNetworkEdge] {
        let known = Set(agents.map(\.id))
        var emitted = Set<String>()
        var result: [MobileAgentNetworkEdge] = []
        for agent in agents {
            if agent.isGroup {
                for memberId in agent.memberIds where memberId != agent.id && known.contains(memberId) {
                    let key = "member::\(agent.id)::\(memberId)"
                    if emitted.insert(key).inserted {
                        result.append(.init(id: key, sourceId: agent.id, targetId: memberId, kind: .membership))
                    }
                }
            } else {
                for partnerId in agent.conversationPartnerIds where partnerId != agent.id && known.contains(partnerId) {
                    let pair = [agent.id, partnerId].sorted()
                    let key = "msg::\(pair[0])::\(pair[1])"
                    if emitted.insert(key).inserted {
                        result.append(.init(id: key, sourceId: pair[0], targetId: pair[1], kind: .message))
                    }
                }
            }
        }
        return result
    }

    static func isMidTurn(_ agent: MobileBotSummary) -> Bool {
        let waiting = agent.waitingReason?.trimmingCharacters(in: .whitespacesAndNewlines)
        return (waiting == nil || waiting?.isEmpty == true) && agent.isRunning
    }

    static func edgeActivity(_ edge: MobileAgentNetworkEdge, agentsById: [String: MobileBotSummary], nowMs: Int64) -> MobileAgentNetworkEdgeActivity {
        guard let source = agentsById[edge.sourceId], let target = agentsById[edge.targetId] else { return .idle }
        if isMidTurn(source), isMidTurn(target) { return .talking }
        guard let sourceAt = source.updatedAtMs, sourceAt > 0, let targetAt = target.updatedAtMs, targetAt > 0 else { return .idle }
        return nowMs - min(sourceAt, targetAt) <= recentWindowMs ? .recent : .idle
    }

    static func activity(_ agent: MobileBotSummary) -> MobileAgentNetworkAgentActivity {
        if let waiting = agent.waitingReason?.trimmingCharacters(in: .whitespacesAndNewlines), !waiting.isEmpty { return .waiting }
        if agent.isComposingMessage { return .typing }
        if agent.isRunning { return .working }
        return .idle
    }

    static func toggle(_ selection: MobileAgentNetworkSelection?, id: String) -> MobileAgentNetworkSelection? {
        selection?.agentId == id ? nil : .init(agentId: id)
    }

    static func reconcile(_ selection: MobileAgentNetworkSelection?, agents: [MobileBotSummary]) -> MobileAgentNetworkSelection? {
        guard let selection else { return nil }
        return agents.contains(where: { $0.id == selection.agentId }) ? selection : nil
    }

    static func summary(agents: [MobileBotSummary], edges: [MobileAgentNetworkEdge]) -> String {
        let groups = agents.filter(\.isGroup).count
        let individuals = agents.count - groups
        let links = edges.filter { $0.kind == .message }.count
        return "\(individuals) \(individuals == 1 ? "agent" : "agents") · \(groups) \(groups == 1 ? "group" : "groups") · \(links) message \(links == 1 ? "link" : "links")"
    }

    static func extractPartnerIds(from rows: [[String: Any]], ownerId: String, knownAgentIds: Set<String>) -> [String] {
        var result = Set<String>()
        func accept(_ raw: String?) {
            guard let raw else { return }
            let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !value.isEmpty, value != ownerId, knownAgentIds.contains(value) else { return }
            result.insert(value)
        }
        let scalarKeys = ["agentId", "sourceAgentId", "targetAgentId", "senderAgentId", "recipientAgentId", "fromAgentId", "toAgentId"]
        let arrayKeys = ["conversationPartnerIds", "participantAgentIds", "agentIds"]
        for row in rows {
            for key in scalarKeys { accept(row[key] as? String) }
            for key in arrayKeys { (row[key] as? [String])?.forEach { accept($0) } }
            if let participants = row["participants"] as? [[String: Any]] {
                for participant in participants {
                    accept(participant["agentId"] as? String)
                    if (participant["type"] as? String) == "agent" { accept(participant["id"] as? String) }
                }
            }
        }
        return result.sorted()
    }

    static func openTarget(id: String, agents: [MobileBotSummary]) -> MobileBotSummary? {
        agents.first(where: { $0.id == id })
    }

    static func nodeAccessibilityIdentifier(_ id: String) -> String {
        "agent-network-node-\(id)"
    }

    static func gateEnabled(from snapshot: Any?) -> Bool? {
        guard let root = snapshot as? [String: Any] else { return nil }
        let direct = root["featureGates"] as? [String: Any]
        let nested = (root["snapshot"] as? [String: Any])?["featureGates"] as? [String: Any]
        let gates = direct ?? nested
        if let value = gates?["sand_agent_network"] as? Bool { return value }
        if let value = gates?["sand_agent_network"] as? NSNumber { return value.boolValue }
        if let row = gates?["sand_agent_network"] as? [String: Any] {
            if let value = row["value"] as? Bool { return value }
            if let value = row["value"] as? NSNumber { return value.boolValue }
        }
        return nil
    }
}

internal struct MobileAgentNetworkPoint: Equatable, Sendable { var x: CGFloat; var y: CGFloat }

internal enum MobileAgentNetworkLayout {
    static func positions(ids: [String], width: CGFloat, height: CGFloat) -> [String: MobileAgentNetworkPoint] {
        guard !ids.isEmpty else { return [:] }
        let width = max(1, width), height = max(1, height)
        let center = MobileAgentNetworkPoint(x: width / 2, y: height / 2)
        guard ids.count > 1 else { return [ids[0]: center] }
        let rx = max(20, (width - 128) / 2), ry = max(20, (height - 128) / 2)
        let goldenAngle = CGFloat.pi * (3 - sqrt(5.0))
        return Dictionary(uniqueKeysWithValues: ids.enumerated().map { index, id in
            let fraction = sqrt(CGFloat(index + 1) / CGFloat(ids.count))
            let angle = CGFloat(index) * goldenAngle
            return (id, .init(x: center.x + cos(angle) * rx * fraction * 0.82, y: center.y + sin(angle) * ry * fraction * 0.82))
        })
    }
    static func clampedScale(_ value: CGFloat) -> CGFloat { min(3, max(1, value)) }
    static func clampedOffset(_ offset: CGSize, scale: CGFloat, size: CGSize, overscroll: CGFloat = 0.5) -> CGSize {
        let scale = clampedScale(scale), extraX = size.width * overscroll, extraY = size.height * overscroll
        return .init(
            width: min(extraX, max(size.width * (1 - scale) - extraX, offset.width)),
            height: min(extraY, max(size.height * (1 - scale) - extraY, offset.height))
        )
    }
}

internal struct MobileAgentNetworkHistorySource {
    let bridge: IOSPreloadBridge

    func loadPartnerIds(roster: [MobileBotSummary]) async throws -> [String: [String]] {
        let known = Set(roster.map(\.id))
        var result = Dictionary(uniqueKeysWithValues: roster.map { ($0.id, $0.conversationPartnerIds) })
        for agent in roster where !agent.isGroup && agent.miniAppId == nil {
            try Task.checkCancellation()
            guard let conversationId = agent.conversationId?.trimmingCharacters(in: .whitespacesAndNewlines), !conversationId.isEmpty else { continue }
            let requestId = "ios-agent-network-history-\(UUID().uuidString.lowercased())"
            do {
                _ = try await bridge.request(method: "feature.execute", params: ["command": [
                    "type": "conversation.openTail", "requestId": requestId, "conversationId": conversationId, "limit": 200,
                ]])
                let response = try await bridge.receiveFeatureEvent(deadlineMilliseconds: 8_000) { event in
                    event["type"] as? String == "conversation.windowOpened"
                        && event["requestId"] as? String == requestId
                        && event["conversationId"] as? String == conversationId
                }
                guard let event = response.value as? [String: Any], let rows = event["messages"] as? [[String: Any]] else { continue }
                let discovered = MobileAgentNetworkModel.extractPartnerIds(from: rows, ownerId: agent.id, knownAgentIds: known)
                result[agent.id] = Array(Set((result[agent.id] ?? []) + discovered)).sorted()
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                continue
            }
        }
        return result
    }
}

internal struct FabushiAgentNetworkView: View {
    let agents: [MobileBotSummary]
    let onOpenAgent: (String) -> Void
    let onClose: () -> Void
    @State private var selection: MobileAgentNetworkSelection?
    @State private var scale: CGFloat = 1
    @State private var baseScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var baseOffset: CGSize = .zero

    var body: some View {
        NavigationStack {
            GeometryReader { proxy in
                let edges = MobileAgentNetworkModel.edges(agents)
                let ordered = agents.sorted { $0.isGroup == $1.isGroup ? $0.id < $1.id : ($0.isGroup && !$1.isGroup) }
                let positions = MobileAgentNetworkLayout.positions(ids: ordered.map(\.id), width: proxy.size.width, height: proxy.size.height)
                let byId = Dictionary(uniqueKeysWithValues: agents.map { ($0.id, $0) })
                ZStack(alignment: .topTrailing) {
                    if agents.isEmpty {
                        ContentUnavailableView("No agents yet", systemImage: "point.3.connected.trianglepath.dotted", description: Text("Create a few teammates and the network draws itself."))
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .accessibilityIdentifier("agent-network-empty")
                    } else {
                        ZStack {
                            Canvas { context, _ in
                                for edge in edges {
                                    guard let source = positions[edge.sourceId], let target = positions[edge.targetId] else { continue }
                                    var path = Path(); path.move(to: .init(x: source.x, y: source.y)); path.addLine(to: .init(x: target.x, y: target.y))
                                    let activity = MobileAgentNetworkModel.edgeActivity(edge, agentsById: byId, nowMs: Int64(Date().timeIntervalSince1970 * 1_000))
                                    context.stroke(path, with: .color(.secondary.opacity(activity == .idle ? 0.32 : 0.85)), style: .init(lineWidth: activity == .talking ? 3 : 1.5, dash: edge.kind == .membership ? [7, 5] : []))
                                }
                            }.allowsHitTesting(false)
                            ForEach(ordered) { agent in
                                if let point = positions[agent.id] { agentNode(agent).position(x: point.x, y: point.y) }
                            }
                        }
                        .scaleEffect(scale, anchor: .topLeading)
                        .offset(offset)
                        .contentShape(Rectangle())
                        .gesture(DragGesture(minimumDistance: 4).onChanged { value in
                            offset = MobileAgentNetworkLayout.clampedOffset(.init(width: baseOffset.width + value.translation.width, height: baseOffset.height + value.translation.height), scale: scale, size: proxy.size)
                        }.onEnded { _ in baseOffset = offset })
                        .simultaneousGesture(MagnificationGesture().onChanged { value in
                            scale = MobileAgentNetworkLayout.clampedScale(baseScale * value)
                            offset = MobileAgentNetworkLayout.clampedOffset(offset, scale: scale, size: proxy.size)
                        }.onEnded { _ in baseScale = scale; baseOffset = offset })
                        .onTapGesture(count: 2) { resetViewport() }
                        .accessibilityIdentifier("agent-network-canvas")
                    }
                    if let selected = selectedAgent {
                        inspector(selected, byId: byId).frame(maxWidth: min(360, max(240, proxy.size.width - 24))).padding(12)
                    }
                }
                .background(RadialGradient(colors: [Color.black.opacity(0.07), Color.black.opacity(0.15)], center: .center, startRadius: 20, endRadius: max(proxy.size.width, proxy.size.height)))
                .clipShape(RoundedRectangle(cornerRadius: 12)).padding(12)
            }
            .navigationTitle("Org chart")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Close", action: onClose).accessibilityIdentifier("agent-network-close") }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { resetViewport() } label: { Image(systemName: "scope") }
                        .accessibilityLabel("Reset network view").accessibilityIdentifier("agent-network-reset")
                }
            }
            .safeAreaInset(edge: .bottom) {
                Text(MobileAgentNetworkModel.summary(agents: agents, edges: MobileAgentNetworkModel.edges(agents)))
                    .font(.caption).foregroundStyle(.secondary).padding(.vertical, 6).accessibilityIdentifier("agent-network-summary")
            }
        }
        .accessibilityIdentifier("agent-network")
        .onChange(of: agents.map(\.id)) { _, _ in selection = MobileAgentNetworkModel.reconcile(selection, agents: agents) }
    }

    private var selectedAgent: MobileBotSummary? {
        selection.flatMap { selected in agents.first(where: { $0.id == selected.agentId }) }
    }

    @ViewBuilder private func agentNode(_ agent: MobileBotSummary) -> some View {
        let activity = MobileAgentNetworkModel.activity(agent)
        Button { selection = MobileAgentNetworkModel.toggle(selection, id: agent.id) } label: {
            VStack(spacing: 5) {
                ZStack {
                    Circle().fill(Color(uiColor: .secondarySystemBackground)).frame(width: 54, height: 54)
                    Image(systemName: agent.isGroup ? "person.3.fill" : "sparkles").font(.title3)
                }
                Text(agent.name).font(.caption.weight(.semibold)).lineLimit(1).frame(maxWidth: 112)
                Text(activityLabel(activity, agent: agent)).font(.caption2).foregroundStyle(.secondary).lineLimit(1).frame(maxWidth: 112)
            }.frame(width: 112)
        }
        .buttonStyle(.plain).accessibilityLabel(agent.name).accessibilityValue(activityLabel(activity, agent: agent))
        .accessibilityAddTraits(selection?.agentId == agent.id ? .isSelected : [])
        .accessibilityIdentifier(MobileAgentNetworkModel.nodeAccessibilityIdentifier(agent.id))
    }

    @ViewBuilder private func inspector(_ agent: MobileBotSummary, byId: [String: MobileBotSummary]) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text(agent.name).font(.headline)
                    Text(activityLabel(MobileAgentNetworkModel.activity(agent), agent: agent)).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button { selection = nil } label: { Image(systemName: "xmark.circle.fill") }
                    .accessibilityLabel("Close details").accessibilityIdentifier("agent-network-inspector-close")
            }
            if !agent.description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { Text(agent.description).font(.subheadline) }
            if agent.isGroup {
                let members = agent.memberIds.compactMap { byId[$0] }
                if !members.isEmpty {
                    Text("\(members.count) \(members.count == 1 ? "member" : "members")").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    Text(members.map(\.name).joined(separator: ", ")).font(.subheadline)
                }
            }
            if let last = agent.lastMessagePreview?.trimmingCharacters(in: .whitespacesAndNewlines), !last.isEmpty {
                Text("Last activity").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Text(last).font(.subheadline).lineLimit(4)
                if let updated = agent.updatedAtMs, updated > 0 {
                    Text(Date(timeIntervalSince1970: Double(updated) / 1_000), style: .relative).font(.caption2).foregroundStyle(.secondary)
                }
            }
            Button(agent.isGroup ? "Open room" : "Open chat") { onOpenAgent(agent.id) }
                .buttonStyle(.borderedProminent).frame(maxWidth: .infinity, alignment: .trailing)
                .accessibilityIdentifier("agent-network-open-\(agent.id)")
        }
        .padding(16).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14)).shadow(radius: 12)
        .accessibilityElement(children: .contain).accessibilityLabel("Org chart details").accessibilityIdentifier("agent-network-inspector")
    }

    private func activityLabel(_ activity: MobileAgentNetworkAgentActivity, agent: MobileBotSummary) -> String {
        switch activity {
        case .waiting:
            let waiting = agent.waitingReason?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return waiting.isEmpty ? "Waiting for you" : waiting
        case .typing: return "Typing…"
        case .working: return "Working…"
        case .idle: return agent.isGroup ? "\(agent.memberIds.count) \(agent.memberIds.count == 1 ? "member" : "members")" : "Idle"
        }
    }

    private func resetViewport() {
        scale = 1; baseScale = 1; offset = .zero; baseOffset = .zero
    }
}

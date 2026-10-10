import SwiftUI

internal struct MobileAgentChannelConnection: Equatable, Identifiable {
    let platform: String
    let label: String
    let status: String
    let detail: String?

    var id: String { platform }

    init(json: [String: Any]) throws {
        guard let platform = json["platform"] as? String,
              !platform.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let label = json["label"] as? String,
              let status = json["status"] as? String
        else {
            throw MobileAgentChannelsModel.DecodeError.invalidResponse
        }
        if let rawDetail = json["detail"], !(rawDetail is String) {
            throw MobileAgentChannelsModel.DecodeError.invalidResponse
        }
        self.platform = platform
        self.label = label
        self.status = status
        self.detail = json["detail"] as? String
    }
}

internal enum MobileAgentChannelRowStatus: Equatable {
    case comingSoon
    case connected
    case error
    case connecting
    case available
}

internal enum MobileAgentChannelsModel {
    enum DecodeError: Error {
        case invalidResponse
    }

    static func decodeConnections(_ value: Any) throws -> [MobileAgentChannelConnection] {
        guard let rows = value as? [[String: Any]] else {
            throw DecodeError.invalidResponse
        }
        return try rows.map(MobileAgentChannelConnection.init(json:))
    }

    static func rowStatus(
        manifest: ConnectorManifest,
        connection: MobileAgentChannelConnection?
    ) -> MobileAgentChannelRowStatus {
        if manifest.availability == .comingSoon { return .comingSoon }
        if connection?.status == "connected" { return .connected }
        if connection?.status == "error" { return .error }
        if connection != nil { return .connecting }
        return .available
    }

    static func statusLabel(_ status: MobileAgentChannelRowStatus) -> String? {
        switch status {
        case .comingSoon: "Soon"
        case .connected: "Connected"
        case .error: "Needs attention"
        case .connecting: "Connecting"
        case .available: nil
        }
    }

    static func detail(
        manifest: ConnectorManifest,
        connection: MobileAgentChannelConnection?
    ) -> String {
        switch rowStatus(manifest: manifest, connection: connection) {
        case .connected:
            let label = connection?.label.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return label.isEmpty ? "Connected" : "Connected as \(label)"
        case .error:
            let detail = connection?.detail?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return detail.isEmpty ? "The platform rejected this connection." : detail
        default:
            return manifest.blurb
        }
    }
}

@MainActor
internal struct MobileAgentChannelsPanel: View {
    let agentId: String
    let agentName: String
    let bridge: IOSPreloadBridge
    let accountScopeKey: String
    let reconnectGeneration: Int
    let onClose: () -> Void

    @State private var connections: [MobileAgentChannelConnection] = []
    @State private var loading = true
    @State private var errorText: String?
    @State private var pending: Set<String> = []
    @State private var credentialPlatform: String?
    @State private var credentialDraft = ""
    @State private var generation: UInt64 = 0

    private var manifests: [ConnectorManifest] {
        var rows = CONNECTOR_MANIFESTS
        let known = Set(rows.map(\.platform))
        for connection in connections where !known.contains(connection.platform) {
            rows.append(.init(
                platform: connection.platform,
                displayName: connection.platform,
                blurb: "Connector configured for this Agent.",
                credentialLabel: "token",
                availability: .available,
                connectGuide: ""
            ))
        }
        return rows
    }

    var body: some View {
        NavigationStack {
            Group {
                if loading && connections.isEmpty {
                    ProgressView("Loading channels…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if let errorText, connections.isEmpty {
                    ContentUnavailableView(
                        "Channels unavailable",
                        systemImage: "exclamationmark.triangle",
                        description: Text(errorText)
                    )
                } else {
                    List {
                        if let errorText {
                            Section {
                                Text("Showing the last known channel snapshot. \(errorText)")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        ForEach(manifests, id: \.platform) { manifest in
                            channelRow(manifest)
                        }
                    }
                    .refreshable { await load() }
                }
            }
            .navigationTitle("Channels · \(agentName)")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", action: onClose)
                }
            }
        }
        .accessibilityIdentifier("mobile-agent-channels")
        .task(id: "\(agentId)|\(accountScopeKey)|\(reconnectGeneration)") {
            generation &+= 1
            pending.removeAll()
            credentialPlatform = nil
            credentialDraft = ""
            await load(expectedGeneration: generation)
        }
        .onDisappear {
            generation &+= 1
            pending.removeAll()
            credentialDraft = ""
        }
    }

    @ViewBuilder
    private func channelRow(_ manifest: ConnectorManifest) -> some View {
        let connection = connections.first { $0.platform == manifest.platform }
        let status = MobileAgentChannelsModel.rowStatus(
            manifest: manifest,
            connection: connection
        )
        let key = manifest.platform

        Section {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline) {
                    Text(manifest.displayName).font(.headline)
                    Spacer()
                    if let label = MobileAgentChannelsModel.statusLabel(status) {
                        Text(label).font(.caption).foregroundStyle(.secondary)
                    }
                }
                Text(MobileAgentChannelsModel.detail(
                    manifest: manifest,
                    connection: connection
                ))
                .font(.subheadline)
                .foregroundStyle(.secondary)

                switch status {
                case .available, .error:
                    if credentialPlatform == manifest.platform {
                        SecureField(manifest.credentialLabel, text: $credentialDraft)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .accessibilityIdentifier("mobile-agent-channel-credential-\(key)")
                        HStack {
                            Button("Cancel") {
                                credentialPlatform = nil
                                credentialDraft = ""
                            }
                            Button(status == .error ? "Reconnect" : "Connect") {
                                let token = credentialDraft
                                Task { await connect(manifest.platform, token: token) }
                            }
                            .disabled(
                                pending.contains("connect:\(key)")
                                    || credentialDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                            )
                            .buttonStyle(.borderedProminent)
                        }
                    } else {
                        Button(status == .error ? "Reconnect" : "Connect") {
                            credentialPlatform = manifest.platform
                            credentialDraft = ""
                        }
                        .disabled(!pending.isEmpty)
                        .accessibilityIdentifier("mobile-agent-channel-connect-\(key)")
                    }

                case .connected, .connecting:
                    HStack {
                        Button("Refresh") {
                            Task { await mutate("refreshChannel", platform: manifest.platform) }
                        }
                        .disabled(!pending.isEmpty)
                        Button("Disconnect", role: .destructive) {
                            Task { await mutate("disconnectChannel", platform: manifest.platform) }
                        }
                        .disabled(!pending.isEmpty)
                    }

                case .comingSoon:
                    EmptyView()
                }

                if !manifest.connectGuide.isEmpty {
                    DisclosureGroup("How to connect") {
                        Text(manifest.connectGuide)
                            .font(.caption)
                            .textSelection(.enabled)
                    }
                }
            }
            .padding(.vertical, 4)
            .accessibilityIdentifier("mobile-agent-channel-row-\(key)")
        }
    }

    private func accepts(_ ownedGeneration: UInt64, _ ownedAgentId: String) -> Bool {
        generation == ownedGeneration
            && agentId == ownedAgentId
            && !Task.isCancelled
    }

    private func load(expectedGeneration: UInt64? = nil) async {
        let ownedGeneration = expectedGeneration ?? generation
        let ownedAgentId = agentId
        loading = true
        do {
            let result = try await bridge.request(
                method: "getAgentChannels",
                params: ["id": ownedAgentId]
            )
            let decoded = try MobileAgentChannelsModel.decodeConnections(result.value)
            guard accepts(ownedGeneration, ownedAgentId) else { return }
            connections = decoded
            errorText = nil
        } catch is CancellationError {
            return
        } catch {
            guard accepts(ownedGeneration, ownedAgentId) else { return }
            errorText = error.localizedDescription
        }
        guard accepts(ownedGeneration, ownedAgentId) else { return }
        loading = false
    }

    private func connect(_ platform: String, token: String) async {
        guard !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        await mutate("connectChannel", platform: platform, token: token)
        if errorText == nil {
            credentialPlatform = nil
            credentialDraft = ""
        }
    }

    private func mutate(_ method: String, platform: String, token: String? = nil) async {
        let ownedGeneration = generation
        let ownedAgentId = agentId
        let operation = method == "connectChannel"
            ? "connect"
            : method == "disconnectChannel" ? "disconnect" : "refresh"
        let key = "\(operation):\(platform)"
        guard !pending.contains(key) else { return }
        pending.insert(key)
        defer {
            if accepts(ownedGeneration, ownedAgentId) {
                pending.remove(key)
            }
        }

        do {
            var params: [String: Any] = [
                "id": ownedAgentId,
                "platform": platform,
            ]
            if let token { params["token"] = token }
            let result = try await bridge.request(method: method, params: params)
            let decoded = try MobileAgentChannelsModel.decodeConnections(result.value)
            guard accepts(ownedGeneration, ownedAgentId) else { return }
            connections = decoded
            errorText = nil
        } catch is CancellationError {
            return
        } catch {
            guard accepts(ownedGeneration, ownedAgentId) else { return }
            errorText = error.localizedDescription
        }
    }
}

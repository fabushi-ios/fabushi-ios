import SwiftUI
import UIKit

internal struct MobileRootNotificationAction {
    enum Kind {
        case openURL(URL)
        case dashboard(action: String, args: [String: Any], successMessage: String?)
    }

    let label: String
    let kind: Kind
}

internal struct MobileRootNotificationTray: Identifiable {
    let id: String
    let title: String
    let detail: String?
    let requestID: String?
    let errorKind: String?
    let actions: [MobileRootNotificationAction]
    let count: Int?
}

internal struct MobileRootNotificationActionNotice {
    let isError: Bool
    let text: String
}

internal struct MobileRootNotificationBridgeRequest {
    let method: String
    let params: [String: Any]
}

internal func mobileRootNotificationLifecycleKey(
    authResolved: Bool,
    loggedIn: Bool,
    accountScopeKey: String,
    reconnectGeneration: Int
) -> String {
    [
        String(authResolved),
        String(loggedIn),
        accountScopeKey,
        String(reconnectGeneration),
    ].joined(separator: "|")
}

internal func mobileRootNotificationListCommand(requestID: String) -> [String: Any] {
    ["type": "tray.list", "requestId": requestID]
}

internal func mobileRootNotificationDismissCommand(
    id: String,
    requestID: String
) -> [String: Any]? {
    let normalizedID = id.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !normalizedID.isEmpty else { return nil }
    return [
        "type": "tray.dismiss",
        "requestId": requestID,
        "id": normalizedID,
    ]
}

internal func mobileRootNotificationClearCommand(requestID: String) -> [String: Any] {
    ["type": "tray.clear", "requestId": requestID]
}

internal func mobileRootNotificationBridgeRequest(
    for action: MobileRootNotificationAction
) -> MobileRootNotificationBridgeRequest {
    switch action.kind {
    case .openURL(let url):
        return .init(
            method: "openExternal",
            params: ["url": url.absoluteString]
        )
    case .dashboard(let actionName, let args, _):
        return .init(
            method: "invokeCursorDashboardAction",
            params: [
                "action": actionName,
                "args": args,
            ]
        )
    }
}

internal func projectMobileRootNotificationAction(_ value: Any) -> MobileRootNotificationAction? {
    guard let row = value as? [String: Any],
          let kind = row["kind"] as? String,
          let label = row["label"] as? String,
          !label.isEmpty
    else { return nil }

    if kind == "open-url",
       let rawURL = row["url"] as? String,
       let url = URL(string: rawURL),
       ["http", "https"].contains(url.scheme?.lowercased() ?? "")
    {
        return .init(label: label, kind: .openURL(url))
    }

    guard kind == "dashboard-action",
          let action = row["action"] as? String,
          !action.isEmpty,
          let args = row["args"] as? [String: Any]
    else { return nil }

    return .init(
        label: label,
        kind: .dashboard(
            action: action,
            args: args,
            successMessage: row["successMessage"] as? String
        )
    )
}

internal func projectMobileRootNotificationTray(_ value: Any) -> MobileRootNotificationTray? {
    guard let row = value as? [String: Any],
          row["kind"] as? String == "error",
          let id = row["id"] as? String,
          !id.isEmpty,
          let title = row["title"] as? String,
          !title.isEmpty
    else { return nil }

    let actions = (row["actions"] as? [Any] ?? [])
        .compactMap(projectMobileRootNotificationAction)
        .prefix(3)

    let rawCount = (row["count"] as? NSNumber)?.intValue
    return .init(
        id: id,
        title: title,
        detail: (row["detail"] as? String).flatMap { $0.isEmpty ? nil : $0 },
        requestID: (row["requestId"] as? String).flatMap { $0.isEmpty ? nil : $0 },
        errorKind: (row["errorKind"] as? String).flatMap { $0.isEmpty ? nil : $0 },
        actions: Array(actions),
        count: rawCount.flatMap { $0 > 1 ? $0 : nil }
    )
}

internal func projectMobileRootNotificationTrays(_ value: Any) -> [MobileRootNotificationTray] {
    (value as? [Any] ?? []).compactMap(projectMobileRootNotificationTray)
}

internal func reduceMobileRootNotificationEvent(
    _ current: [MobileRootNotificationTray],
    event: [String: Any]
) -> [MobileRootNotificationTray] {
    guard event["type"] as? String == "tray.changed",
          let action = event["action"] as? String
    else { return current }

    switch action {
    case "cleared":
        return []
    case "dismissed":
        guard let id = event["id"] as? String else { return current }
        return current.filter { $0.id != id }
    case "pushed":
        guard let trayValue = event["tray"],
              let tray = projectMobileRootNotificationTray(trayValue)
        else { return current }
        if let index = current.firstIndex(where: { $0.id == tray.id }) {
            var next = current
            next[index] = tray
            return next
        }
        return current + [tray]
    default:
        return current
    }
}

internal enum MobileSidebarAgentStatusBadge: Equatable, Sendable {
    case working
    case blocked
    case unread
}

internal struct MobileSidebarAgentVisualProjection: Equatable, Sendable {
    let isTyping: Bool
    let isWorking: Bool
    let statusBadge: MobileSidebarAgentStatusBadge?
    let statusLabel: String?
    let title: String?
    let isPinned: Bool
}

internal func projectMobileSidebarAgentVisual(
    _ bot: MobileBotSummary,
    isPinned: Bool
) -> MobileSidebarAgentVisualProjection {
    let isBlocked = bot.waitingReason != nil
    let isTyping = !isBlocked && bot.isComposingMessage
    let isWorking = !isBlocked && (bot.isRunning || isTyping)
    let statusBadge: MobileSidebarAgentStatusBadge?
    let statusLabel: String?
    if isBlocked {
        statusBadge = .blocked
        statusLabel = "Needs attention"
    } else if bot.unread {
        statusBadge = .unread
        statusLabel = "Unread activity"
    } else if isWorking {
        statusBadge = .working
        statusLabel = "Working"
    } else {
        statusBadge = nil
        statusLabel = nil
    }

    let title: String?
    if bot.isGroup {
        title = nil
    } else if let candidate = bot.title?.trimmingCharacters(in: .whitespacesAndNewlines),
              !candidate.isEmpty
    {
        title = candidate
    } else {
        title = nil
    }

    return .init(
        isTyping: isTyping,
        isWorking: isWorking,
        statusBadge: statusBadge,
        statusLabel: statusLabel,
        title: title,
        isPinned: isPinned
    )
}

private func mobileSidebarAgentBadgeColor(
    _ status: MobileSidebarAgentStatusBadge?
) -> Color? {
    switch status {
    case .working: .green
    case .blocked: .orange
    case .unread: Color.accentColor
    case nil: nil
    }
}

internal func mobileBotHomeSubtitle(_ bot: MobileBotSummary) -> String {
    if let waitingReason = bot.waitingReason?.trimmingCharacters(in: .whitespacesAndNewlines),
       !waitingReason.isEmpty
    {
        return waitingReason
    }
    if bot.isComposingMessage { return "正在输入…" }
    if let preview = bot.lastMessagePreview?.trimmingCharacters(in: .whitespacesAndNewlines),
       !preview.isEmpty
    {
        return preview
    }
    if bot.isRunning { return "正在运行…" }
    return bot.description.isEmpty ? "Ready" : bot.description
}

extension GrokMobileShell {
    var rootNotificationLifecycleKey: String {
        mobileRootNotificationLifecycleKey(
            authResolved: model.authResolved,
            loggedIn: model.loggedIn,
            accountScopeKey: mobileAccountScopeKey,
            reconnectGeneration: reconnectGeneration
        )
    }

    @ViewBuilder
    var rootNotificationStack: some View {
        if !rootNotificationTrays.isEmpty {
            VStack(alignment: .trailing, spacing: 8) {
                if rootNotificationTrays.count > 1 {
                    Button("Clear all") {
                        Task { await clearRootNotifications() }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .accessibilityIdentifier("root-notifications-clear-all")
                }

                ForEach(rootNotificationTrays) { tray in
                    HStack(alignment: .top, spacing: 10) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                            .accessibilityHidden(true)

                        VStack(alignment: .leading, spacing: 6) {
                            HStack(spacing: 6) {
                                Text(tray.title)
                                    .font(.subheadline.weight(.semibold))
                                if let count = tray.count {
                                    Text("×\(count)")
                                        .font(.caption.monospacedDigit().weight(.semibold))
                                        .foregroundStyle(.secondary)
                                        .accessibilityLabel("Occurred \(count) times")
                                }
                            }
                            if let detail = tray.detail {
                                Text(detail)
                                    .font(.footnote)
                                    .foregroundStyle(.secondary)
                            }
                            if !tray.actions.isEmpty {
                                HStack(spacing: 8) {
                                    ForEach(Array(tray.actions.enumerated()), id: \.offset) { index, action in
                                        Button(action.label) {
                                            Task {
                                                await runRootNotificationAction(
                                                    action,
                                                    trayID: tray.id,
                                                    actionIndex: index
                                                )
                                            }
                                        }
                                        .buttonStyle(.bordered)
                                        .controlSize(.small)
                                        .disabled(rootNotificationActionPending.contains("\(tray.id):\(index)"))
                                    }
                                }
                            }
                            if let notice = rootNotificationActionNotice[tray.id] {
                                Text(notice.text)
                                    .font(.caption)
                                    .foregroundStyle(notice.isError ? Color.red : Color.secondary)
                                    .accessibilityIdentifier("root-notification-action-result-\(tray.id)")
                            }
                        }

                        Spacer(minLength: 4)

                        if let requestID = tray.requestID {
                            Button {
                                UIPasteboard.general.string = requestID
                                rootNotificationCopiedRequestID = requestID
                            } label: {
                                Image(systemName: rootNotificationCopiedRequestID == requestID ? "checkmark" : "doc.on.doc")
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Copy request ID")
                            .accessibilityIdentifier("root-notification-copy-\(tray.id)")
                        }

                        Button {
                            Task { await dismissRootNotification(tray.id) }
                        } label: {
                            Image(systemName: "xmark")
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Dismiss notification")
                        .accessibilityIdentifier("root-notification-dismiss-\(tray.id)")
                    }
                    .padding(12)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .overlay(
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .stroke(Color.red.opacity(0.18), lineWidth: 1)
                    )
                    .shadow(radius: 6, y: 2)
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("root-notification-\(tray.id)")
                }
            }
            .frame(maxWidth: 520)
            .padding(.horizontal, 12)
            .padding(.top, 8)
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Notifications")
            .accessibilityIdentifier("root-notifications")
        }
    }

    @MainActor
    func runRootNotificationLifecycle() async {
        rootNotificationTrays = []
        rootNotificationActionPending.removeAll()
        rootNotificationActionNotice.removeAll()
        rootNotificationCopiedRequestID = nil

        guard model.authResolved, model.loggedIn else { return }

        do {
            try await refreshRootNotifications()
        } catch is CancellationError {
            return
        } catch {
            // Advisory surface: a failed hydration must not fabricate Host
            // success or block the primary product UI.
        }

        while !Task.isCancelled, model.loggedIn {
            do {
                let event = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 1_000
                ) { event in
                    event["type"] as? String == "tray.changed"
                }
                guard !Task.isCancelled,
                      let payload = event.value as? [String: Any]
                else { return }
                rootNotificationTrays = reduceMobileRootNotificationEvent(
                    rootNotificationTrays,
                    event: payload
                )
            } catch is CancellationError {
                return
            } catch IOSFeatureEventBrokerError.timedOut {
                continue
            } catch {
                try? await Task.sleep(nanoseconds: 250_000_000)
            }
        }
    }

    @MainActor
    func refreshRootNotifications() async throws {
        let requestID = "ios-tray-list-\(UUID().uuidString.lowercased())"
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": mobileRootNotificationListCommand(requestID: requestID),
            ]
        )
        let event = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 5_120
        ) { event in
            event["type"] as? String == "tray.listed"
        }
        guard let payload = event.value as? [String: Any],
              let trays = payload["trays"]
        else {
            rootNotificationTrays = []
            return
        }
        rootNotificationTrays = projectMobileRootNotificationTrays(trays)
    }

    @MainActor
    func dismissRootNotification(_ id: String) async {
        let requestID = "ios-tray-dismiss-\(UUID().uuidString.lowercased())"
        guard let command = mobileRootNotificationDismissCommand(
            id: id,
            requestID: requestID
        ) else { return }
        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: ["command": command]
            )
        } catch {
            rootNotificationActionNotice[id] = .init(
                isError: true,
                text: "Couldn’t dismiss this notification."
            )
        }
    }

    @MainActor
    func clearRootNotifications() async {
        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: [
                    "command": mobileRootNotificationClearCommand(
                        requestID: "ios-tray-clear-\(UUID().uuidString.lowercased())"
                    ),
                ]
            )
        } catch {
            guard let firstID = rootNotificationTrays.first?.id else { return }
            rootNotificationActionNotice[firstID] = .init(
                isError: true,
                text: "Couldn’t clear notifications."
            )
        }
    }

    @MainActor
    func runRootNotificationAction(
        _ action: MobileRootNotificationAction,
        trayID: String,
        actionIndex: Int
    ) async {
        let pendingKey = "\(trayID):\(actionIndex)"
        guard !rootNotificationActionPending.contains(pendingKey) else { return }
        rootNotificationActionPending.insert(pendingKey)
        rootNotificationActionNotice[trayID] = nil
        defer { rootNotificationActionPending.remove(pendingKey) }

        do {
            let request = mobileRootNotificationBridgeRequest(for: action)
            switch action.kind {
            case .openURL:
                _ = try await bridge.request(
                    method: request.method,
                    params: request.params
                )
            case .dashboard(_, _, let successMessage):
                let result = try await bridge.request(
                    method: request.method,
                    params: request.params
                ).value
                guard let response = result as? [String: Any],
                      let ok = response["ok"] as? Bool
                else {
                    rootNotificationActionNotice[trayID] = .init(
                        isError: true,
                        text: "Couldn’t complete the notification action — try again."
                    )
                    return
                }
                let message = (response["message"] as? String) ?? successMessage
                if ok {
                    if let message, !message.isEmpty {
                        rootNotificationActionNotice[trayID] = .init(
                            isError: false,
                            text: message
                        )
                    }
                } else {
                    rootNotificationActionNotice[trayID] = .init(
                        isError: true,
                        text: message ?? "Couldn’t complete the notification action — try again."
                    )
                }
            }
        } catch {
            rootNotificationActionNotice[trayID] = .init(
                isError: true,
                text: "Couldn’t complete the notification action — try again."
            )
        }
    }

    var home: some View {
        ZStack {
            Color(red: 0.985, green: 0.985, blue: 0.975).ignoresSafeArea()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    HStack {
                        Button { legacyOpen = true } label: {
                            ZStack {
                                Circle().fill(Color(red: 1.0, green: 0.78, blue: 0.82))
                                Text(String(model.accountName.prefix(1)).uppercased()).font(.system(size: 14, weight: .bold)).foregroundStyle(.black)
                            }
                            .frame(width: 38, height: 38)
                            .overlay(Circle().stroke(.white, lineWidth: 3)).shadow(color: .black.opacity(0.08), radius: 6)
                        }
                        .accessibilityIdentifier("grok-mobile-legacy")
                        Spacer()
                        if agentNetworkAvailability != .unavailable {
                            Button { agentNetworkOpen = true } label: {
                                Image(systemName: "point.3.connected.trianglepath.dotted")
                            }
                            .accessibilityLabel("Agent network")
                            .accessibilityValue(agentNetworkAvailability == .retainedEmptyRoster ? "No agents yet" : "\(bots.count) nodes")
                            .accessibilityIdentifier("grok-mobile-agent-network")
                        }
                        Button { toggleCommandPalette() } label: { Image(systemName: "magnifyingglass") }
                            .accessibilityIdentifier("grok-mobile-search")
                        Button { composeOpen = true } label: { Image(systemName: "plus") }
                            .accessibilityIdentifier("grok-mobile-add")
                    }
                    .font(.system(size: 19, weight: .semibold)).foregroundStyle(.black)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 18).padding(.top, 10)

                    VStack(spacing: 7) {
                        ZStack {
                            ClothGhostAvatar(botId: "all-hands-green", size: 50).offset(x: -25, y: 7).rotationEffect(.degrees(-8))
                            ClothGhostAvatar(botId: "all-hands-violet", size: 50).offset(x: -1, y: 17).rotationEffect(.degrees(7))
                            ClothGhostAvatar(botId: "mahayana-assistant", size: 55).offset(x: 24, y: -1)
                            Text("+2").font(.system(size: 29, weight: .bold)).foregroundStyle(Color.black.opacity(0.34)).offset(x: 48, y: 28)
                        }.frame(width: 130, height: 82)
                        Text("All Hands").font(.system(size: 14)).foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity).padding(.top, 38).padding(.bottom, 34)

                    if searchOpen {
                        commandPaletteContent
                    }

                    if !searchOpen {
                        FabushiProductHub(
                            onOpenGlobalDharma: {
                                legacySection = .miniapps
                                legacyOpen = true
                            },
                            onOpenAI: { prompt in
                                if let prompt, !prompt.isEmpty {
                                    botDrafts["mahayana-assistant"] = prompt
                                }
                                selectBotForConversation(
                                    bots.first(where: { $0.id == "mahayana-assistant" })
                                        ?? MobileBotSummary(
                                            id: "mahayana-assistant",
                                            name: "Mahayana",
                                            description: "Ready to help"
                                        )
                                )
                            }
                        )
                        .padding(.bottom, 12)
                    }

                    if let botActionError {
                        Text(botActionError)
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .padding(.horizontal, 18)
                            .padding(.bottom, 8)
                            .accessibilityIdentifier("grok-mobile-bot-action-error")
                    }

                    if !searchOpen {
                        sectionTitle("Board")
                        botRow(MobileBotSummary(id: "mahayana-assistant", name: "Mahayana", description: "Ready to help"), subtitle: "that's the only new one.", badge: "Board")

                        if let rosterStatusKind,
                           rosterStatusKind == .empty || rosterStatusKind == .allHidden {
                            MobileRosterStatusView(
                                kind: rosterStatusKind,
                                onShowHiddenBots: { hiddenChatsOpen = true }
                            )
                        }

                        if !bots.isEmpty {
                            let sidebarProjection = MobileAgentSidebarSections.projected(
                                agentIds: filteredBots.map(\.id),
                                pinnedAgentIds: pinnedBotIds,
                                sections: agentSidebarSections,
                                searching: !query.isEmpty
                            )
                            ForEach(sidebarProjection) { section in
                                let sectionBots = filteredBots.filter { section.agentIds.contains($0.id) }
                                if section.id == MobileAgentSidebarSections.pinnedSectionID
                                    || section.id == MobileAgentSidebarSections.unassignedSectionID
                                {
                                    sectionTitle("\(section.name)  \(sectionBots.count)")
                                } else {
                                    agentSidebarSectionHeader(
                                        section,
                                        count: sectionBots.count
                                    )
                                }
                                if !section.isCollapsed {
                                    ForEach(sectionBots) { bot in
                                        botRow(
                                            bot,
                                            subtitle: mobileBotHomeSubtitle(bot),
                                            badge: bot.isGroup ? "Group" : (bot.miniAppId == nil ? "Bot" : "Mini App Bot")
                                        )
                                    }
                                }
                            }

                            let hiddenBots = bots.filter { $0.hidden }
                            Button {
                                hiddenChatsOpen = true
                            } label: {
                                Label(
                                    "隐藏的 Bots  \(hiddenBots.count)",
                                    systemImage: "eye.slash"
                                )
                                .font(.subheadline.weight(.semibold))
                                .padding(.horizontal, 18)
                                .padding(.vertical, 10)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Hidden Bots")
                            .accessibilityValue("\(hiddenBots.count)")
                            .accessibilityIdentifier("grok-hidden-bots")
                        }

                        let projects = filteredConversations.filter { $0.kind == .group || $0.kind == .direct }
                        if !projects.isEmpty {
                            sectionTitle("Projects  \(projects.count)")
                            ForEach(projects.prefix(8)) { conversation in conversationRow(conversation) }
                        }
                        let channels = filteredConversations.filter { $0.kind == .channel }
                        if !channels.isEmpty {
                            sectionTitle("Channels  \(channels.count)")
                            ForEach(channels.prefix(8)) { conversation in conversationRow(conversation) }
                        }
                    }
                    Spacer(minLength: 40)
                }
            }

            if isRosterPrivacyBlocked {
                MobileRosterPrivacyBlockedView(
                    onSignOut: { await model.logout() },
                    onOpenSettings: {
                        _ = openExternalURL(ROSTER_PRIVACY_SETTINGS_URL)
                    }
                )
            } else {
                AccessCoverView(
                    access: accessCoverComposition.access,
                    isVisible: accessCoverComposition.isVisible,
                    onOpenAccess: {
                        _ = openExternalURL(ACCESS_ONBOARDING_URL)
                    }
                )

                MobileCoordinatorConnectionNotice(
                    snapshot: coordinatorConnectionSnapshot,
                    onRetry: retryCoordinatorConnection
                )
                .padding(.horizontal, 18)
                .padding(.bottom, 18)
                .frame(maxHeight: .infinity, alignment: .bottom)
            }
        }
        .confirmationDialog("Create", isPresented: $composeOpen, titleVisibility: .visible) {
            Button("New Bot") {
                botName = "New chat"
                botDescription = ""
                botAvatarShape = "wedge"
                botAvatarColor = "cyan"
                botError = nil
                createBotOpen = true
            }
            Button("New message") { legacyOpen = true }
            Button("New group") { legacyOpen = true }
            Button("New channel") { legacyOpen = true }
            Button("Cancel", role: .cancel) { }
        }
        .sheet(isPresented: $createBotOpen) { createBotSheet }
        .sheet(isPresented: $hiddenChatsOpen) { hiddenChatsSheet }
        .sheet(item: $botRenameTarget) { bot in renameBotSheet(bot) }
        .sheet(item: $botDeleteTarget) { bot in botDeleteConfirmationSheet(bot) }
        .accessibilityIdentifier("grok-mobile-home")
    }

    var hiddenChatsSheet: some View {
        let hiddenBots = bots.filter { $0.hidden && !$0.isGroup && $0.miniAppId == nil }
        return NavigationStack {
            Group {
                if hiddenBots.isEmpty {
                    ContentUnavailableView(
                        "No hidden bots",
                        systemImage: "eye.slash",
                        description: Text("Hidden Bots stay active and keep their history; they are only removed from the home list.")
                    )
                    .accessibilityIdentifier("hidden-bots-empty")
                } else {
                    List(hiddenBots) { bot in
                        HStack(spacing: 12) {
                            Button {
                                hiddenChatsOpen = false
                                selectBotForConversation(bot)
                            } label: {
                                HStack(spacing: 12) {
                                    let projection = projectMobileSidebarAgentVisual(
                                        bot,
                                        isPinned: pinnedBotIds.contains(bot.id)
                                    )
                                    MobileAgentAvatar(bot: bot, size: 42, activeOverride: projection.isWorking, badge: mobileSidebarAgentBadgeColor(projection.statusBadge))
                                    VStack(alignment: .leading, spacing: 2) {
                                        HStack(spacing: 5) {
                                            Text(bot.name)
                                                .foregroundStyle(.primary)
                                                .lineLimit(1)
                                            if let title = projection.title {
                                                Text(title)
                                                    .font(.caption2)
                                                    .foregroundStyle(.secondary)
                                                    .lineLimit(1)
                                            }
                                            if projection.isPinned {
                                                Image(systemName: "pin.fill")
                                                    .font(.caption2)
                                                    .foregroundStyle(.secondary)
                                                    .accessibilityLabel("Pinned")
                                            }
                                        }
                                        if let statusLabel = projection.statusLabel {
                                            Text(statusLabel)
                                                .font(.caption2)
                                                .foregroundStyle(.secondary)
                                        }
                                    }
                                    Spacer()
                                }
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(bot.name)
                            .accessibilityIdentifier("hidden-bot-open-\(bot.id)")

                            Button("Unhide") {
                                Task { await setBotHidden(bot, hidden: false) }
                            }
                            .disabled(hiddenChatsController.isPending(bot.id))
                            .accessibilityLabel("Unhide \(bot.name)")
                            .accessibilityIdentifier("hidden-bot-unhide-\(bot.id)")
                        }
                        .accessibilityElement(children: .contain)
                        .accessibilityIdentifier("hidden-bot-row-\(bot.id)")
                    }
                }
            }
            .navigationTitle("Hidden Bots")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { hiddenChatsOpen = false }
                        .keyboardShortcut(.cancelAction)
                        .accessibilityIdentifier("hidden-bots-close")
                }
            }
        }
        .presentationDetents([.medium, .large])
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("hidden-bots-dialog")
    }

    var filteredBots: [MobileBotSummary] {
        let visible = bots.filter { !$0.hidden }
        let filtered = query.isEmpty
            ? visible
            : visible.filter {
                $0.name.localizedCaseInsensitiveContains(query)
                    || $0.description.localizedCaseInsensitiveContains(query)
            }
        let pinned = pinnedBotIds
        let pinnedRank = Dictionary(
            uniqueKeysWithValues: pinnedBotIdOrder.enumerated().map { ($0.element, $0.offset) }
        )
        return filtered.sorted {
            let lhsPinned = pinned.contains($0.id)
            let rhsPinned = pinned.contains($1.id)
            if lhsPinned != rhsPinned { return lhsPinned }
            if lhsPinned, rhsPinned {
                let lhsRank = pinnedRank[$0.id] ?? Int.max
                let rhsRank = pinnedRank[$1.id] ?? Int.max
                if lhsRank != rhsRank { return lhsRank < rhsRank }
            }
            let lhsUpdated = $0.updatedAtMs ?? 0
            let rhsUpdated = $1.updatedAtMs ?? 0
            if lhsUpdated != rhsUpdated { return lhsUpdated > rhsUpdated }
            return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
    }

    var filteredConversations: [ConversationSummary] {
        let rows = messaging.conversations.filter { !$0.isArchived }
        guard !query.isEmpty else { return rows }
        return rows.filter { $0.title.localizedCaseInsensitiveContains(query) || $0.preview.localizedCaseInsensitiveContains(query) }
    }

    func sectionTitle(_ text: String) -> some View {
        Text(text).font(.system(size: 16)).foregroundStyle(Color.black.opacity(0.42)).padding(.horizontal, 18).padding(.top, 13).padding(.bottom, 7)
    }

    func agentSidebarSectionHeader(
        _ section: MobileAgentSidebarSection,
        count: Int
    ) -> some View {
        HStack(spacing: 8) {
            Button {
                Task { await toggleAgentSidebarSectionCollapsed(section) }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: section.isCollapsed ? "chevron.right" : "chevron.down")
                        .font(.caption.weight(.semibold))
                    Text("\(section.name)  \(count)")
                        .font(.system(size: 16))
                }
                .foregroundStyle(Color.black.opacity(0.42))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(botActionBusy)
            .accessibilityLabel(section.isCollapsed ? "Expand \(section.name)" : "Collapse \(section.name)")
            .accessibilityValue("\(count) bots")
            .accessibilityIdentifier("agent-section-collapse-\(section.id)")
            Spacer()
            Menu {
                Button {
                    sectionRenameDraft = section.name
                    sectionRenameTarget = section
                } label: {
                    Label("重命名分组", systemImage: "pencil")
                }
                Button {
                    Task { await moveAgentSidebarSection(section, offset: -1) }
                } label: {
                    Label("上移分组", systemImage: "arrow.up")
                }
                .disabled(agentSidebarSections.first?.id == section.id)
                Button {
                    Task { await moveAgentSidebarSection(section, offset: 1) }
                } label: {
                    Label("下移分组", systemImage: "arrow.down")
                }
                .disabled(agentSidebarSections.last?.id == section.id)
                Divider()
                Button(role: .destructive) {
                    sectionDeleteTarget = section
                } label: {
                    Label("删除分组", systemImage: "trash")
                }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .foregroundStyle(.secondary)
                    .frame(width: 32, height: 32)
            }
            .accessibilityIdentifier("agent-section-actions-\(section.id)")
        }
        .padding(.leading, 18)
        .padding(.trailing, 10)
        .padding(.top, 13)
        .padding(.bottom, 7)
    }

    func botRow(_ bot: MobileBotSummary, subtitle: String, badge: String) -> some View {
        let projection = projectMobileSidebarAgentVisual(
            bot,
            isPinned: pinnedBotIds.contains(bot.id)
        )
        return HStack(spacing: 0) {
            Button {
                if bot.isGroup && !bot.isSharedRoom {
                    groupMembersTarget = bot
                } else {
                    selectBotForConversation(bot)
                }
            } label: {
                HStack(spacing: 12) {
                    MobileAgentAvatar(bot: bot, size: 47, activeOverride: projection.isWorking, badge: mobileSidebarAgentBadgeColor(projection.statusBadge))
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 7) {
                            Text(bot.name)
                                .font(.system(size: 17, weight: .semibold))
                                .foregroundStyle(.black)
                            if let title = projection.title {
                                Text(title)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .padding(.horizontal, 7)
                                    .padding(.vertical, 3)
                                    .background(Color.black.opacity(0.045), in: Capsule())
                            } else {
                                Text(badge)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .padding(.horizontal, 7)
                                    .padding(.vertical, 3)
                                    .background(Color.black.opacity(0.045), in: Capsule())
                            }
                            if projection.isPinned {
                                Image(systemName: "pin.fill")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .accessibilityLabel("Pinned")
                            }
                        }
                        Text(subtitle)
                            .font(.system(size: 14))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    Spacer()
                    if let status = projection.statusBadge {
                        Group {
                            switch status {
                            case .blocked:
                                Image(systemName: "exclamationmark.circle.fill")
                                    .foregroundStyle(.orange)
                            case .unread:
                                Circle()
                                    .fill(Color.accentColor)
                                    .frame(width: 8, height: 8)
                            case .working:
                                EmptyView()
                            }
                        }
                        .accessibilityLabel(projection.statusLabel ?? "")
                    }
                    if let updatedAtMs = bot.updatedAtMs, updatedAtMs > 0 {
                        Text(Date(timeIntervalSince1970: Double(updatedAtMs) / 1000), style: .relative)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                .padding(.leading, 18).padding(.vertical, 9)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(bot.name)
            .accessibilityValue(projection.statusLabel ?? "Idle")
            .accessibilityIdentifier(
                bot.miniAppId.map { "grok-mobile-miniapp-bot-\($0)" }
                    ?? "grok-mobile-bot-\(bot.id)"
            )

            if bot.miniAppId == nil {
                Menu {
                    Button {
                        botSettingsTarget = bot
                    } label: {
                        Label("设置", systemImage: "gearshape")
                    }
                    if MobileAgentSidebarSections.canAssign(
                        isPinned: pinnedBotIds.contains(bot.id),
                        isHidden: bot.hidden
                    ) {
                        Menu {
                            Button {
                                Task { await assignBot(bot, toSection: nil) }
                            } label: {
                                Label("未分组", systemImage: "tray")
                            }
                            ForEach(agentSidebarSections) { section in
                                Button {
                                    Task { await assignBot(bot, toSection: section.id) }
                                } label: {
                                    Label(section.name, systemImage: "folder")
                                }
                            }
                            Divider()
                            Button {
                                beginCreateAgentSidebarSection(for: bot)
                            } label: {
                                Label("新建分组…", systemImage: "folder.badge.plus")
                            }
                        } label: {
                            Label("移到分组", systemImage: "folder")
                        }
                    }
                    if let pinnedIndex = pinnedBotIdOrder.firstIndex(of: bot.id) {
                        Button {
                            Task { await movePinnedBot(bot, offset: -1) }
                        } label: {
                            Label("上移置顶", systemImage: "arrow.up")
                        }
                        .disabled(pinnedIndex == 0)

                        Button {
                            Task { await movePinnedBot(bot, offset: 1) }
                        } label: {
                            Label("下移置顶", systemImage: "arrow.down")
                        }
                        .disabled(pinnedIndex == pinnedBotIdOrder.count - 1)
                    }
                    Button {
                        Task { await toggleBotPin(bot) }
                    } label: {
                        Label(
                            pinnedBotIds.contains(bot.id) ? "取消置顶" : "置顶",
                            systemImage: pinnedBotIds.contains(bot.id) ? "pin.slash" : "pin"
                        )
                    }
                    if bot.isGroup {
                        Button {
                            groupMembersTarget = bot
                        } label: {
                            Label("成员", systemImage: "person.2")
                        }
                        Divider()
                        Button(role: .destructive) {
                            requestBotDelete(bot)
                        } label: {
                            Label("删除群组", systemImage: "trash")
                        }
                    } else {
                        Button {
                            beginBotRename(bot)
                        } label: {
                            Label("重命名", systemImage: "pencil")
                        }
                        if let conversationId = bot.conversationId, !conversationId.isEmpty {
                            Button {
                                openLegacyConversation(conversationId)
                            } label: {
                                Label("显示完整会话", systemImage: "list.bullet.rectangle")
                            }
                            Button {
                                UIPasteboard.general.string = conversationId
                            } label: {
                                Label("复制会话 ID", systemImage: "doc.on.doc")
                            }
                        }
                        Button {
                            showAsyncTasks(bot)
                        } label: {
                            Label("异步任务", systemImage: "clock")
                        }
                        Button {
                            Task { await setBotUnread(bot, unread: !bot.unread) }
                        } label: {
                            Label(
                                bot.unread ? "标记已读" : "标记未读",
                                systemImage: bot.unread ? "envelope.open" : "envelope.badge"
                            )
                        }
                        Button {
                            Task { await duplicateBot(bot) }
                        } label: {
                            Label("复制", systemImage: "square.on.square")
                        }
                        Button {
                            Task { await setBotHidden(bot, hidden: true) }
                        } label: {
                            Label("从首页隐藏", systemImage: "eye.slash")
                        }
                        Divider()
                        Button(role: .destructive) {
                            requestBotDelete(bot)
                        } label: {
                            Label("删除", systemImage: "trash")
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 16, weight: .semibold))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(botActionBusy)
                .accessibilityIdentifier("grok-bot-actions-\(bot.id)")
                .padding(.trailing, 6)
            }
        }
    }

    func conversationRow(_ conversation: ConversationSummary) -> some View {
        Button { openLegacyConversation(conversation.id) } label: {
            HStack(spacing: 12) {
                ClothGhostAvatar(botId: "conversation:\(conversation.id)", size: 45, badge: conversation.unreadCount > 0 ? .blue : nil)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 7) {
                        Text(conversation.title).font(.system(size: 17, weight: .semibold)).foregroundStyle(.black).lineLimit(1)
                        Text(conversation.kind == .channel ? "Channel" : "Engineering").font(.caption).foregroundStyle(.secondary).padding(.horizontal, 7).padding(.vertical, 3).background(Color.black.opacity(0.045), in: Capsule())
                    }
                    Text(conversation.preview.isEmpty ? "Ready" : conversation.preview).font(.system(size: 14)).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Text(conversation.time).font(.caption).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 18).padding(.vertical, 9)
        }.buttonStyle(.plain)
    }

    func renameBotSheet(_ bot: MobileBotSummary) -> some View {
        NavigationStack {
            Form {
                Section("名称") {
                    TextField("Bot 名称", text: $botRenameDraft)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("rename-bot-name")
                }
                if let botActionError {
                    Section {
                        Text(botActionError)
                            .foregroundStyle(.red)
                            .font(.footnote)
                            .accessibilityIdentifier("rename-bot-error")
                    }
                }
            }
            .navigationTitle("重命名 Bot")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") {
                        botRenameTarget = nil
                        botRenameDraft = ""
                        botActionError = nil
                    }
                    .disabled(botActionBusy)
                    .accessibilityIdentifier("rename-bot-cancel")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(botActionBusy ? "保存中…" : "保存") {
                        Task { await commitBotRename() }
                    }
                    .disabled(
                        botActionBusy
                            || committedMobileBotName(
                                initialValue: bot.name,
                                draftValue: botRenameDraft
                            ) == nil
                    )
                    .accessibilityIdentifier("rename-bot-submit")
                }
            }
        }
    }

    func botDeleteConfirmationSheet(_ bot: MobileBotSummary) -> some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 18) {
                Text(bot.isGroup ? "永久删除群组？" : "永久删除 Bot？")
                    .font(.title2.weight(.semibold))
                Text("“\(bot.name)”")
                    .font(.headline)
                Text(mobileBotDeleteDescription(bot))
                    .foregroundStyle(.secondary)

                if let botActionError {
                    Text(botActionError)
                        .foregroundStyle(.red)
                        .font(.footnote)
                        .accessibilityIdentifier("delete-bot-error")
                }

                Spacer()

                HStack {
                    Button("取消") {
                        guard !botActionBusy else { return }
                        botDeleteTarget = nil
                        botActionError = nil
                    }
                    .buttonStyle(.bordered)
                    .disabled(botActionBusy)
                    .accessibilityIdentifier("delete-bot-cancel")

                    Spacer()

                    Button(role: .destructive) {
                        Task { await deleteBot(bot) }
                    } label: {
                        Text(botActionBusy ? "删除中…" : "删除")
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(botActionBusy)
                    .accessibilityIdentifier("delete-bot-confirm")
                }
            }
            .padding(24)
            .navigationTitle("删除确认")
            .navigationBarTitleDisplayMode(.inline)
        }
        .interactiveDismissDisabled(botActionBusy)
        .presentationDetents([.medium])
        .accessibilityIdentifier("delete-bot-confirmation")
    }

    var createBotSheet: some View {
        NavigationStack {
            Form {
                Section {
                    HStack {
                        Spacer()
                        MobileOnboardingCharacter(
                            colorId: botAvatarColor,
                            shapeId: botAvatarShape,
                            size: 82,
                            state: botBusy ? .working : .idle
                        )
                        Spacer()
                    }
                }
                Section("Name") {
                    TextField("Bot name", text: $botName)
                        .accessibilityIdentifier("new-bot-name")
                }
                Section("Description") {
                    TextField("What does this Bot do?", text: $botDescription, axis: .vertical)
                        .lineLimit(2...5)
                        .accessibilityIdentifier("new-bot-description")
                }
                Section("Avatar shape") {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 4), spacing: 10) {
                        ForEach(AvatarImagePolicy.shapes, id: \.self) { shape in
                            Button {
                                botAvatarShape = shape
                            } label: {
                                Text(shape.capitalized)
                                    .font(.caption.weight(botAvatarShape == shape ? .bold : .regular))
                                    .frame(maxWidth: .infinity, minHeight: 36)
                                    .background(
                                        botAvatarShape == shape
                                            ? Color.accentColor.opacity(0.2)
                                            : Color.secondary.opacity(0.10),
                                        in: RoundedRectangle(cornerRadius: 9)
                                    )
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Avatar shape \(shape)")
                            .accessibilityValue(botAvatarShape == shape ? "Selected" : "Not selected")
                            .accessibilityIdentifier("new-bot-avatar-shape-\(shape)")
                        }
                    }
                }
                Section("Avatar color") {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 3), spacing: 10) {
                        ForEach(AvatarImagePolicy.colors, id: \.id) { color in
                            Button {
                                botAvatarColor = color.id
                            } label: {
                                Text(color.label)
                                    .font(.caption.weight(botAvatarColor == color.id ? .bold : .regular))
                                    .frame(maxWidth: .infinity, minHeight: 36)
                                    .background(
                                        botAvatarColor == color.id
                                            ? Color.accentColor.opacity(0.2)
                                            : Color.secondary.opacity(0.10),
                                        in: RoundedRectangle(cornerRadius: 9)
                                    )
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Avatar color \(color.label)")
                            .accessibilityValue(botAvatarColor == color.id ? "Selected" : "Not selected")
                            .accessibilityIdentifier("new-bot-avatar-color-\(color.id)")
                        }
                    }
                }
                if let botError { Section { Text(botError).foregroundStyle(.red).font(.footnote) } }
            }
            .navigationTitle("New Bot")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        createBotOpen = false
                        botError = nil
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(botBusy ? "Creating…" : "Create") { Task { await createBot() } }
                        .disabled(botBusy || botName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("create-bot-submit")
                }
            }
        }
    }
}

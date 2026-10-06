import AuthenticationServices
import Foundation
import Observation
import UIKit

enum MobileChatRole: String, Equatable {
    case user
    case assistant
}

enum MobileChatEntryKind: String, Equatable {
    case message
    case action
    case thinking
    case handoff
    case notice
    case permissionRequest
    case timelineEvent
}

enum MahayanaChatPumpOutcome: Equatable {
    case terminal
    case nonTerminal

    var shouldSettleLifecycle: Bool { self == .terminal }
}

struct MobileChatMessage: Identifiable, Equatable {
    let id: String
    let role: MobileChatRole
    var text: String
    var kind: MobileChatEntryKind = .message
    var operationId: String?
    var actionTitle: String?
    var actionDetail: String?
    var actionStatus: String?
    var handoffRequestId: String?
    var handoffAgentId: String?
    var canonicalMessageId: String?
    var replyToMessageId: String?
    var attachmentBatchId: String?
    var attachmentURL: String?
    var attachmentFileName: String?
    var attachmentAlt: String?
    var timelineEvent: SandTimelineEvent?
    var timelineAutomationId: String?
    var branched = false
    var streaming = false
    var createdAt = Date()
}

private func mobileTranscriptCardDate(_ value: Any?) -> Date {
    if let milliseconds = value as? NSNumber {
        return Date(timeIntervalSince1970: milliseconds.doubleValue / 1_000)
    }
    return Date()
}

private func projectMobileTimelineEvent(
    _ raw: [String: Any]
) -> (event: SandTimelineEvent, automationId: String?)? {
    guard let type = raw["type"] as? String else { return nil }
    switch type {
    case "name-changed":
        guard let to = raw["to"] as? String else { return nil }
        return (.nameChanged(to: to), nil)
    case "channel-connected":
        guard let label = raw["label"] as? String else { return nil }
        return (.channelConnected(label: label), nil)
    case "channel-disconnected":
        guard let label = raw["label"] as? String else { return nil }
        return (.channelDisconnected(label: label), nil)
    case "automation-changed":
        guard let automationId = raw["automationId"] as? String, !automationId.isEmpty,
              let action = raw["action"] as? String, !action.isEmpty,
              let automationName = raw["automationName"] as? String, !automationName.isEmpty
        else { return nil }
        return (.automationChanged(action: action, automationName: automationName), automationId)
    default:
        return nil
    }
}

/// Native projection for the recovered Desktop transcript-card family.
/// Unknown/malformed cards fail closed rather than becoming generic messages.
func projectMobileTranscriptCard(
    event: [String: Any],
    operationId: String?
) -> MobileChatMessage? {
    guard let card = event["card"] as? [String: Any],
          let kind = card["kind"] as? String
    else { return nil }

    if kind == "listenerConnect" {
        return projectListenerConnectTranscriptCard(event: event, operationId: operationId)
    }

    let entryId = (event["entryId"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? (card["id"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? "transcript-card:\(UUID().uuidString.lowercased())"
    let createdAt = mobileTranscriptCardDate(event["timestampMs"] ?? card["timestampMs"])

    switch kind {
    case "notice":
        guard let text = card["text"] as? String, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return MobileChatMessage(
            id: entryId, role: .assistant, text: text, kind: .notice,
            operationId: operationId, createdAt: createdAt
        )

    case "permissionRequest", "permission-request":
        let nestedTitle = (card["permission"] as? [String: Any])?["title"] as? String
        guard let title = (nestedTitle ?? card["title"] as? String),
              !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }
        return MobileChatMessage(
            id: entryId, role: .assistant, text: title, kind: .permissionRequest,
            operationId: operationId, createdAt: createdAt
        )

    case "timelineEvent", "timeline-event", "event":
        guard let rawEvent = card["event"] as? [String: Any],
              let projection = projectMobileTimelineEvent(rawEvent)
        else { return nil }
        return MobileChatMessage(
            id: entryId, role: .assistant, text: describeTimelineEvent(projection.event),
            kind: .timelineEvent, operationId: operationId,
            timelineEvent: projection.event, timelineAutomationId: projection.automationId,
            createdAt: createdAt
        )

    default:
        return nil
    }
}

func projectListenerConnectTranscriptCard(
    event: [String: Any],
    operationId: String?
) -> MobileChatMessage? {
    guard let card = event["card"] as? [String: Any],
          card["kind"] as? String == "listenerConnect",
          let platform = card["platform"] as? String,
          !platform.isEmpty
    else { return nil }

    let displayName: String
    switch platform.lowercased() {
    case "slack": displayName = "Slack"
    case "github": displayName = "GitHub"
    case "git": displayName = "Git"
    case "teams": displayName = "Microsoft Teams"
    case "linear": displayName = "Linear"
    case "sentry": displayName = "Sentry"
    case "pagerduty": displayName = "PagerDuty"
    default: displayName = platform
    }

    let connected = card["connected"] as? Bool ?? false
    let pending = card["pending"] as? Bool ?? false
    let reason = (card["reason"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
    let detail = reason?.isEmpty == false
        ? reason
        : (connected ? "\(displayName) 已连接。" : "连接 \(displayName) 后，此例程才能接收对应事件。")
    let entryId = (event["entryId"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? "listener-connect:\(platform.lowercased())"

    return MobileChatMessage(
        id: entryId,
        role: .assistant,
        text: "",
        kind: .action,
        operationId: operationId,
        actionTitle: connected ? "\(displayName) 已连接" : "连接 \(displayName)",
        actionDetail: detail,
        actionStatus: connected ? "completed" : (pending ? "pending" : "waiting")
    )
}

struct MiniAppToolContract: Equatable, Sendable {
    let name: String
    let description: String
    let approval: String
    let title: String?
    let inputSchemaJSON: String?

    init(
        name: String,
        description: String,
        approval: String,
        title: String? = nil,
        inputSchemaJSON: String? = nil
    ) {
        self.name = name
        self.description = description
        self.approval = approval
        self.title = title
        self.inputSchemaJSON = inputSchemaJSON
    }

    var inputSchemaObject: [String: Any]? {
        guard let inputSchemaJSON,
              let data = inputSchemaJSON.data(using: .utf8),
              let raw = try? JSONSerialization.jsonObject(with: data),
              let object = raw as? [String: Any]
        else { return nil }
        return object
    }
}

struct MarketplacePlugin: Identifiable, Equatable, Sendable {
    let pluginId: String
    let displayName: String
    let description: String
    let latestVersion: String?
    let sourceRef: String?
    let tools: [MiniAppToolContract]

    init(
        pluginId: String,
        displayName: String,
        description: String,
        latestVersion: String?,
        sourceRef: String? = nil,
        tools: [MiniAppToolContract]
    ) {
        self.pluginId = pluginId
        self.displayName = displayName
        self.description = description
        self.latestVersion = latestVersion
        self.sourceRef = sourceRef
        self.tools = tools
    }

    var id: String { pluginId }

    func replacingTools(_ tools: [MiniAppToolContract]) -> MarketplacePlugin {
        MarketplacePlugin(
            pluginId: pluginId,
            displayName: displayName,
            description: description,
            latestVersion: latestVersion,
            sourceRef: sourceRef,
            tools: tools
        )
    }
}

struct PluginPermissionRequest: Identifiable, Equatable {
    let pluginId: String
    let runtime: String
    let permissions: [String]
    var id: String { pluginId }
}

private final class BrowserAuthPresentationContext: NSObject, ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        if let keyWindow = scenes.flatMap(\.windows).first(where: \.isKeyWindow) {
            return keyWindow
        }
        if let window = scenes.flatMap(\.windows).first {
            return window
        }
        return ASPresentationAnchor()
    }
}

@MainActor
@Observable
final class MarketplaceModel {
    var query = ""
    var message = "Mahayana Rust Host 正在启动"
    var loading = false
    var installingPluginId: String?
    var plugins: [MarketplacePlugin] = []
    var permissionRequest: PluginPermissionRequest?
    var featureHostSmokeStatus: String?
    var authResolved = false
    var loggedIn = false
    var accountName = "Fabushi"
    var accountEmail = ""
    var accountUsage: AccountUsageProjection?
    var accountUsageLoading = false
    var accountUsageError: String?
    var onboardingStep: Int
    var browserLoginAttemptId: String?
    var browserLoginURL: URL?
    var loginBusy = false
    var loginError: String?
    var chatDraft = ""
    var chatMessages: [MobileChatMessage] = []
    var chatBusy = false
    var activeOperationId: String?
    let globalDharmaCommerce: GlobalDharmaCommerceModel

    private let bridge: IOSPreloadBridge
    private let globalDharmaBridge: GlobalDharmaMiniAppBridge
    private let onboardingKey = "fabushi.mobile.onboarding-complete.v1"
    @ObservationIgnored private var globalDharmaAccountScope: String?
    @ObservationIgnored private var globalDharmaExecution: [String: Any]?
    private static let globalDharmaExecutionKeyPrefix = "fabushi.ios.miniapp-execution.v1:"
    @ObservationIgnored private let browserAuthPresentationContext = BrowserAuthPresentationContext()
    @ObservationIgnored private var webAuthenticationSession: ASWebAuthenticationSession?

    init(bridge: IOSPreloadBridge) {
        self.bridge = bridge
        globalDharmaBridge = GlobalDharmaMiniAppBridge(bridge: bridge)
        globalDharmaCommerce = GlobalDharmaCommerceModel(bridge: bridge)
        onboardingStep = UserDefaults.standard.bool(forKey: onboardingKey) ? 3 : 0
    }

    static func nextGlobalDharmaExecution(
        previous: [String: Any]?,
        tool: String,
        result: Any,
        source: String
    ) -> [String: Any] {
        let previousRevision = (previous?["revision"] as? NSNumber)?.intValue ?? 0
        return [
            "protocol": "fabushi.miniapp.execution.v1",
            "miniAppId": GlobalDharmaMiniAppBridge.globalDharmaId,
            "revision": previousRevision + 1,
            "source": source,
            "phase": "completed",
            "tool": tool,
            "result": result,
        ]
    }

    static func globalDharmaRuntime(from execution: [String: Any]) -> [String: Any]? {
        guard execution["protocol"] as? String == "fabushi.miniapp.execution.v1",
              execution["miniAppId"] as? String == GlobalDharmaMiniAppBridge.globalDharmaId,
              let revision = (execution["revision"] as? NSNumber)?.intValue,
              revision > 0
        else { return nil }
        return [
            "protocol": "fabushi.miniapp.runtime.v1",
            "miniAppId": GlobalDharmaMiniAppBridge.globalDharmaId,
            "revision": revision,
            "state": execution,
        ]
    }

    static func bridgeGlobalDharmaStatusResult(
        _ result: [String: Any],
        runtime: [String: Any]
    ) -> [String: Any] {
        var bridged = result
        var structured = (result["structuredContent"] as? [String: Any]) ?? [:]
        structured["runtime"] = runtime
        bridged["structuredContent"] = structured
        return bridged
    }

    private static let canonicalAccountIdentityKeys = [
        "principalId",
        "principal_id",
        "id",
        "userId",
        "user_id",
        "userNo",
        "user_no",
        "username",
    ]

    private static func stableGlobalDharmaAccountIdentity(in auth: [String: Any]) -> String? {
        if let user = auth["user"] as? [String: Any],
           let identity = stableGlobalDharmaIdentityComponent(in: user) {
            return identity
        }
        return stableGlobalDharmaIdentityComponent(in: auth)
    }

    private static func stableGlobalDharmaIdentityComponent(in object: [String: Any]) -> String? {
        for key in canonicalAccountIdentityKeys {
            guard let raw = object[key] else { continue }
            if let value = raw as? String {
                let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmed.isEmpty { return trimmed }
                continue
            }
            if let number = raw as? NSNumber,
               CFGetTypeID(number) != CFBooleanGetTypeID() {
                return number.stringValue
            }
        }
        return nil
    }

    static func globalDharmaScope(for auth: [String: Any]) -> String? {
        guard let raw = stableGlobalDharmaAccountIdentity(in: auth) else { return nil }
        return Data(raw.utf8)
            .base64EncodedString()
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "=", with: "")
    }

    private static func globalDharmaExecutionKey(scope: String) -> String {
        globalDharmaExecutionKeyPrefix + scope
    }

    private static func loadGlobalDharmaExecution(scope: String) -> [String: Any]? {
        let key = globalDharmaExecutionKey(scope: scope)
        guard let data = UserDefaults.standard.data(forKey: key),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              object["protocol"] as? String == "fabushi.miniapp.execution.v1",
              object["miniAppId"] as? String == GlobalDharmaMiniAppBridge.globalDharmaId,
              ((object["revision"] as? NSNumber)?.intValue ?? 0) > 0
        else { return nil }
        return object
    }

    func recordGlobalDharmaExecution(tool: String, result: Any, source: String) {
        guard loggedIn, let scope = globalDharmaAccountScope else { return }
        let execution = Self.nextGlobalDharmaExecution(
            previous: globalDharmaExecution,
            tool: tool,
            result: result,
            source: source
        )
        guard JSONSerialization.isValidJSONObject(execution),
              let data = try? JSONSerialization.data(withJSONObject: execution)
        else { return }
        globalDharmaExecution = execution
        UserDefaults.standard.set(data, forKey: Self.globalDharmaExecutionKey(scope: scope))
    }

    func globalDharmaSharedRuntime() throws -> [String: Any] {
        guard loggedIn,
              let execution = globalDharmaExecution,
              let runtime = Self.globalDharmaRuntime(from: execution)
        else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Global Dharma has no completed account-scoped Bot execution to restore"
            )
        }
        return runtime
    }

    func initializeApp() async {
        authResolved = false
        do {
            let result = try await bridge.request(method: "feature.auth.status")
            applyAuth(result.value as? [String: Any])
            authResolved = true
            if loggedIn {
                await refreshAccountUsage()
                await refresh()
            }
        } catch {
            authResolved = true
            message = "账号状态加载失败：\(error.localizedDescription)"
        }
    }

    private func applyAuth(_ object: [String: Any]?, defaultLoggedIn: Bool = false) {
        let auth = (object?["auth"] as? [String: Any]) ?? object
        loggedIn = auth?["loggedIn"] as? Bool ?? defaultLoggedIn
        if !loggedIn {
            accountUsage = nil
            accountUsageError = nil
        }

        if loggedIn, let auth, let scope = Self.globalDharmaScope(for: auth) {
            globalDharmaAccountScope = scope
            globalDharmaExecution = Self.loadGlobalDharmaExecution(scope: scope)
        } else {
            // Account-scoped Mini App state must fail closed whenever the Host
            // cannot supply the same stable account identity used by the
            // Coordinator. Never retain a previous account's runtime.
            globalDharmaAccountScope = nil
            globalDharmaExecution = nil
        }

        guard let user = auth?["user"] as? [String: Any] else {
            accountName = "Fabushi"
            accountEmail = ""
            return
        }
        accountName = (user["nickname"] as? String)
            ?? (user["username"] as? String)
            ?? (user["email"] as? String)
            ?? "Fabushi"
        accountEmail = user["email"] as? String ?? ""
    }

    func advanceOnboarding() {
        onboardingStep = min(3, onboardingStep + 1)
        if onboardingStep == 3 { UserDefaults.standard.set(true, forKey: onboardingKey) }
    }

    func retreatOnboarding() { onboardingStep = max(0, onboardingStep - 1) }

    func beginBrowserLogin() async {
        guard !loginBusy else { return }
        loginBusy = true
        loginError = nil
        do {
            let result = try await bridge.request(method: "feature.auth.browserStart")
            guard let object = result.value as? [String: Any],
                  let attemptId = object["attemptId"] as? String,
                  let loginURLString = (object["loginUrl"] as? String) ?? (object["authorizationUrl"] as? String),
                  let loginURL = URL(string: loginURLString)
            else { throw MahayanaCoordinator.CoordinatorError.invalidResponse }
            browserLoginAttemptId = attemptId
            browserLoginURL = loginURL
            loginBusy = false
            if loginURLString.hasPrefix("about:blank#fabushi-test-browser-login") {
                await completeBrowserLogin(attemptId: attemptId)
            } else {
                presentBrowserLogin(loginURL)
            }
        } catch {
            browserLoginAttemptId = nil
            browserLoginURL = nil
            loginBusy = false
            loginError = error.localizedDescription
        }
    }

    func reopenBrowserLogin() async {
        guard let attemptId = browserLoginAttemptId else { return }
        do {
            let result = try await bridge.request(method: "feature.auth.browserReopen", params: ["attemptId": attemptId])
            guard let object = result.value as? [String: Any],
                  let loginURLString = (object["loginUrl"] as? String) ?? (object["authorizationUrl"] as? String),
                  let loginURL = URL(string: loginURLString)
            else { throw MahayanaCoordinator.CoordinatorError.invalidResponse }
            browserLoginURL = loginURL
            if loginURLString.hasPrefix("about:blank#fabushi-test-browser-login") {
                await completeBrowserLogin(attemptId: attemptId)
            } else {
                presentBrowserLogin(loginURL)
            }
        } catch { loginError = error.localizedDescription }
    }

    func cancelBrowserLogin() async {
        webAuthenticationSession?.cancel()
        webAuthenticationSession = nil
        await cancelBrowserLoginAttempt()
    }

    private func cancelBrowserLoginAttempt() async {
        guard let attemptId = browserLoginAttemptId else { return }
        do {
            _ = try await bridge.request(method: "feature.auth.browserCancel", params: ["attemptId": attemptId])
        } catch { loginError = error.localizedDescription }
        browserLoginAttemptId = nil
        browserLoginURL = nil
        loginBusy = false
        message = "登录授权已取消"
    }

    private func presentBrowserLogin(_ loginURL: URL) {
        webAuthenticationSession?.cancel()
        let session = ASWebAuthenticationSession(
            url: loginURL,
            callbackURLScheme: "fabushi"
        ) { [weak self] callbackURL, error in
            Task { @MainActor in
                guard let self else { return }
                self.webAuthenticationSession = nil
                if let callbackURL {
                    self.handleDeepLink(callbackURL)
                    return
                }
                if let authError = error as? ASWebAuthenticationSessionError, authError.code == .canceledLogin {
                    await self.cancelBrowserLoginAttempt()
                    return
                }
                if let error {
                    self.loginError = error.localizedDescription
                    self.message = "登录页面未能完成，请重试"
                }
            }
        }
        session.presentationContextProvider = browserAuthPresentationContext
        session.prefersEphemeralWebBrowserSession = false
        webAuthenticationSession = session
        if !session.start() {
            webAuthenticationSession = nil
            loginError = "无法打开应用内登录页面"
            message = "登录页面未能打开，请重试"
        }
    }

    func runFeatureHostSmokeIfRequested() async {
        guard ProcessInfo.processInfo.environment["FABUSHI_FEATURE_HOST_SMOKE"] == "1" else { return }
        featureHostSmokeStatus = "running"
        do {
            let infoResult = try await bridge.request(method: "feature.info")
            guard let info = infoResult.value as? [String: Any],
                  info["platform"] as? String == "ios",
                  let protocolVersion = info["protocolVersion"] as? String,
                  !protocolVersion.isEmpty,
                  (info["runtimeVersion"] as? String)?.contains("test") == true
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }

            _ = try await bridge.request(method: "feature.auth.status")
            let providers = try await bridge.request(method: "feature.auth.providers")
            guard let providerRows = providers.value as? [[String: Any]],
                  providerRows.contains(where: { $0["id"] as? String == "google" })
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }

            let oauth = try await bridge.request(
                method: "feature.auth.oauthStart",
                params: ["provider": "google"]
            )
            guard let oauthObject = oauth.value as? [String: Any],
                  let attemptId = oauthObject["attemptId"] as? String
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            let oauthCompleted = try await bridge.request(
                method: "feature.auth.oauthPoll",
                params: ["attemptId": attemptId]
            )
            guard let completedObject = oauthCompleted.value as? [String: Any],
                  completedObject["status"] as? String == "completed"
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }

            _ = try await executeFeatureCommand(
                type: "chat.send",
                requestId: "ios-chat",
                fields: ["text": "请用一句话说明自动化测试状态"]
            )
            _ = try await executeFeatureCommand(
                type: "marketplace.install",
                requestId: "ios-install",
                fields: ["miniAppId": "global-dharma"]
            )
            _ = try await executeFeatureCommand(
                type: "miniapp.open",
                requestId: "ios-open",
                fields: ["miniAppId": "global-dharma"]
            )
            _ = try await executeFeatureCommand(
                type: "capability.request",
                requestId: "ios-capability",
                fields: [
                    "miniAppId": "global-dharma",
                    "capability": "camera",
                    "reason": "cross-platform UI automation",
                ]
            )
            let approval = try await receiveFeatureEvent(type: "approval.requested")
            guard let approvalId = approval["approvalId"] as? String else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            _ = try await bridge.request(
                method: "feature.approval.resolve",
                params: [
                    "resolution": [
                        "approvalId": approvalId,
                        "decision": "allow-once",
                    ],
                ]
            )

            let longTask = try await executeFeatureCommand(
                type: "runtime.longTask",
                requestId: "ios-long-task",
                fields: ["label": "iOS simulated user operation"]
            )
            guard let operationId = longTask["operationId"] as? String else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            _ = try await bridge.request(
                method: "feature.interrupt",
                params: ["operationId": operationId]
            )

            _ = try await executeFeatureCommand(
                type: "session.clear",
                requestId: "ios-session-clear"
            )
            featureHostSmokeStatus = "passed"
        } catch {
            featureHostSmokeStatus = "failed: \(error.localizedDescription)"
        }
    }

    private func executeFeatureCommand(
        type: String,
        requestId: String,
        fields: [String: Any] = [:]
    ) async throws -> [String: Any] {
        var command = fields
        command["type"] = type
        command["requestId"] = requestId
        let result = try await bridge.request(
            method: "feature.execute",
            params: ["command": command]
        )
        guard let accepted = result.value as? [String: Any],
              accepted["requestId"] as? String == requestId
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return accepted
    }

    private func receiveFeatureEvent(type expectedType: String) async throws -> [String: Any] {
        let result = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 5_120
        ) { event in
            event["type"] as? String == expectedType
        }
        guard let event = result.value as? [String: Any] else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return event
    }

    func handleDeepLink(_ url: URL) {
        guard url.scheme?.lowercased() == "fabushi",
              url.user == nil,
              url.password == nil,
              url.port == nil,
              url.host?.lowercased() == "auth"
        else { return }
        let parts = url.pathComponents.filter { $0 != "/" && !$0.isEmpty }
        guard parts == ["complete"], let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return }
        let allowedNames = Set(["attemptId", "status"])
        var params: [String: String] = [:]
        for item in components.queryItems ?? [] {
            guard allowedNames.contains(item.name), params[item.name] == nil, let value = item.value else { return }
            params[item.name] = value
        }
        let attemptId = params["attemptId"] ?? ""
        let status = (params["status"] ?? "completed").lowercased()
        guard attemptId.range(of: "^[A-Za-z0-9_-]{8,96}$", options: .regularExpression) != nil,
              ["completed", "cancelled", "failed"].contains(status)
        else { return }
        message = status == "completed" ? "登录授权已完成，正在同步账号状态" : "登录授权状态：\(status)"
        if status == "completed" { Task { await completeBrowserLogin(attemptId: attemptId) } }
    }

    func completeBrowserLogin(attemptId: String) async {
        message = "登录授权已完成，正在通过 Rust Host 同步账号状态"
        do {
            let result = try await bridge.request(
                method: "feature.auth.browserPoll",
                params: ["attemptId": attemptId]
            )
            guard let object = result.value as? [String: Any],
                  let status = object["status"] as? String
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            switch status {
            case "completed":
                if let auth = object["auth"] as? [String: Any] {
                    applyAuth(auth, defaultLoggedIn: true)
                } else {
                    loggedIn = true
                }
                browserLoginAttemptId = nil
                browserLoginURL = nil
                webAuthenticationSession = nil
                loginError = nil
                await refreshAccountUsage()
                await refresh()
                message = "登录成功，账号状态已同步"
            case "cancelled":
                message = "登录授权已取消"
            case "failed":
                message = "登录授权失败"
            default:
                message = "登录结果尚未可用，请返回浏览器重试"
            }
        } catch {
            message = "登录状态同步失败：\(error.localizedDescription)"
        }
    }

    func logout() async {
        if let operationId = activeOperationId {
            _ = try? await bridge.request(method: "feature.interrupt", params: ["operationId": operationId])
        }
        do {
            let scopeToClear = globalDharmaAccountScope
            let result = try await bridge.request(method: "feature.auth.logout")
            if let scopeToClear {
                UserDefaults.standard.removeObject(
                    forKey: Self.globalDharmaExecutionKey(scope: scopeToClear)
                )
            }
            applyAuth(result.value as? [String: Any])
        } catch {
            message = "退出登录失败：\(error.localizedDescription)"
            return
        }
        loggedIn = false
        accountUsage = nil
        accountUsageError = nil
        chatMessages = []
        activeOperationId = nil
        chatBusy = false
        message = "已退出登录"
    }

    func sendChat() async {
        let text = chatDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, loggedIn, !chatBusy else { return }
        chatDraft = ""
        chatBusy = true
        let requestId = "ios-chat-\(UUID().uuidString.lowercased())"
        chatMessages.append(MobileChatMessage(id: requestId, role: .user, text: text))
        do {
            let accepted = try await executeFeatureCommand(
                type: "chat.send",
                requestId: requestId,
                fields: ["text": text, "agentId": "mahayana-assistant", "mode": "agent"]
            )
            let operationId = accepted["operationId"] as? String ?? requestId
            activeOperationId = operationId
            chatMessages.append(MobileChatMessage(
                id: "thinking:\(operationId)", role: .assistant, text: "", kind: .thinking, operationId: operationId,
                actionTitle: "正在思考", actionStatus: "running"
            ))
            let outcome = await pumpChatEvents(operationId: operationId)
            if outcome.shouldSettleLifecycle {
                chatBusy = false
                activeOperationId = nil
            }
        } catch is CancellationError {
            // View-driven cancellation is a normal lifecycle path.
            if activeOperationId == nil {
                chatBusy = false
                activeOperationId = nil
            }
            return
        } catch {
            message = "发送失败：\(error.localizedDescription)"
            if activeOperationId == nil {
                chatBusy = false
                activeOperationId = nil
            }
            return
        }
    }

    func resolveBoxHandoff(_ entry: MobileChatMessage, resolution: String) async {
        guard entry.actionStatus == "pending",
              let handoffRequestId = entry.handoffRequestId,
              let handoffAgentId = entry.handoffAgentId
        else { return }
        do {
            let accepted = try await executeFeatureCommand(
                type: "box.handoff.resolve",
                requestId: "ios-box-handoff-\(UUID().uuidString.lowercased())",
                fields: [
                    "handoffRequestId": handoffRequestId,
                    "agentId": handoffAgentId,
                    "resolution": resolution,
                ]
            )
            if let index = chatMessages.firstIndex(where: { $0.handoffRequestId == handoffRequestId }) {
                chatMessages[index].actionStatus = resolution
            }
            guard let operationId = accepted["operationId"] as? String, !operationId.isEmpty else { return }
            chatBusy = true
            activeOperationId = operationId
            chatMessages.append(MobileChatMessage(
                id: "thinking:\(operationId)",
                role: .assistant,
                text: "",
                kind: .thinking,
                operationId: operationId,
                actionTitle: "正在从接管状态恢复",
                actionStatus: "running"
            ))
            let outcome = await pumpChatEvents(operationId: operationId)
            if outcome.shouldSettleLifecycle {
                chatBusy = false
                activeOperationId = nil
            }
        } catch {
            message = "恢复 Agent 失败：\(error.localizedDescription)"
        }
    }

    func stopChat() async {
        guard let operationId = activeOperationId else { return }
        _ = try? await bridge.request(method: "feature.interrupt", params: ["operationId": operationId])
    }

    private func pumpChatEvents(operationId: String) async -> MahayanaChatPumpOutcome {
        for _ in 0..<1800 {
            if Task.isCancelled { return .nonTerminal }
            do {
                let ownedHandoffRequestIDs = Set(
                    chatMessages.compactMap(\.handoffRequestId)
                )
                let result = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 450_000
                ) { event in
                    guard let type = event["type"] as? String else { return false }
                    if type == "box.handoff.resolved" {
                        guard let requestID = event["requestId"] as? String else { return false }
                        return ownedHandoffRequestIDs.contains(requestID)
                    }
                    let acceptedTypes: Set<String> = [
                        "box.handoff.requested",
                        "model.routed",
                        "operation.started",
                        "chat.message",
                        "chat.delta",
                        "agent.step",
                        "transcript.card",
                        "operation.completed",
                        "operation.interrupted",
                        "operation.failed",
                    ]
                    guard acceptedTypes.contains(type) else { return false }
                    return (event["operationId"] as? String ?? operationId) == operationId
                }
                guard let event = result.value as? [String: Any],
                      let type = event["type"] as? String
                else { continue }
                switch type {
                case "box.handoff.requested":
                    let eventOperationId = event["operationId"] as? String ?? operationId
                    guard eventOperationId == operationId,
                          let requestId = event["requestId"] as? String,
                          let agentId = event["agentId"] as? String
                    else { continue }
                    let row = MobileChatMessage(
                        id: "handoff:\(requestId)",
                        role: .assistant,
                        text: event["instruction"] as? String ?? "请完成 Agent 请求的本机步骤。",
                        kind: .handoff,
                        operationId: operationId,
                        actionTitle: "等待用户接管",
                        actionDetail: [event["reason"] as? String, event["domain"] as? String].compactMap { $0 }.joined(separator: " · "),
                        actionStatus: "pending",
                        handoffRequestId: requestId,
                        handoffAgentId: agentId
                    )
                    if let index = chatMessages.firstIndex(where: { $0.handoffRequestId == requestId }) { chatMessages[index] = row } else { chatMessages.append(row) }
                case "box.handoff.resolved":
                    guard let requestId = event["requestId"] as? String else { continue }
                    if let index = chatMessages.firstIndex(where: { $0.handoffRequestId == requestId }) {
                        chatMessages[index].actionStatus = event["resolution"] as? String ?? "completed"
                    }
                case "model.routed":
                    guard (event["operationId"] as? String ?? operationId) == operationId else { continue }
                    let provider = event["provider"] as? String ?? ""
                    let model = event["model"] as? String ?? ""
                    upsertAction(operationId: operationId, stepId: "model-route", title: "选择模型", detail: [provider, model].filter { !$0.isEmpty }.joined(separator: " · "), status: "completed")
                case "operation.started":
                    guard event["operationId"] as? String == operationId else { continue }
                    if !chatMessages.contains(where: { $0.kind == .thinking && $0.operationId == operationId }) {
                        chatMessages.append(MobileChatMessage(id: "thinking:\(operationId)", role: .assistant, text: "", kind: .thinking, operationId: operationId, actionTitle: event["label"] as? String ?? "正在思考", actionStatus: "running"))
                    }
                case "chat.message":
                    let eventOperationId = event["operationId"] as? String ?? operationId
                    guard eventOperationId == operationId else { continue }
                    let role = (event["role"] as? String) == "user" ? MobileChatRole.user : .assistant
                    let eventText = event["text"] as? String ?? ""
                    if role == .assistant {
                        removeThinking(operationId: operationId)
                        let generatedAttachment = event["attachment"] as? [String: Any]
                        if eventText.isEmpty, generatedAttachment != nil, !chatMessages.contains(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
                            chatMessages.append(MobileChatMessage(id: "assistant:\(operationId)", role: .assistant, text: "", operationId: operationId))
                        } else {
                            upsertAssistantMessage(operationId: operationId, text: eventText, append: false)
                        }
                        if let index = chatMessages.lastIndex(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
                            chatMessages[index].canonicalMessageId = event["messageId"] as? String
                            chatMessages[index].replyToMessageId = event["replyToMessageId"] as? String
                            chatMessages[index].attachmentBatchId = event["attachmentBatchId"] as? String
                            if let attachment = event["attachment"] as? [String: Any] {
                                chatMessages[index].attachmentURL = attachment["url"] as? String
                                chatMessages[index].attachmentFileName = attachment["file_name"] as? String
                                chatMessages[index].attachmentAlt = attachment["alt"] as? String
                            }
                            chatMessages[index].branched = event["branched"] as? Bool ?? false
                        }
                    } else if !chatMessages.contains(where: { $0.role == .user && $0.text == eventText }) {
                        chatMessages.append(MobileChatMessage(id: "user:\(UUID().uuidString)", role: .user, text: eventText))
                    }
                case "chat.delta":
                    guard event["operationId"] as? String == operationId else { continue }
                    removeThinking(operationId: operationId)
                    upsertAssistantMessage(operationId: operationId, text: event["delta"] as? String ?? "", append: true)
                case "agent.step":
                    let eventOperationId = event["operationId"] as? String ?? operationId
                    guard eventOperationId == operationId else { continue }
                    let title = event["title"] as? String ?? "助手动作"
                    let stepId = event["stepId"] as? String ?? "step-\(UUID().uuidString)"
                    upsertAction(operationId: operationId, stepId: stepId, title: title, detail: event["detail"] as? String, status: event["status"] as? String ?? "completed")
                case "transcript.card":
                    guard let row = projectMobileTranscriptCard(
                        event: event,
                        operationId: event["operationId"] as? String ?? operationId
                    ) else { continue }
                    if let index = chatMessages.firstIndex(where: { $0.id == row.id }) {
                        chatMessages[index] = row
                    } else {
                        chatMessages.append(row)
                    }
                case "operation.completed", "operation.interrupted":
                    guard event["operationId"] as? String == operationId else { continue }
                    removeThinking(operationId: operationId)
                    settleActions(operationId: operationId, status: type == "operation.completed" ? "completed" : "failed")
                    return .terminal
                case "operation.failed":
                    guard event["operationId"] as? String == operationId else { continue }
                    removeThinking(operationId: operationId)
                    settleActions(operationId: operationId, status: "failed")
                    message = event["message"] as? String ?? "本次任务失败"
                    return .terminal
                default:
                    break
                }
            } catch {
                message = "消息流中断：\(error.localizedDescription)"
                if Task.isCancelled { return .nonTerminal }
                try? await Task.sleep(nanoseconds: 80_000_000)
                continue
            }
            try? await Task.sleep(nanoseconds: 80_000_000)
        }
        if chatBusy { message = "任务仍在后台运行，稍后会继续同步事件" }
        return .nonTerminal
    }

    private func removeThinking(operationId: String) {
        chatMessages.removeAll { $0.kind == .thinking && $0.operationId == operationId }
    }

    private func settleActions(operationId: String, status: String) {
        for index in chatMessages.indices where chatMessages[index].kind == .action &&
            chatMessages[index].operationId == operationId &&
            chatMessages[index].actionStatus == "running" {
            chatMessages[index].actionStatus = status
        }
    }

    private func upsertAssistantMessage(operationId: String, text: String, append: Bool) {
        guard !text.isEmpty else { return }
        if let index = chatMessages.lastIndex(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
            if append { chatMessages[index].text += text } else { chatMessages[index].text = text }
            return
        }
        chatMessages.append(MobileChatMessage(id: "assistant:\(operationId)", role: .assistant, text: text, operationId: operationId))
    }

    private func upsertAction(operationId: String, stepId: String, title: String, detail: String?, status: String) {
        let id = "action:\(operationId):\(stepId)"
        let entry = MobileChatMessage(id: id, role: .assistant, text: "", kind: .action, operationId: operationId, actionTitle: title, actionDetail: detail, actionStatus: status)
        if let index = chatMessages.firstIndex(where: { $0.id == id }) { chatMessages[index] = entry } else { chatMessages.append(entry) }
    }

    func refreshAccountUsage() async {
        guard loggedIn else {
            accountUsage = nil
            accountUsageError = nil
            accountUsageLoading = false
            return
        }

        accountUsageLoading = true
        defer { accountUsageLoading = false }

        do {
            let result = try await bridge.request(method: "feature.usage.status")
            guard let payload = result.value as? [String: Any],
                  let usage = AccountUsageProjection(payload: payload)
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            accountUsage = usage
            accountUsageError = nil
        } catch {
            // Usage is supplementary account UI. A temporarily unavailable
            // budget endpoint must not turn a valid authenticated session into
            // a login/marketplace failure.
            accountUsage = nil
            accountUsageError = "usage_unavailable"
        }
    }

    private static func canonicalInputSchemaJSON(_ value: Any?) -> String? {
        guard let object = value as? [String: Any],
              JSONSerialization.isValidJSONObject(object),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func marketplacePlugin(from item: [String: Any]) -> MarketplacePlugin? {
        guard let id = item["pluginId"] as? String, !id.isEmpty else { return nil }
        let source = item["source"] as? [String: Any]
        let releaseManifest = item["releaseManifest"] as? [String: Any]
        let install = item["install"] as? [String: Any]
            ?? releaseManifest?["install"] as? [String: Any]
        let commands = source?["commands"] as? [[String: Any]]
            ?? item["commands"] as? [[String: Any]]
            ?? releaseManifest?["commands"] as? [[String: Any]]
            ?? []
        let installSource = install?["source"] as? [String: Any]
        let sourceRef = (installSource?["sourceRef"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let latestVersion = [
            item["latestVersion"] as? String,
            item["version"] as? String,
            releaseManifest?["version"] as? String,
            install?["version"] as? String,
        ]
        .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
        .first { !$0.isEmpty }

        return MarketplacePlugin(
            pluginId: id,
            displayName: item["displayName"] as? String ?? item["title"] as? String ?? id,
            description: item["description"] as? String ?? "无描述",
            latestVersion: latestVersion,
            sourceRef: sourceRef?.isEmpty == false ? sourceRef : nil,
            tools: commands.compactMap(Self.toolContract(from:))
        )
    }

    static func webMcpToolContract(from item: [String: Any]) -> MiniAppToolContract? {
        guard let name = (item["name"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines),
            !name.isEmpty,
            name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil
        else { return nil }
        let annotations = item["annotations"] as? [String: Any]
        let approval: String
        if annotations?["readOnlyHint"] as? Bool == true {
            approval = "none"
        } else if annotations?["destructiveHint"] as? Bool == true {
            approval = "destructive"
        } else {
            approval = "required"
        }
        let description = (item["description"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let title = (item["title"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return MiniAppToolContract(
            name: name,
            description: description?.isEmpty == false ? description! : name,
            approval: approval,
            title: title?.isEmpty == false ? title : nil,
            inputSchemaJSON: canonicalInputSchemaJSON(item["inputSchema"])
        )
    }

    static let globalDharmaStatusFallbackTool = MiniAppToolContract(
        name: "status",
        description: "读取全球法布施 canonical shared runtime 状态",
        approval: "none"
    )

    static func activeLocalInstallSatisfies(
        plugin: MarketplacePlugin,
        pointer: [String: Any]?
    ) -> Bool {
        guard let pointer,
              pointer["pluginId"] as? String == plugin.pluginId
        else { return false }
        guard let targetVersion = plugin.latestVersion, !targetVersion.isEmpty else {
            return true
        }
        return pointer["version"] as? String == targetVersion
    }

    func refresh() async {
        loading = true
        defer { loading = false }
        do {
            let result = try await bridge.request(
                method: "feature.marketplace.browse",
                params: ["query": query.isEmpty ? NSNull() : query, "platform": "ios"]
            )
            let object = result.value as? [String: Any]
            let rows = object?["plugins"] as? [[String: Any]] ?? []
            plugins = rows.compactMap(Self.marketplacePlugin(from:))
            message = "原生 iOS · Rust Host 已连接"
        } catch {
            message = "市场加载失败：\(error.localizedDescription)"
        }
    }

    func install(_ plugin: MarketplacePlugin) async {
        guard let version = plugin.latestVersion, !version.isEmpty else {
            message = "\(plugin.pluginId) 没有可安装版本"
            return
        }
        installingPluginId = plugin.pluginId
        message = "正在安装 \(plugin.pluginId)@\(version)…"
        do {
            let metadata = try await bridge.request(
                method: "feature.marketplace.release",
                params: ["pluginId": plugin.pluginId, "version": version]
            )
            guard let release = (metadata.value as? [String: Any])?["releaseManifest"] as? [String: Any] else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            let install = (metadata.value as? [String: Any])?["install"] as? [String: Any]
                ?? release["install"] as? [String: Any]
            guard install?["protocol"] as? String == "fabushi.marketplace.install.v1",
                  install?["strategy"] as? String == "github-immutable",
                  let source = install?["source"] as? [String: Any],
                  let sourceRef = source["sourceRef"] as? String,
                  !sourceRef.isEmpty,
                  source["marketplaceHostsPackage"] as? Bool != true
            else { throw MahayanaCoordinator.CoordinatorError.invalidResponse }
            let installed = try await bridge.request(
                method: "feature.plugin.install",
                params: ["release": release, "platform": "ios"]
            )
            guard let object = installed.value as? [String: Any] else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            let pluginId = object["pluginId"] as? String ?? plugin.pluginId
            let runtime = object["runtime"] as? String ?? "unknown"
            let permissions = object["requestedPermissions"] as? [String] ?? []
            let accountInstall = try await bridge.request(
                method: "feature.marketplace.add",
                params: ["pluginId": pluginId, "platform": "ios"]
            )
            guard let accountObject = accountInstall.value as? [String: Any],
                  accountObject["accountSynchronized"] as? Bool == true,
                  let bot = accountObject["bot"] as? [String: Any],
                  let botId = bot["id"] as? String,
                  !botId.isEmpty
            else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App 已本地安装，但 Fabushi 账号/Bot 同步未完成")
            }
            installingPluginId = nil
            if permissions.isEmpty {
                await startPortableRuntime(pluginId: pluginId, runtime: runtime)
            } else {
                permissionRequest = PluginPermissionRequest(
                    pluginId: pluginId,
                    runtime: runtime,
                    permissions: permissions
                )
                message = "\(pluginId) 请求 \(permissions.count) 项权限"
            }
        } catch {
            installingPluginId = nil
            message = "安装失败：\(error.localizedDescription)"
        }
    }

    func approvePermissions() async {
        guard let request = permissionRequest else { return }
        permissionRequest = nil
        installingPluginId = request.pluginId
        message = "正在授权 \(request.pluginId)…"
        do {
            for permission in request.permissions {
                _ = try await bridge.request(
                    method: "plugin.permission.grant",
                    params: ["pluginId": request.pluginId, "permission": permission]
                )
            }
            installingPluginId = nil
            await startPortableRuntime(pluginId: request.pluginId, runtime: request.runtime)
        } catch {
            installingPluginId = nil
            message = "授权失败：\(error.localizedDescription)"
        }
    }

    func denyPermissions() {
        guard let request = permissionRequest else { return }
        permissionRequest = nil
        installingPluginId = nil
        message = "\(request.pluginId) 已安装，但权限未授权"
    }

    private func startPortableRuntime(pluginId: String, runtime: String) async {
        guard ["deepseek-js", "javascript", "cordis-js"].contains(runtime) else {
            message = "\(pluginId) 已安装 · \(runtime)"
            return
        }
        installingPluginId = pluginId
        do {
            let compatibility = try await bridge.request(
                method: "plugin.compatibility",
                params: ["pluginId": pluginId]
            )
            guard let object = compatibility.value as? [String: Any], object["portableCompatible"] as? Bool == true else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed("插件不满足移动端 portable runtime 约束")
            }
            _ = try await bridge.request(
                method: "runtime.start",
                params: ["pluginId": pluginId, "config": [String: Any]()]
            )
            message = "\(pluginId) 已安装并启动 · \(runtime)"
        } catch {
            message = "\(pluginId) 已安装但启动失败：\(error.localizedDescription)"
        }
        installingPluginId = nil
    }

    func webMcpPlugin(for plugin: MarketplacePlugin) async -> MarketplacePlugin {
        guard plugin.pluginId == GlobalDharmaMiniAppBridge.globalDharmaId else {
            return plugin
        }
        do {
            let advertised = try await globalDharmaBridge.listOfficialMcpTools(pluginId: plugin.pluginId)
            let tools = advertised.compactMap(Self.webMcpToolContract(from:))
            guard !tools.isEmpty else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed(
                    "Global Dharma canonical MCP tools/list returned no usable tools"
                )
            }
            return plugin.replacingTools(tools)
        } catch {
            message = "Global Dharma WebMCP 工具合同不可用，仅保留只读 status 恢复：\(error.localizedDescription)"
            return plugin.replacingTools([Self.globalDharmaStatusFallbackTool])
        }
    }

    private func reconcileLocalMiniAppInstall(_ plugin: MarketplacePlugin) async throws {
        let active = try await bridge.request(
            method: "feature.plugin.active",
            params: ["pluginId": plugin.pluginId]
        )
        if Self.activeLocalInstallSatisfies(
            plugin: plugin,
            pointer: active.value as? [String: Any]
        ) {
            return
        }

        guard let version = plugin.latestVersion, !version.isEmpty else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "\(plugin.pluginId) has no immutable Marketplace version for local reconciliation"
            )
        }
        let metadata = try await bridge.request(
            method: "feature.marketplace.release",
            params: ["pluginId": plugin.pluginId, "version": version]
        )
        guard let metadataObject = metadata.value as? [String: Any],
              let release = metadataObject["releaseManifest"] as? [String: Any]
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        let install = metadataObject["install"] as? [String: Any]
            ?? release["install"] as? [String: Any]
        guard install?["protocol"] as? String == "fabushi.marketplace.install.v1",
              install?["strategy"] as? String == "github-immutable",
              let source = install?["source"] as? [String: Any],
              let sourceRef = source["sourceRef"] as? String,
              !sourceRef.isEmpty,
              source["marketplaceHostsPackage"] as? Bool != true
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        let installed = try await bridge.request(
            method: "feature.plugin.install",
            params: ["release": release, "platform": "ios"]
        )
        guard let pointer = installed.value as? [String: Any],
              Self.activeLocalInstallSatisfies(plugin: plugin, pointer: pointer)
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
    }

    private func localMiniAppHtml(pluginId: String) async throws -> String {
        let result = try await bridge.request(
            method: "feature.plugin.uiDocument",
            params: ["pluginId": pluginId]
        )
        guard let html = (result.value as? [String: Any])?["html"] as? String,
              !html.isEmpty
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return html
    }

    func loadLocalMiniAppHtml(plugin: MarketplacePlugin) async -> String? {
        do {
            return try await localMiniAppHtml(pluginId: plugin.pluginId)
        } catch {
            do {
                try await reconcileLocalMiniAppInstall(plugin)
                return try await localMiniAppHtml(pluginId: plugin.pluginId)
            } catch {
                message = "本地 Mini App 包不可用，转 Hosted WebMCP：\(error.localizedDescription)"
                return nil
            }
        }
    }

    func callWebMcpTool(pluginId: String, name: String, arguments: [String: Any]) async throws -> Any {
        guard name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid WebMCP tool name")
        }
        if pluginId == GlobalDharmaMiniAppBridge.globalDharmaId {
            let result = try await globalDharmaBridge.callOfficialMcpTool(
                pluginId: pluginId,
                name: name,
                arguments: arguments
            )
            guard name == "status" else { return result }
            let runtime = try globalDharmaSharedRuntime()
            return Self.bridgeGlobalDharmaStatusResult(result, runtime: runtime)
        }
        return try await callRuntimeTool(pluginId: pluginId, name: name, arguments: arguments)
    }

    func callRuntimeTool(pluginId: String, name: String, arguments: [String: Any]) async throws -> Any {
        guard name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid WebMCP tool name")
        }
        let result = try await bridge.request(
            method: "runtime.call",
            params: [
                "pluginId": pluginId,
                "name": name,
                "arguments": arguments,
            ]
        )
        return result.value
    }

    private static func toolContract(from command: [String: Any]) -> MiniAppToolContract? {
        let name = ((command["tool"] as? String) ?? (command["name"] as? String) ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty,
              name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil
        else { return nil }
        let description = ((command["description"] as? String) ?? name)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let title = (command["title"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return MiniAppToolContract(
            name: name,
            description: description.isEmpty ? name : description,
            approval: (command["approval"] as? String) ?? "none",
            title: title?.isEmpty == false ? title : nil,
            inputSchemaJSON: canonicalInputSchemaJSON(command["inputSchema"] ?? command["schema"])
        )
    }
}

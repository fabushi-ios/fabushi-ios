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
    var streaming = false
    var createdAt = Date()
}

struct MiniAppToolContract: Equatable, Sendable {
    let name: String
    let description: String
    let approval: String
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

    private let host: MahayanaHost
    private let onboardingKey = "fabushi.mobile.onboarding-complete.v1"
    @ObservationIgnored private let browserAuthPresentationContext = BrowserAuthPresentationContext()
    @ObservationIgnored private var webAuthenticationSession: ASWebAuthenticationSession?

    init(host: MahayanaHost) {
        self.host = host
        globalDharmaCommerce = GlobalDharmaCommerceModel(host: host)
        onboardingStep = UserDefaults.standard.bool(forKey: onboardingKey) ? 3 : 0
    }

    func initializeApp() async {
        authResolved = false
        do {
            let result = try await host.request(method: "feature.auth.status")
            applyAuth(result.value as? [String: Any])
            authResolved = true
            if loggedIn { await refresh() }
        } catch {
            authResolved = true
            message = "账号状态加载失败：\(error.localizedDescription)"
        }
    }

    private func applyAuth(_ object: [String: Any]?, defaultLoggedIn: Bool = false) {
        loggedIn = object?["loggedIn"] as? Bool ?? defaultLoggedIn
        guard let user = object?["user"] as? [String: Any] else {
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
            let result = try await host.request(method: "feature.auth.browserStart")
            guard let object = result.value as? [String: Any],
                  let attemptId = object["attemptId"] as? String,
                  let loginURLString = (object["loginUrl"] as? String) ?? (object["authorizationUrl"] as? String),
                  let loginURL = URL(string: loginURLString)
            else { throw MahayanaHost.HostError.invalidResponse }
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
            let result = try await host.request(method: "feature.auth.browserReopen", params: ["attemptId": attemptId])
            guard let object = result.value as? [String: Any],
                  let loginURLString = (object["loginUrl"] as? String) ?? (object["authorizationUrl"] as? String),
                  let loginURL = URL(string: loginURLString)
            else { throw MahayanaHost.HostError.invalidResponse }
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
            _ = try await host.request(method: "feature.auth.browserCancel", params: ["attemptId": attemptId])
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
            let infoResult = try await host.request(method: "feature.info")
            guard let info = infoResult.value as? [String: Any],
                  info["platform"] as? String == "ios",
                  let protocolVersion = info["protocolVersion"] as? String,
                  !protocolVersion.isEmpty,
                  (info["runtimeVersion"] as? String)?.contains("test") == true
            else {
                throw MahayanaHost.HostError.invalidResponse
            }

            _ = try await host.request(method: "feature.auth.status")
            let providers = try await host.request(method: "feature.auth.providers")
            guard let providerRows = providers.value as? [[String: Any]],
                  providerRows.contains(where: { $0["id"] as? String == "google" })
            else {
                throw MahayanaHost.HostError.invalidResponse
            }

            let oauth = try await host.request(
                method: "feature.auth.oauthStart",
                params: ["provider": "google"]
            )
            guard let oauthObject = oauth.value as? [String: Any],
                  let attemptId = oauthObject["attemptId"] as? String
            else {
                throw MahayanaHost.HostError.invalidResponse
            }
            let oauthCompleted = try await host.request(
                method: "feature.auth.oauthPoll",
                params: ["attemptId": attemptId]
            )
            guard let completedObject = oauthCompleted.value as? [String: Any],
                  completedObject["status"] as? String == "completed"
            else {
                throw MahayanaHost.HostError.invalidResponse
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
                throw MahayanaHost.HostError.invalidResponse
            }
            _ = try await host.request(
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
                throw MahayanaHost.HostError.invalidResponse
            }
            _ = try await host.request(
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
        let result = try await host.request(
            method: "feature.execute",
            params: ["command": command]
        )
        guard let accepted = result.value as? [String: Any],
              accepted["requestId"] as? String == requestId
        else {
            throw MahayanaHost.HostError.invalidResponse
        }
        return accepted
    }

    private func receiveFeatureEvent(type expectedType: String) async throws -> [String: Any] {
        for _ in 0..<64 {
            let result = try await host.request(method: "feature.receive")
            guard let event = result.value as? [String: Any] else { continue }
            if event["type"] as? String == expectedType { return event }
        }
        throw MahayanaHost.HostError.requestFailed("未收到 FeatureHost 事件 \(expectedType)")
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
            let result = try await host.request(
                method: "feature.auth.browserPoll",
                params: ["attemptId": attemptId]
            )
            guard let object = result.value as? [String: Any],
                  let status = object["status"] as? String
            else {
                throw MahayanaHost.HostError.invalidResponse
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
            _ = try? await host.request(method: "feature.interrupt", params: ["operationId": operationId])
        }
        do {
            let result = try await host.request(method: "feature.auth.logout")
            applyAuth(result.value as? [String: Any])
        } catch {
            message = "退出登录失败：\(error.localizedDescription)"
            return
        }
        loggedIn = false
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

    func stopChat() async {
        guard let operationId = activeOperationId else { return }
        _ = try? await host.request(method: "feature.interrupt", params: ["operationId": operationId])
    }

    private func pumpChatEvents(operationId: String) async -> MahayanaChatPumpOutcome {
        for _ in 0..<1800 {
            if Task.isCancelled { return .nonTerminal }
            do {
                let result = try await host.request(method: "feature.receive")
                guard let event = result.value as? [String: Any], let type = event["type"] as? String else {
                    try? await Task.sleep(nanoseconds: 80_000_000)
                    continue
                }
                switch type {
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
                        upsertAssistantMessage(operationId: operationId, text: eventText, append: false)
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

    func refresh() async {
        loading = true
        defer { loading = false }
        do {
            let result = try await host.request(
                method: "feature.marketplace.browse",
                params: ["query": query.isEmpty ? NSNull() : query, "platform": "ios"]
            )
            let object = result.value as? [String: Any]
            let rows = object?["plugins"] as? [[String: Any]] ?? []
            plugins = rows.compactMap { (item: [String: Any]) -> MarketplacePlugin? in
                guard let id = item["pluginId"] as? String, !id.isEmpty else { return nil }
                let source = item["source"] as? [String: Any]
                let commands = source?["commands"] as? [[String: Any]]
                    ?? item["commands"] as? [[String: Any]]
                    ?? []
                let install = item["install"] as? [String: Any]
                    ?? (item["releaseManifest"] as? [String: Any])?["install"] as? [String: Any]
                let installSource = install?["source"] as? [String: Any]
                let sourceRef = (installSource?["sourceRef"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
                return MarketplacePlugin(
                    pluginId: id,
                    displayName: item["displayName"] as? String ?? id,
                    description: item["description"] as? String ?? "无描述",
                    latestVersion: item["latestVersion"] as? String,
                    sourceRef: sourceRef?.isEmpty == false ? sourceRef : nil,
                    tools: commands.compactMap(Self.toolContract(from:))
                )
            }
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
            let metadata = try await host.request(
                method: "feature.marketplace.release",
                params: ["pluginId": plugin.pluginId, "version": version]
            )
            guard let release = (metadata.value as? [String: Any])?["releaseManifest"] as? [String: Any] else {
                throw MahayanaHost.HostError.invalidResponse
            }
            let install = (metadata.value as? [String: Any])?["install"] as? [String: Any]
                ?? release["install"] as? [String: Any]
            guard install?["protocol"] as? String == "fabushi.marketplace.install.v1",
                  install?["strategy"] as? String == "github-immutable",
                  let source = install?["source"] as? [String: Any],
                  let sourceRef = source["sourceRef"] as? String,
                  !sourceRef.isEmpty,
                  source["marketplaceHostsPackage"] as? Bool != true
            else { throw MahayanaHost.HostError.invalidResponse }
            let installed = try await host.request(
                method: "feature.plugin.install",
                params: ["release": release, "platform": "ios"]
            )
            guard let object = installed.value as? [String: Any] else {
                throw MahayanaHost.HostError.invalidResponse
            }
            let pluginId = object["pluginId"] as? String ?? plugin.pluginId
            let runtime = object["runtime"] as? String ?? "unknown"
            let permissions = object["requestedPermissions"] as? [String] ?? []
            let accountInstall = try await host.request(
                method: "feature.marketplace.add",
                params: ["pluginId": pluginId, "platform": "ios"]
            )
            guard let accountObject = accountInstall.value as? [String: Any],
                  accountObject["accountSynchronized"] as? Bool == true,
                  let bot = accountObject["bot"] as? [String: Any],
                  let botId = bot["id"] as? String,
                  !botId.isEmpty
            else {
                throw MahayanaHost.HostError.requestFailed("Mini App 已本地安装，但 Fabushi 账号/Bot 同步未完成")
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
                _ = try await host.request(
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
            let compatibility = try await host.request(
                method: "plugin.compatibility",
                params: ["pluginId": pluginId]
            )
            guard let object = compatibility.value as? [String: Any], object["portableCompatible"] as? Bool == true else {
                throw MahayanaHost.HostError.requestFailed("插件不满足移动端 portable runtime 约束")
            }
            _ = try await host.request(
                method: "runtime.start",
                params: ["pluginId": pluginId, "config": [String: Any]()]
            )
            message = "\(pluginId) 已安装并启动 · \(runtime)"
        } catch {
            message = "\(pluginId) 已安装但启动失败：\(error.localizedDescription)"
        }
        installingPluginId = nil
    }

    func loadLocalMiniAppHtml(pluginId: String) async -> String? {
        do {
            let result = try await host.request(
                method: "feature.plugin.uiDocument",
                params: ["pluginId": pluginId]
            )
            let html = (result.value as? [String: Any])?["html"] as? String
            return html?.isEmpty == false ? html : nil
        } catch {
            return nil
        }
    }

    func callRuntimeTool(pluginId: String, name: String, arguments: [String: Any]) async throws -> Any {
        guard name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil else {
            throw MahayanaHost.HostError.requestFailed("Invalid WebMCP tool name")
        }
        let result = try await host.request(
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
        return MiniAppToolContract(
            name: name,
            description: description.isEmpty ? name : description,
            approval: (command["approval"] as? String) ?? "none"
        )
    }
}

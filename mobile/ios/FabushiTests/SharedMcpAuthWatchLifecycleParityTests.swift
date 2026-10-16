import XCTest
@testable import Fabushi

private final class AuthWatchRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var _events: [McpConnectorAuthEvent] = []
    private var _completions: [McpAuthCompletion] = []
    private var _reloads = 0

    func event(_ event: McpConnectorAuthEvent) {
        lock.lock()
        _events.append(event)
        lock.unlock()
    }

    func completion(_ completion: McpAuthCompletion) {
        lock.lock()
        _completions.append(completion)
        lock.unlock()
    }

    func reload() {
        lock.lock()
        _reloads += 1
        lock.unlock()
    }

    var events: [McpConnectorAuthEvent] {
        lock.lock()
        defer { lock.unlock() }
        return _events
    }

    var completions: [McpAuthCompletion] {
        lock.lock()
        defer { lock.unlock() }
        return _completions
    }

    var reloads: Int {
        lock.lock()
        defer { lock.unlock() }
        return _reloads
    }
}

private final class AuthRegistrationRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [(String, String)] = []

    func append(_ authorizationUrl: String, _ serverName: String) {
        lock.lock()
        values.append((authorizationUrl, serverName))
        lock.unlock()
    }

    func snapshot() -> [(String, String)] {
        lock.lock()
        defer { lock.unlock() }
        return values
    }
}


private func authWatchHTTPServer(
    disabled: Bool = false,
    accounts: [McpDisplayAccountSlot] = []
) -> DisplayServer {
    .init(
        id: "12",
        name: "GitHub",
        serverIdentifier: "github",
        config: .http(url: "https://mcp.example.test"),
        isTeamServer: false,
        disabledByTeamAdminPolicy: disabled,
        accounts: accounts
    )
}

final class SharedMcpAuthWatchLifecycleParityTests: XCTestCase {
    func testStdioAuthenticationIsRejectedWithoutBackendProbe() async throws {
        let recorder = AuthWatchRecorder()
        let server = DisplayServer(
            id: "12",
            name: "Local",
            serverIdentifier: "local",
            config: .stdio(command: "node", args: ["server.js"]),
            isTeamServer: false
        )
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, _, _, _ in
                XCTFail("stdio auth must not probe the backend")
                return .init(
                    isAvailable: false,
                    requiresAuth: false,
                    hasValidToken: false,
                    authUrl: "",
                    error: ""
                )
            },
            validateTokens: { _ in [] },
            resolveDisplayServer: { _, _ in server },
            reload: { recorder.reload() },
            onConnectorAuth: recorder.event,
            autoPollEnabled: false
        ))

        let result = try await lifecycle.authenticateServer("12")

        XCTAssertEqual(result.status, .notSupported)
        XCTAssertTrue(result.message?.contains("Runner-managed stdio") == true)
        XCTAssertEqual(recorder.events.last?.reason, "stdio_unsupported")
        let pendingCount1 = await lifecycle.pendingWatchCount()
        XCTAssertEqual(pendingCount1, 0)
    }

    func testHttpsAuthenticationStartsWatchAndCompletesAfterTokenLands() async throws {
        let recorder = AuthWatchRecorder()
        let server = authWatchHTTPServer(accounts: [
            .init(accountKey: "work", hasToken: false, serverIdentifier: "github--work"),
        ])
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, redirect, force, account in
                XCTAssertEqual(redirect, MCP_OAUTH_IOS_CALLBACK_URL)
                XCTAssertFalse(force)
                XCTAssertEqual(account, "work")
                return .init(
                    isAvailable: true,
                    requiresAuth: true,
                    hasValidToken: false,
                    authUrl: "https://login.example.test/oauth?state=s&redirect_uri=https%3A%2F%2Fexample.invalid",
                    error: ""
                )
            },
            validateTokens: { targets in
                [.init(
                    serverUrl: targets[0].serverUrl,
                    accountKey: targets[0].accountKey,
                    hasValidToken: true
                )]
            },
            resolveDisplayServer: { _, _ in server },
            reload: { recorder.reload() },
            onConnectorAuth: recorder.event,
            autoPollEnabled: false
        ))
        await lifecycle.setAuthCompletionObserver(recorder.completion)

        // validateAuthorizationUrl intentionally validates the auth endpoint
        // against the MCP server, not the OAuth callback URL.
        let result = try await lifecycle.authenticateServer(
            "12",
            accountKey: "work",
            requestingAgentId: "agent-1"
        )
        XCTAssertEqual(result.status, .started)
        XCTAssertEqual(result.authorizationUrl?.hasPrefix("https://login.example.test/"), true)
        let pendingCount2 = await lifecycle.pendingWatchCount()
        XCTAssertEqual(pendingCount2, 1)

        await lifecycle.pollPendingAuthWatch(serverId: "12", accountKey: "work")

        let pendingCount3 = await lifecycle.pendingWatchCount()
        XCTAssertEqual(pendingCount3, 0)
        XCTAssertEqual(recorder.reloads, 1)
        XCTAssertEqual(recorder.completions.last?.outcome, .completed)
        XCTAssertEqual(recorder.completions.last?.serverIdentifier, "github--work")
        XCTAssertEqual(recorder.completions.last?.requestingAgentId, "agent-1")
        XCTAssertEqual(recorder.events.last?.phase, "token_stored")
        XCTAssertEqual(recorder.events.last?.outcome, "ok")
    }

    func testForceReauthSuppressesFirstPoll() async throws {
        let recorder = AuthWatchRecorder()
        let server = authWatchHTTPServer()
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, _, force, _ in
                XCTAssertTrue(force)
                return .init(
                    isAvailable: true,
                    requiresAuth: true,
                    hasValidToken: true,
                    authUrl: "https://login.example.test/oauth",
                    error: ""
                )
            },
            validateTokens: { targets in
                [.init(
                    serverUrl: targets[0].serverUrl,
                    accountKey: targets[0].accountKey,
                    hasValidToken: true
                )]
            },
            resolveDisplayServer: { _, _ in server },
            reload: { recorder.reload() },
            autoPollEnabled: false
        ))
        await lifecycle.setAuthCompletionObserver(recorder.completion)

        _ = try await lifecycle.authenticateServer("12", forceReauth: true)
        await lifecycle.pollPendingAuthWatch(
            serverId: "12",
            accountKey: DEFAULT_MCP_ACCOUNT_KEY
        )
        let pendingCount4 = await lifecycle.pendingWatchCount()
        XCTAssertEqual(pendingCount4, 1)
        XCTAssertTrue(recorder.completions.isEmpty)

        await lifecycle.pollPendingAuthWatch(
            serverId: "12",
            accountKey: DEFAULT_MCP_ACCOUNT_KEY
        )
        let pendingCount5 = await lifecycle.pendingWatchCount()
        XCTAssertEqual(pendingCount5, 0)
        XCTAssertEqual(recorder.completions.last?.outcome, .completed)
    }

    func testFreshAdminPolicyConfirmationCancelsPendingAuthentication() async throws {
        let recorder = AuthWatchRecorder()
        let stale = authWatchHTTPServer(disabled: true)
        let fresh = authWatchHTTPServer(disabled: true)
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, _, _, _ in
                XCTFail("admin-blocked connector must not probe OAuth")
                return .init(
                    isAvailable: false,
                    requiresAuth: false,
                    hasValidToken: false,
                    authUrl: "",
                    error: ""
                )
            },
            validateTokens: { _ in [] },
            resolveDisplayServer: { _, freshRead in freshRead ? fresh : stale },
            reload: { recorder.reload() },
            onConnectorAuth: recorder.event,
            autoPollEnabled: false
        ))

        let result = try await lifecycle.authenticateServer("12")

        XCTAssertEqual(result.status, .notSupported)
        XCTAssertEqual(recorder.reloads, 1)
        XCTAssertEqual(recorder.events.last?.reason, "admin_blocked")
    }

    func testInvalidAuthorizationURLIsRefused() async throws {
        let recorder = AuthWatchRecorder()
        let server = authWatchHTTPServer()
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, _, _, _ in
                .init(
                    isAvailable: true,
                    requiresAuth: true,
                    hasValidToken: false,
                    authUrl: "http://evil.example.test/oauth",
                    error: ""
                )
            },
            validateTokens: { _ in [] },
            resolveDisplayServer: { _, _ in server },
            reload: { recorder.reload() },
            onConnectorAuth: recorder.event,
            autoPollEnabled: false
        ))

        let result = try await lifecycle.authenticateServer("12")

        XCTAssertEqual(result.status, .notSupported)
        XCTAssertEqual(result.authorizationUrl, nil)
        XCTAssertEqual(recorder.events.last?.reason, "invalid_auth_url")
    }

    func testExpiredWatchIsRemovedWithoutTokenValidation() async throws {
        let recorder = AuthWatchRecorder()
        let server = authWatchHTTPServer()
        let now = Int64(100)
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, _, _, _ in
                .init(
                    isAvailable: true,
                    requiresAuth: true,
                    hasValidToken: false,
                    authUrl: "https://login.example.test/oauth",
                    error: ""
                )
            },
            validateTokens: { _ in
                XCTFail("expired watch must not validate tokens")
                return []
            },
            resolveDisplayServer: { _, _ in server },
            reload: { recorder.reload() },
            onConnectorAuth: recorder.event,
            nowMs: { now + 10 },
            authWatchTimeoutMs: 0,
            autoPollEnabled: false
        ))

        _ = try await lifecycle.authenticateServer("12")
        await lifecycle.pollPendingAuthWatch(
            serverId: "12",
            accountKey: DEFAULT_MCP_ACCOUNT_KEY
        )

        let pendingCount6 = await lifecycle.pendingWatchCount()
        XCTAssertEqual(pendingCount6, 0)
        XCTAssertEqual(recorder.events.last?.outcome, "timeout")
    }

    func testBackgroundSuspendsAutoPollingAndActiveResumeChecksImmediately() async throws {
        let recorder = AuthWatchRecorder()
        let server = authWatchHTTPServer()
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, _, _, _ in
                .init(
                    isAvailable: true,
                    requiresAuth: true,
                    hasValidToken: false,
                    authUrl: "https://login.example.test/oauth",
                    error: ""
                )
            },
            validateTokens: { targets in
                [.init(
                    serverUrl: targets[0].serverUrl,
                    accountKey: targets[0].accountKey,
                    hasValidToken: true
                )]
            },
            resolveDisplayServer: { _, _ in server },
            reload: { recorder.reload() },
            onConnectorAuth: recorder.event,
            authWatchPollIntervalMs: 60_000,
            autoPollEnabled: true
        ))
        await lifecycle.setAuthCompletionObserver(recorder.completion)

        _ = try await lifecycle.authenticateServer("12")
        let foregroundTasks = await lifecycle.activeAutoPollTaskCount()
        XCTAssertEqual(foregroundTasks, 1)

        await lifecycle.sceneEnteredBackground()
        let backgroundTasks = await lifecycle.activeAutoPollTaskCount()
        let backgroundPending = await lifecycle.pendingWatchCount()
        XCTAssertEqual(backgroundTasks, 0)
        XCTAssertEqual(backgroundPending, 1)
        XCTAssertTrue(recorder.completions.isEmpty)

        await lifecycle.sceneBecameActive()

        let resumedPending = await lifecycle.pendingWatchCount()
        let resumedTasks = await lifecycle.activeAutoPollTaskCount()
        XCTAssertEqual(resumedPending, 0)
        XCTAssertEqual(resumedTasks, 0)
        XCTAssertEqual(recorder.completions.last?.outcome, .completed)
        XCTAssertEqual(recorder.reloads, 1)
    }


    func testAuthenticationRegistersOAuthCallbackStateBeforeStartingWatch() async throws {
        let recorder = AuthWatchRecorder()
        let server = authWatchHTTPServer()
        let registrations = AuthRegistrationRecorder()
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, redirect, _, _ in
                XCTAssertEqual(redirect, MCP_OAUTH_IOS_CALLBACK_URL)
                var components = try XCTUnwrap(
                    URLComponents(string: "https://login.example.test/oauth")
                )
                components.queryItems = [
                    .init(name: "redirect_uri", value: MCP_OAUTH_IOS_CALLBACK_URL),
                    .init(name: "state", value: "mcp-state-1"),
                ]
                return .init(
                    isAvailable: true,
                    requiresAuth: true,
                    hasValidToken: false,
                    authUrl: try XCTUnwrap(components.url?.absoluteString),
                    error: ""
                )
            },
            validateTokens: { _ in [] },
            resolveDisplayServer: { _, _ in server },
            reload: { recorder.reload() },
            registerOAuthCallback: { authorizationUrl, serverName in
                registrations.append(authorizationUrl, serverName)
                return parseMcpOAuthLoopbackAuthorization(authorizationUrl) != nil
            },
            onConnectorAuth: recorder.event,
            autoPollEnabled: false
        ))

        let result = try await lifecycle.authenticateServer("12")

        XCTAssertEqual(result.status, .started)
        let recordedRegistrations = registrations.snapshot()
        XCTAssertEqual(recordedRegistrations.count, 1)
        XCTAssertEqual(recordedRegistrations.first?.1, "GitHub")
        XCTAssertEqual(
            recordedRegistrations.first.flatMap { parseMcpOAuthLoopbackAuthorization($0.0)?.state },
            "mcp-state-1"
        )
        let pendingWatchCount = await lifecycle.pendingWatchCount()
        XCTAssertEqual(pendingWatchCount, 1)
    }

    func testAuthenticationFailsClosedWhenOAuthCallbackStateCannotRegister() async throws {
        let recorder = AuthWatchRecorder()
        let server = authWatchHTTPServer()
        let lifecycle = SandMcpAuthWatchLifecycle(deps: .init(
            checkAuthStatus: { _, _, _, _ in
                .init(
                    isAvailable: true,
                    requiresAuth: true,
                    hasValidToken: false,
                    authUrl: "https://login.example.test/oauth?state=bad-state",
                    error: ""
                )
            },
            validateTokens: { _ in [] },
            resolveDisplayServer: { _, _ in server },
            reload: { recorder.reload() },
            registerOAuthCallback: { _, _ in false },
            onConnectorAuth: recorder.event,
            autoPollEnabled: false
        ))

        let result = try await lifecycle.authenticateServer("12")

        XCTAssertEqual(result.status, .notSupported)
        XCTAssertTrue(result.message?.contains("registered iOS OAuth callback") == true)
        let pendingWatchCount = await lifecycle.pendingWatchCount()
        XCTAssertEqual(pendingWatchCount, 0)
        XCTAssertEqual(recorder.events.last?.reason, "invalid_auth_url")
    }

}

import CryptoKit
import XCTest
@testable import Fabushi

@MainActor
final class GlobalDharmaMiniAppParityTests: XCTestCase {
    func testLocalMiniAppSigningAuthorityKeepsPrivateKeyBehindKeychainAdapter() async throws {
        let scope = String(repeating: "a", count: 64)
        let authority = LocalMiniAppSigningAuthority(
            service: "com.ombhrum.fabushi.tests.local-author.\(UUID().uuidString)"
        )
        let descriptor = try await authority.descriptor(accountScope: scope)
        let payload = Data("fabushi.fabapp.manifest.v1\u{0}test".utf8)
        let signature = try await authority.sign(payload, accountScope: scope)

        var encoded = descriptor.publicKeyBase64URL
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        while encoded.count % 4 != 0 { encoded.append("=") }
        let publicBytes = try XCTUnwrap(Data(base64Encoded: encoded))
        let publicKey = try Curve25519.Signing.PublicKey(rawRepresentation: publicBytes)

        XCTAssertTrue(publicKey.isValidSignature(signature, for: payload))
        XCTAssertTrue(descriptor.keyId.hasPrefix("local-ed25519-"))
        XCTAssertFalse(descriptor.publicKeyBase64URL.isEmpty)

        try await authority.deleteKeyForTesting(accountScope: scope)
    }

    func testGovernedLifetimeEntitlementConstantsMatchCrossPlatformContract() {
        XCTAssertEqual(GlobalDharmaMiniAppBridge.globalDharmaId, "global-dharma")
        XCTAssertEqual(GlobalDharmaMiniAppBridge.prayerWheelCapability, "local.prayer-wheel.start")
        XCTAssertEqual(GlobalDharmaMiniAppBridge.prayerWheelLifetimeSku, "local-prayer-wheel.lifetime")
        XCTAssertEqual(GlobalDharmaMiniAppBridge.prayerWheelLifetimeProductId, "prod.global-dharma.local-prayer-wheel.lifetime")
        XCTAssertEqual(GlobalDharmaMiniAppBridge.prayerWheelLifetimeCNYMinor, 108_000)
    }

    func testProjectedMiniAppBotCarriesComposerLaunchMetadata() {
        let bot = MobileBotSummary(
            id: "global-dharma-bot",
            name: "全球法布施",
            description: "用自然语言或 Web UI 使用小程序",
            miniAppId: GlobalDharmaMiniAppBridge.globalDharmaId,
            menuButtonText: "打开应用"
        )
        XCTAssertEqual(bot.miniAppId, "global-dharma")
        XCTAssertEqual(bot.menuButtonText, "打开应用")
    }

    func testMiniAppWebProfilesAreStablePerAccountAndDisjointAcrossAccountsAndPlugins() throws {
        let a1 = try XCTUnwrap(
            MarketplaceModel.miniAppWebsiteDataStoreIdentifier(
                accountScope: "account-a",
                pluginId: "global-dharma"
            )
        )
        let a2 = try XCTUnwrap(
            MarketplaceModel.miniAppWebsiteDataStoreIdentifier(
                accountScope: "account-a",
                pluginId: "global-dharma"
            )
        )
        let otherAccount = try XCTUnwrap(
            MarketplaceModel.miniAppWebsiteDataStoreIdentifier(
                accountScope: "account-b",
                pluginId: "global-dharma"
            )
        )
        let otherPlugin = try XCTUnwrap(
            MarketplaceModel.miniAppWebsiteDataStoreIdentifier(
                accountScope: "account-a",
                pluginId: "faliu-flashcards"
            )
        )
        XCTAssertEqual(a1, a2)
        XCTAssertNotEqual(a1, otherAccount)
        XCTAssertNotEqual(a1, otherPlugin)
        XCTAssertNil(
            MarketplaceModel.miniAppWebsiteDataStoreIdentifier(
                accountScope: "",
                pluginId: "global-dharma"
            )
        )
    }

    func testGeneratedMiniAppDocumentFailsClosedForNetworkAndEmbedding() {
        let original = "<html><head><title>Draft</title></head><body><script>fetch('https://evil.invalid')</script></body></html>"
        let hardened = hardenGeneratedMiniAppDocument(original)
        XCTAssertTrue(hardened.contains("default-src 'none'"))
        XCTAssertTrue(hardened.contains("connect-src 'none'"))
        XCTAssertTrue(hardened.contains("frame-src 'none'"))
        XCTAssertTrue(hardened.contains("object-src 'none'"))
        XCTAssertTrue(hardened.contains("form-action 'none'"))
        XCTAssertTrue(hardened.contains("base-uri 'none'"))
        XCTAssertTrue(hardened.contains("fetch('https://evil.invalid')"))
        XCTAssertEqual(
            hardened.components(separatedBy: "Content-Security-Policy").count - 1,
            1
        )
    }

    func testGeneratedMiniAppPreviewDigestChangesForEveryByteMutation() {
        let first = generatedMiniAppPreviewDigest("<html><body>draft one</body></html>")
        let same = generatedMiniAppPreviewDigest("<html><body>draft one</body></html>")
        let changed = generatedMiniAppPreviewDigest("<html><body>draft two</body></html>")
        XCTAssertEqual(first, same)
        XCTAssertNotEqual(first, changed)
        XCTAssertEqual(first.count, 64)
        XCTAssertTrue(first.allSatisfy { $0.isHexDigit })
    }

    func testAgentGeneratedMiniAppSourceRejectsExternalURLTraversalAndNonHTML() {
        XCTAssertEqual(
            MarketplaceModel.canonicalGeneratedMiniAppArtifactPath(
                "/tmp/account/agent/attachments/generated.html"
            ),
            "/tmp/account/agent/attachments/generated.html"
        )
        XCTAssertNil(
            MarketplaceModel.canonicalGeneratedMiniAppArtifactPath(
                "/tmp/account/agent/attachments/../escape.html"
            )
        )
        XCTAssertNil(
            MarketplaceModel.canonicalGeneratedMiniAppArtifactPath(
                "https://evil.invalid/generated.html"
            )
        )
        XCTAssertNil(
            mobileGeneratedMiniAppArtifactPath("https://evil.invalid/generated.html")
        )
        XCTAssertEqual(
            mobileGeneratedMiniAppArtifactPath(
                "file:///tmp/account/agent/attachments/generated.html"
            ),
            "/tmp/account/agent/attachments/generated.html"
        )
        XCTAssertNil(
            mobileGeneratedMiniAppArtifactPath(
                "file:///tmp/account/agent/attachments/generated.js"
            )
        )
    }

    func testControlledAgentDraftPreviewMustMatchExactReviewedBytes() {
        let html = "<html><head></head><body>offline</body></html>"
        let sha = generatedMiniAppPreviewDigest(html)
        let transaction = LocalMiniAppDraftTransaction(
            accountScope: String(repeating: "a", count: 64),
            transactionId: String(repeating: "b", count: 32),
            reservedAppId: "local.fabushi.test",
            publisherId: "local.fabushi"
        )
        let review = LocalMiniAppDraftReview(
            transaction: transaction,
            draftDigest: String(repeating: "c", count: 64),
            totalBytes: UInt64(Data(html.utf8).count),
            requestedCapabilities: [],
            files: [
                LocalMiniAppDraftReviewFile(
                    path: "index.html",
                    sha256: sha,
                    bytes: UInt64(Data(html.utf8).count),
                    mime: "text/html"
                )
            ]
        )
        XCTAssertTrue(generatedMiniAppReviewMatchesPreview(review, html: html))
        XCTAssertFalse(
            generatedMiniAppReviewMatchesPreview(
                review,
                html: html.replacingOccurrences(of: "offline", with: "changed")
            )
        )
    }

    func testGeneratedPreviewOriginNeverReceivesInstalledWebMcpBridgeTrust() {
        XCTAssertFalse(
            isTrustedWebMcpBridgeURL(
                URL(string: "https://preview.miniapp.local.fabushi.invalid/drafts/deadbeef/")
            )
        )
    }

    func testWebMcpBridgeTrustIsBoundedToCanonicalLocalAndHostedOrigins() {
        XCTAssertTrue(isTrustedWebMcpBridgeHost("miniapp.local.fabushi.invalid"))
        XCTAssertTrue(isTrustedWebMcpBridgeHost("fabushi.ombhrum.com"))
        XCTAssertFalse(isTrustedWebMcpBridgeHost("ombhrum.com"))
        XCTAssertFalse(isTrustedWebMcpBridgeHost("fabushi.ombhrum.com.evil.invalid"))
        XCTAssertFalse(isTrustedWebMcpBridgeHost(nil))

        XCTAssertTrue(isTrustedWebMcpBridgeURL(URL(string: "https://fabushi.ombhrum.com/miniapps/test/")))
        XCTAssertFalse(isTrustedWebMcpBridgeURL(URL(string: "http://fabushi.ombhrum.com/miniapps/test/")))

        let root = URL(fileURLWithPath: "/tmp/fabushi/plugins/test/versions/1.0.0/web", isDirectory: true)
        let entry = root.appendingPathComponent("index.html")
        let sibling = URL(fileURLWithPath: "/tmp/fabushi/plugins/other/index.html")
        XCTAssertTrue(isTrustedWebMcpBridgeURL(entry, localRootURL: root))
        XCTAssertFalse(isTrustedWebMcpBridgeURL(root, localRootURL: root))
        XCTAssertFalse(isTrustedWebMcpBridgeURL(sibling, localRootURL: root))
    }

    func testWebMcpToolContractPreservesCanonicalInputSchemaForNativeAndInjectedUX() throws {
        let tool = try XCTUnwrap(MarketplaceModel.webMcpToolContract(from: [
            "name": "configure",
            "title": "配置",
            "description": "配置测试 Tool",
            "inputSchema": [
                "type": "object",
                "required": ["count"],
                "properties": [
                    "count": ["type": "integer"],
                    "enabled": ["type": "boolean"],
                ],
            ],
            "annotations": ["readOnlyHint": true],
        ]))
        XCTAssertEqual(tool.title, "配置")
        XCTAssertEqual(tool.inputSchemaObject?["type"] as? String, "object")
        let properties = try XCTUnwrap(tool.inputSchemaObject?["properties"] as? [String: Any])
        XCTAssertEqual((properties["count"] as? [String: Any])?["type"] as? String, "integer")

        let plugin = MarketplacePlugin(
            pluginId: "schema-test",
            displayName: "Schema Test",
            description: "test",
            latestVersion: nil,
            tools: [tool]
        )
        let definition = try XCTUnwrap(webMcpToolDefinitions(plugin: plugin).first)
        let injectedSchema = try XCTUnwrap(definition["inputSchema"] as? [String: Any])
        XCTAssertEqual(injectedSchema["type"] as? String, "object")
        XCTAssertNotNil((injectedSchema["properties"] as? [String: Any])?["enabled"])
    }

    func testToolCommandParserMatchesDesktopSlashAndDefaultSemantics() throws {
        let status = try XCTUnwrap(MarketplaceModel.webMcpToolContract(from: [
            "name": "status",
            "inputSchema": ["type": "object", "properties": [String: Any]()],
            "annotations": ["readOnlyHint": true],
        ]))
        let echo = try XCTUnwrap(MarketplaceModel.webMcpToolContract(from: [
            "name": "echo",
            "inputSchema": [
                "type": "object",
                "properties": ["message": ["type": "string"]],
            ],
            "annotations": ["readOnlyHint": true],
        ]))
        let configure = try XCTUnwrap(MarketplaceModel.webMcpToolContract(from: [
            "name": "configure",
            "inputSchema": [
                "type": "object",
                "required": ["count", "mode"],
                "properties": [
                    "region": ["type": "string", "default": "cn"],
                    "enabled": ["type": "boolean"],
                    "tags": ["type": "array", "items": ["type": "string"]],
                    "count": ["type": "integer"],
                    "mode": ["type": "string", "enum": ["safe", "fast"]],
                ],
            ],
            "annotations": ["readOnlyHint": true],
        ]))
        let tools = [status, echo, configure]

        XCTAssertEqual(try parseMiniAppToolCommand("hello", tools: tools), .text("hello"))

        switch try parseMiniAppToolCommand("/status", tools: tools) {
        case .call(let tool, let arguments):
            XCTAssertEqual(tool.name, "status")
            XCTAssertTrue(arguments.isEmpty)
        default:
            XCTFail("zero-field tool should call immediately")
        }

        switch try parseMiniAppToolCommand("/echo hello world", tools: tools) {
        case .call(let tool, let arguments):
            XCTAssertEqual(tool.name, "echo")
            XCTAssertEqual(arguments["message"], .string("hello world"))
        default:
            XCTFail("single-string tool should receive the remainder directly")
        }

        switch try parseMiniAppToolCommand("/configure", tools: tools) {
        case .form(let tool, let initial):
            XCTAssertEqual(tool.name, "configure")
            XCTAssertEqual(initial["region"], .string("cn"))
            XCTAssertEqual(initial["enabled"], .boolean(false))
            XCTAssertEqual(initial["tags"], .array([]))
            XCTAssertNil(initial["count"])
            XCTAssertNil(initial["mode"])
        default:
            XCTFail("structured tool should open a schema form")
        }

        XCTAssertThrowsError(try parseMiniAppToolCommand("/missing", tools: tools)) { error in
            XCTAssertEqual(error.localizedDescription, "当前插件没有 /missing Tool")
        }
    }

    func testToolCommandSchemaValidationFailsBeforeExecutionAndAcceptsValidForm() throws {
        let tool = try XCTUnwrap(MarketplaceModel.webMcpToolContract(from: [
            "name": "configure",
            "inputSchema": [
                "type": "object",
                "required": ["count", "mode"],
                "properties": [
                    "count": ["type": "integer"],
                    "mode": ["type": "string", "enum": ["safe", "fast"]],
                    "options": [
                        "type": "object",
                        "properties": [
                            "dryRun": ["type": "boolean"],
                        ],
                    ],
                ],
            ],
            "annotations": ["readOnlyHint": true],
        ]))
        let schema = try XCTUnwrap(tool.inputSchemaObject)

        let missing = validateMiniAppSchemaValue(schema, value: .object([:]))
        XCTAssertTrue(missing.contains("$.count 是必填项"))
        XCTAssertTrue(missing.contains("$.mode 是必填项"))

        let wrongTypes = validateMiniAppSchemaValue(schema, value: .object([
            "count": .string("two"),
            "mode": .string("unsafe"),
            "options": .array([]),
        ]))
        XCTAssertTrue(wrongTypes.contains("$.count 必须是整数"))
        XCTAssertTrue(wrongTypes.contains("$.mode 必须是枚举中的一个值"))
        XCTAssertTrue(wrongTypes.contains("$.options 必须是对象"))

        let valid = validateMiniAppSchemaValue(schema, value: .object([
            "count": .integer(2),
            "mode": .string("safe"),
            "options": .object(["dryRun": .boolean(false)]),
        ]))
        XCTAssertEqual(valid, [])
    }

    func testGlobalDharmaStatusFallbackIsReadOnlyAndNarrow() {
        let tool = MarketplaceModel.globalDharmaStatusFallbackTool
        XCTAssertEqual(tool.name, "status")
        XCTAssertEqual(tool.approval, "none")
    }

    func testDelegatedMiniAppCredentialMustBeBoundedBearerAndNeverRedacted() throws {
        let token = String(repeating: "a", count: 64)
        XCTAssertEqual(
            try GlobalDharmaMiniAppBridge.delegatedPluginCredential(from: [
                "accessToken": token,
                "tokenType": "Bearer",
                "expiresIn": 300,
            ]),
            token
        )

        XCTAssertThrowsError(
            try GlobalDharmaMiniAppBridge.delegatedPluginCredential(from: [
                "accessToken": "[stored by Mahayana]",
                "tokenType": "Bearer",
                "expiresIn": 300,
            ])
        )
        XCTAssertThrowsError(
            try GlobalDharmaMiniAppBridge.delegatedPluginCredential(from: [
                "accessToken": token,
                "tokenType": "Bearer",
                "expiresIn": 301,
            ])
        )
    }

    func testGlobalDharmaExecutionProjectionPreservesOneHostBoundaryRevision() throws {
        let first = MarketplaceModel.nextGlobalDharmaExecution(
            previous: nil,
            tool: "status",
            result: ["structuredContent": ["running": false]],
            source: "bot"
        )
        XCTAssertEqual(first["protocol"] as? String, "fabushi.miniapp.execution.v1")
        XCTAssertEqual(first["miniAppId"] as? String, "global-dharma")
        XCTAssertEqual((first["revision"] as? NSNumber)?.intValue, 1)
        XCTAssertEqual(first["source"] as? String, "bot")
        XCTAssertEqual(first["phase"] as? String, "completed")
        XCTAssertEqual(first["tool"] as? String, "status")

        let second = MarketplaceModel.nextGlobalDharmaExecution(
            previous: first,
            tool: "start",
            result: ["structuredContent": ["running": true]],
            source: "web-ui"
        )
        XCTAssertEqual((second["revision"] as? NSNumber)?.intValue, 2)

        let runtime = try XCTUnwrap(MarketplaceModel.globalDharmaRuntime(from: first))
        XCTAssertEqual(runtime["protocol"] as? String, "fabushi.miniapp.runtime.v1")
        XCTAssertEqual(runtime["miniAppId"] as? String, "global-dharma")
        XCTAssertEqual((runtime["revision"] as? NSNumber)?.intValue, 1)
        let state = try XCTUnwrap(runtime["state"] as? [String: Any])
        XCTAssertEqual(state["tool"] as? String, "status")
        XCTAssertEqual(state["source"] as? String, "bot")
    }

    func testGlobalDharmaStatusBridgePreservesRemoteStatusAndRestoresCanonicalRuntime() throws {
        let execution = MarketplaceModel.nextGlobalDharmaExecution(
            previous: nil,
            tool: "status",
            result: ["structuredContent": ["running": true]],
            source: "bot"
        )
        let runtime = try XCTUnwrap(MarketplaceModel.globalDharmaRuntime(from: execution))
        let bridged = MarketplaceModel.bridgeGlobalDharmaStatusResult(
            [
                "content": [["type": "text", "text": "running"]],
                "structuredContent": ["running": true, "mode": "home"],
            ],
            runtime: runtime
        )

        let structured = try XCTUnwrap(bridged["structuredContent"] as? [String: Any])
        XCTAssertEqual(structured["running"] as? Bool, true)
        XCTAssertEqual(structured["mode"] as? String, "home")
        let restored = try XCTUnwrap(structured["runtime"] as? [String: Any])
        XCTAssertEqual(restored["protocol"] as? String, "fabushi.miniapp.runtime.v1")
        XCTAssertEqual(restored["miniAppId"] as? String, "global-dharma")
        XCTAssertEqual((restored["revision"] as? NSNumber)?.intValue, 1)
    }


    func testGlobalDharmaAccountScopeUsesCanonicalStableIdentityAndFailsClosed() {
        XCTAssertEqual(
            MarketplaceModel.globalDharmaScope(for: [
                "loggedIn": true,
                "user": ["principalId": "account-alpha", "email": "ignored@example.invalid"],
            ]),
            "YWNjb3VudC1hbHBoYQ"
        )
        XCTAssertEqual(
            MarketplaceModel.globalDharmaScope(for: [
                "loggedIn": true,
                "user": ["principal_id": "account-alpha"],
            ]),
            "YWNjb3VudC1hbHBoYQ"
        )
        XCTAssertEqual(
            MarketplaceModel.globalDharmaScope(for: [
                "loggedIn": true,
                "user": ["userId": NSNumber(value: 42)],
            ]),
            "NDI"
        )
        XCTAssertEqual(
            MarketplaceModel.globalDharmaScope(for: [
                "loggedIn": true,
                "userNo": NSNumber(value: 73),
            ]),
            "NzM"
        )
        XCTAssertNil(MarketplaceModel.globalDharmaScope(for: [
            "loggedIn": true,
            "user": ["email": "legacy-email-is-not-a-stable-runtime-identity@example.invalid"],
        ]))
        XCTAssertNil(MarketplaceModel.globalDharmaScope(for: [
            "loggedIn": true,
            "user": ["userId": true],
        ]))
    }

    func testMiniAppBridgeSessionRequiresExactInstanceNonceAndExplicitGrant() {
        let session = MiniAppWebMcpBridgeSession(
            pluginInstanceId: "global-dharma:instance-1",
            nonce: "0123456789abcdef0123456789abcdef",
            grants: ["status", "start"]
        )

        XCTAssertTrue(session.allows(
            pluginInstanceId: "global-dharma:instance-1",
            nonce: "0123456789abcdef0123456789abcdef",
            toolName: "status"
        ))
        XCTAssertFalse(session.allows(
            pluginInstanceId: "global-dharma:instance-2",
            nonce: "0123456789abcdef0123456789abcdef",
            toolName: "status"
        ))
        XCTAssertFalse(session.allows(
            pluginInstanceId: "global-dharma:instance-1",
            nonce: "wrong-nonce",
            toolName: "status"
        ))
        XCTAssertFalse(session.allows(
            pluginInstanceId: "global-dharma:instance-1",
            nonce: "0123456789abcdef0123456789abcdef",
            toolName: "deploy_latest"
        ))
    }

    func testMarketplaceProjectionPreservesInstallableVersionAcrossCatalogShapes() throws {
        let legacy = try XCTUnwrap(MarketplaceModel.marketplacePlugin(from: [
            "pluginId": "global-dharma",
            "displayName": "全球法布施",
            "description": "test",
            "version": "1.0.0",
            "install": [
                "version": "1.0.0",
                "source": ["sourceRef": String(repeating: "a", count: 40)],
            ],
        ]))
        XCTAssertEqual(legacy.latestVersion, "1.0.0")
        XCTAssertEqual(legacy.sourceRef, String(repeating: "a", count: 40))

        let releaseProjection = try XCTUnwrap(MarketplaceModel.marketplacePlugin(from: [
            "pluginId": "global-dharma",
            "displayName": "全球法布施",
            "releaseManifest": [
                "version": "1.0.1",
                "install": [
                    "version": "1.0.1",
                    "source": ["sourceRef": String(repeating: "b", count: 40)],
                ],
            ],
        ]))
        XCTAssertEqual(releaseProjection.latestVersion, "1.0.1")
        XCTAssertEqual(releaseProjection.sourceRef, String(repeating: "b", count: 40))
    }

    func testCanonicalMcpToolProjectionDerivesFailClosedApprovalFromAnnotations() throws {
        let status = try XCTUnwrap(MarketplaceModel.webMcpToolContract(from: [
            "name": "status",
            "description": "读取状态",
            "annotations": ["readOnlyHint": true],
        ]))
        XCTAssertEqual(status.approval, "none")

        let stop = try XCTUnwrap(MarketplaceModel.webMcpToolContract(from: [
            "name": "stop",
            "description": "停止服务",
            "annotations": ["destructiveHint": true],
        ]))
        XCTAssertEqual(stop.approval, "destructive")

        let start = try XCTUnwrap(MarketplaceModel.webMcpToolContract(from: [
            "name": "start",
            "description": "启动服务",
        ]))
        XCTAssertEqual(start.approval, "required")
        XCTAssertNil(MarketplaceModel.webMcpToolContract(from: ["name": "bad tool"]))
    }

    func testDeviceLocalInstallReconciliationRequiresMatchingPluginAndVersion() {
        let plugin = MarketplacePlugin(
            pluginId: "global-dharma",
            displayName: "全球法布施",
            description: "test",
            latestVersion: "1.0.0",
            tools: []
        )
        XCTAssertFalse(MarketplaceModel.activeLocalInstallSatisfies(plugin: plugin, pointer: nil))
        XCTAssertFalse(MarketplaceModel.activeLocalInstallSatisfies(
            plugin: plugin,
            pointer: ["pluginId": "global-dharma", "version": "0.9.9"]
        ))
        XCTAssertFalse(MarketplaceModel.activeLocalInstallSatisfies(
            plugin: plugin,
            pointer: ["pluginId": "other", "version": "1.0.0"]
        ))
        XCTAssertTrue(MarketplaceModel.activeLocalInstallSatisfies(
            plugin: plugin,
            pointer: ["pluginId": "global-dharma", "version": "1.0.0"]
        ))
    }

    func testMiniAppBridgeFreshSessionUsesPerLoadIdentityAndBoundedExplicitGrants() {
        let plugin = MarketplacePlugin(
            pluginId: "bridge-session-test",
            displayName: "Bridge Session Test",
            description: "test",
            latestVersion: nil,
            tools: [
                .init(name: "status", description: "status", approval: "none"),
                .init(name: "start", description: "start", approval: "confirm"),
            ]
        )
        let first = MiniAppWebMcpBridgeSession.fresh(plugin: plugin)
        let second = MiniAppWebMcpBridgeSession.fresh(plugin: plugin)

        XCTAssertNotEqual(first.pluginInstanceId, second.pluginInstanceId)
        XCTAssertNotEqual(first.nonce, second.nonce)
        XCTAssertGreaterThanOrEqual(first.nonce.count, 16)
        XCTAssertEqual(first.grants, Set(["status", "start"]))
    }
}

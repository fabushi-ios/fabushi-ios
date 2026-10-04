import XCTest
@testable import Fabushi

@MainActor
final class GlobalDharmaMiniAppParityTests: XCTestCase {
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

    func testWebMcpBridgeTrustIsBoundedToCanonicalLocalAndHostedOrigins() {
        XCTAssertTrue(isTrustedWebMcpBridgeHost("miniapp.local.fabushi.invalid"))
        XCTAssertTrue(isTrustedWebMcpBridgeHost("fabushi.ombhrum.com"))
        XCTAssertFalse(isTrustedWebMcpBridgeHost("ombhrum.com"))
        XCTAssertFalse(isTrustedWebMcpBridgeHost("fabushi.ombhrum.com.evil.invalid"))
        XCTAssertFalse(isTrustedWebMcpBridgeHost(nil))
    }

    func testGlobalDharmaStatusFallbackIsReadOnlyAndNarrow() {
        let tool = MarketplaceModel.globalDharmaStatusFallbackTool
        XCTAssertEqual(tool.name, "status")
        XCTAssertEqual(tool.approval, "none")
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

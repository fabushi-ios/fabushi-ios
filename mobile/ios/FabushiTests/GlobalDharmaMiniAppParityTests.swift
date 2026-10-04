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

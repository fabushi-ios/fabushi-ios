import XCTest
@testable import Fabushi

final class AvatarEditorParityTests: XCTestCase {
    func testEditorScopeFencesAccountAgentAndReconnectReplacement() {
        let scope = MobileAvatarEditorScope(
            accountScopeKey: "account-a",
            agentId: "agent-a",
            reconnectGeneration: 4
        )
        XCTAssertEqual(
            scope,
            MobileAvatarEditorScope(
                accountScopeKey: "account-a",
                agentId: "agent-a",
                reconnectGeneration: 4
            )
        )
        XCTAssertNotEqual(
            scope,
            MobileAvatarEditorScope(
                accountScopeKey: "account-b",
                agentId: "agent-a",
                reconnectGeneration: 4
            )
        )
        XCTAssertNotEqual(
            scope,
            MobileAvatarEditorScope(
                accountScopeKey: "account-a",
                agentId: "agent-b",
                reconnectGeneration: 4
            )
        )
        XCTAssertNotEqual(
            scope,
            MobileAvatarEditorScope(
                accountScopeKey: "account-a",
                agentId: "agent-a",
                reconnectGeneration: 5
            )
        )
    }

    func testCropMatchesDesktopZoomAndPanBounds() {
        let initial = AvatarImagePolicy.initialCrop(width: 1_000, height: 500)
        XCTAssertEqual(initial, .init(zoom: 1, centerX: 500, centerY: 250))

        let zoomed = AvatarImagePolicy.clampCrop(
            width: 1_000,
            height: 500,
            crop: .init(zoom: 10, centerX: -100, centerY: 900)
        )
        XCTAssertEqual(zoomed.zoom, 5)
        XCTAssertEqual(zoomed.centerX, 50)
        XCTAssertEqual(zoomed.centerY, 450)

        let rect = AvatarImagePolicy.cropRect(
            width: 1_000,
            height: 500,
            crop: .init(zoom: 2, centerX: 500, centerY: 250)
        )
        XCTAssertEqual(rect.width, 250)
        XCTAssertEqual(rect.height, 250)
        XCTAssertEqual(rect.origin.x, 375)
        XCTAssertEqual(rect.origin.y, 125)
    }

    func testSourceLimitAndDesktopCharacterCatalogArePinned() {
        XCTAssertNil(AvatarImagePolicy.sourceSizeError(byteLength: 25 * 1024 * 1024))
        XCTAssertEqual(
            AvatarImagePolicy.sourceSizeError(byteLength: 25 * 1024 * 1024 + 1),
            "Choose an image smaller than 25 MB."
        )
        XCTAssertEqual(AvatarImagePolicy.sourceMaxDimension, 1_024)
        XCTAssertEqual(AvatarImagePolicy.outputSize, 256)
        XCTAssertEqual(AvatarImagePolicy.minZoom, 1)
        XCTAssertEqual(AvatarImagePolicy.maxZoom, 5)
        XCTAssertEqual(AvatarImagePolicy.shapes, [
            "blob", "pebble", "squircle", "tablet", "wedge", "hex", "cloud", "teardrop",
        ])
        XCTAssertEqual(AvatarImagePolicy.colors.map(\.id), [
            "black", "brown", "red", "orange", "yellow", "green",
            "cyan", "blue", "violet", "magenta", "gray",
        ])
    }

    func testAvatarUpdateCommandPreservesCanonicalHostOwner() {
        let image = GrokMobileBotService.avatarUpdateCommand(
            id: "agent-1",
            isGroup: false,
            avatarDataURL: "data:image/png;base64,AAAA",
            clearAvatar: false,
            avatarShape: nil,
            avatarColor: nil,
            requestId: "request-image"
        )
        XCTAssertEqual(image["type"] as? String, "bot.update")
        XCTAssertEqual(image["id"] as? String, "agent-1")
        XCTAssertEqual(image["avatar"] as? String, "data:image/png;base64,AAAA")
        XCTAssertNil(image["avatarShape"])

        let character = GrokMobileBotService.avatarUpdateCommand(
            id: "agent-1",
            isGroup: false,
            avatarDataURL: nil,
            clearAvatar: true,
            avatarShape: "cloud",
            avatarColor: "violet",
            requestId: "request-character"
        )
        XCTAssertEqual(character["avatar"] as? String, "")
        XCTAssertEqual(character["avatarShape"] as? String, "cloud")
        XCTAssertEqual(character["avatarColor"] as? String, "violet")
    }

    func testGroupAvatarCommandAndProjectionUseCanonicalGroupOwner() throws {
        let command = GrokMobileBotService.avatarUpdateCommand(
            id: "group-1",
            isGroup: true,
            avatarDataURL: "data:image/png;base64,AAAA",
            clearAvatar: false,
            avatarShape: nil,
            avatarColor: nil,
            requestId: "group-avatar"
        )
        XCTAssertEqual(command["type"] as? String, "group.update")
        let parsed = try XCTUnwrap(GrokMobileBotService.parseGroup([
            "id": "group-1",
            "name": "Group",
            "description": "",
            "memberIds": ["agent-a"],
            "avatar": "data:image/png;base64,AAAA",
        ]))
        XCTAssertEqual(parsed.avatarDataURL, "data:image/png;base64,AAAA")
        XCTAssertTrue(parsed.isGroup)
    }

    func testBotProjectionRetainsAuthoritativeAvatarFields() throws {
        let parsed = try XCTUnwrap(GrokMobileBotService.parseBot([
            "id": "agent-avatar",
            "name": "Avatar Agent",
            "description": "",
            "avatar": "data:image/png;base64,AAAA",
            "avatarShape": "cloud",
            "avatarColor": "violet",
        ]))
        XCTAssertEqual(parsed.avatarDataURL, "data:image/png;base64,AAAA")
        XCTAssertEqual(parsed.avatarShape, "cloud")
        XCTAssertEqual(parsed.avatarColor, "violet")
    }

    func testNativeAvatarDispatcherMatchesDesktopPrecedenceAndDeterministicFallbacks() {
        let persona = MobileBotSummary(id: "agent-1", name: "Agent", description: "")
        XCTAssertEqual(mobileAgentAvatarKind(persona), .persona)
        XCTAssertEqual(mobileResolvePersonaColor(agentId: "agent-1", override: nil), "red")
        XCTAssertEqual(mobileResolvePersonaShape(agentId: "agent-1", override: nil), "blob")
        XCTAssertEqual(
            mobileResolvePersonaColor(agentId: "global-dharma-bot", override: nil),
            "violet"
        )
        XCTAssertEqual(
            mobileResolvePersonaShape(agentId: "global-dharma-bot", override: nil),
            "hex"
        )
        XCTAssertEqual(
            mobileResolvePersonaColor(agentId: "agent-1", override: "black"),
            "black"
        )
        XCTAssertEqual(
            mobileResolvePersonaShape(agentId: "agent-1", override: "cloud"),
            "cloud"
        )

        let group = MobileBotSummary(
            id: "group-1",
            name: "Group",
            description: "",
            isGroup: true,
            memberIds: ["a", "b"]
        )
        XCTAssertEqual(mobileAgentAvatarKind(group), .group)

        let shared = MobileBotSummary(
            id: "room-1",
            name: "Room",
            description: "",
            isGroup: true,
            memberIds: ["a", "b"],
            isSharedRoom: true
        )
        XCTAssertEqual(mobileAgentAvatarKind(shared), .sharedRoom)

        let custom = MobileBotSummary(
            id: "room-1",
            name: "Room",
            description: "",
            avatarDataURL: "data:image/png;base64,AAAA",
            isGroup: true,
            memberIds: ["a", "b"],
            isSharedRoom: true
        )
        XCTAssertEqual(mobileAgentAvatarKind(custom), .photo)
    }

    func testAvatarActivityProjectionMatchesDesktopPriority() {
        XCTAssertEqual(
            GrokMobileBotService.avatarState(
                currentActivity: ["kind": "tool", "tool": "WebSearch"],
                awaitingUserResponsePresent: false,
                isComposingMessage: false,
                isRunning: false
            ),
            .searching
        )
        XCTAssertEqual(
            GrokMobileBotService.avatarState(
                currentActivity: ["kind": "tool", "tool": "SendToAgent"],
                awaitingUserResponsePresent: false,
                isComposingMessage: false,
                isRunning: false
            ),
            .sending
        )
        XCTAssertEqual(
            GrokMobileBotService.avatarState(
                currentActivity: ["verb": "waiting"],
                awaitingUserResponsePresent: false,
                isComposingMessage: false,
                isRunning: false
            ),
            .orbit
        )
        XCTAssertEqual(
            GrokMobileBotService.avatarState(
                currentActivity: ["verb": "coding"],
                awaitingUserResponsePresent: false,
                isComposingMessage: false,
                isRunning: false
            ),
            .working
        )
        XCTAssertEqual(
            GrokMobileBotService.avatarState(
                currentActivity: ["kind": "thinking"],
                awaitingUserResponsePresent: true,
                isComposingMessage: true,
                isRunning: true
            ),
            .idle
        )
        XCTAssertEqual(
            GrokMobileBotService.avatarState(
                currentActivity: nil,
                awaitingUserResponsePresent: false,
                isComposingMessage: true,
                isRunning: true
            ),
            .thinking
        )
    }

    func testRosterProjectionAndMiniAppMergePreserveAvatarAuthority() throws {
        let parsed = try XCTUnwrap(GrokMobileBotService.parseBot([
            "id": "agent-avatar",
            "name": "Avatar Agent",
            "description": "",
            "avatar": "data:image/png;base64,AAAA",
            "avatarShape": "cloud",
            "avatarColor": "violet",
            "isSharedRoom": true,
            "currentActivity": ["kind": "tool", "tool": "WebSearch"],
        ]))
        XCTAssertTrue(parsed.isSharedRoom)
        XCTAssertEqual(parsed.avatarState, .searching)

        let canonical = MobileBotSummary(
            id: "mini-1",
            name: "Canonical",
            description: "canonical",
            avatarDataURL: "data:image/png;base64,AAAA",
            avatarShape: "cloud",
            avatarColor: "violet",
            isRunning: true,
            avatarState: .working,
            miniAppId: "canonical-app",
            isSharedRoom: true
        )
        let installed = MobileBotSummary(
            id: "mini-1",
            name: "Installed",
            description: "installed",
            miniAppId: "installed-app"
        )
        let merged = GrokMobileBotService.mergeBots([installed], [canonical])
        let value = try XCTUnwrap(merged.first)
        XCTAssertEqual(value.avatarDataURL, canonical.avatarDataURL)
        XCTAssertEqual(value.avatarShape, "cloud")
        XCTAssertEqual(value.avatarColor, "violet")
        XCTAssertEqual(value.avatarState, .working)
        XCTAssertTrue(value.isSharedRoom)
        XCTAssertEqual(value.miniAppId, "installed-app")
    }

}

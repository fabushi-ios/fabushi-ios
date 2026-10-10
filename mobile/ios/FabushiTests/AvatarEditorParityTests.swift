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
}

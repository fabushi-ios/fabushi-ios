import XCTest
@testable import Fabushi

@MainActor
final class TelegramThemeEngineTests: XCTestCase {
    func testAllFourCanonicalThemePalettesMatchExactHexSpecs() {
        XCTAssertEqual(TelegramThemePalette.dayClassic.accentHex, 0x007AFF)
        XCTAssertEqual(TelegramThemePalette.dayClassic.outgoingBubbleHex, 0xE1FFC7)
        XCTAssertEqual(TelegramThemePalette.dayClassic.draftRedHex, 0xDD4B39)
        XCTAssertFalse(TelegramThemePalette.dayClassic.overallDarkAppearance)

        XCTAssertEqual(TelegramThemePalette.day.incomingBubbleHex, 0xF1F1F4)
        XCTAssertEqual(TelegramThemePalette.day.outgoingBubbleHex, 0x007AFF)
        XCTAssertFalse(TelegramThemePalette.day.overallDarkAppearance)

        XCTAssertEqual(TelegramThemePalette.night.plainBgHex, 0x000000)
        XCTAssertEqual(TelegramThemePalette.night.blocksBgHex, 0x000000)
        XCTAssertEqual(TelegramThemePalette.night.cardBgHex, 0x1C1C1D)
        XCTAssertTrue(TelegramThemePalette.night.overallDarkAppearance)

        XCTAssertEqual(TelegramThemePalette.nightTinted.plainBgHex, 0x18222D)
        XCTAssertEqual(TelegramThemePalette.nightTinted.accentHex, 0x2EA6FF)
        XCTAssertEqual(TelegramThemePalette.nightTinted.outgoingBubbleHex, 0x3D6A97)
        XCTAssertTrue(TelegramThemePalette.nightTinted.overallDarkAppearance)
    }

    func testCmdDCycleOrderAndPersistenceRoundtrip() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("ThemePersist_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let engine = TelegramThemeEngine(appSupportDir: dir)
        engine.currentMode = .night
        XCTAssertEqual(engine.cycleThemeMode(), .nightTinted)
        XCTAssertEqual(engine.cycleThemeMode(), .day)
        XCTAssertEqual(engine.cycleThemeMode(), .dayClassic)
        XCTAssertEqual(engine.cycleThemeMode(), .night)

        engine.fontSizeStep = .extraLarge
        engine.bubbleCorners = BubbleCornersConfig(mainRadius: 12, isBubbleFreeMode: true)
        engine.useLiquidGlassChrome = false

        let restored = TelegramThemeEngine(appSupportDir: dir)
        XCTAssertEqual(restored.currentMode, .night)
        XCTAssertEqual(restored.fontSizeStep, .extraLarge)
        XCTAssertEqual(restored.bubbleCorners.mainRadius, 12)
        XCTAssertTrue(restored.bubbleCorners.isBubbleFreeMode)
        XCTAssertFalse(restored.useLiquidGlassChrome)
    }

    func testDeterministicPeerAvatarGradientsModuloSeven() {
        XCTAssertEqual(TelegramThemeEngine.peerAvatarGradientTable.count, 7)
        let expectedBottoms: [UInt32] = [0xFC5C51, 0xFA790F, 0x895DD5, 0x0FB297, 0x00C0C2, 0x3CA5EC, 0x3D72ED]
        for idx in 0..<7 {
            let g = TelegramThemeEngine.peerAvatarGradient(for: Int64(idx))
            XCTAssertEqual(g.index, idx)
            XCTAssertEqual(g.bottomHex, expectedBottoms[idx])
        }
        // Negative peer IDs use magnitude % 7 without crashing
        let neg = TelegramThemeEngine.peerAvatarGradient(for: -100123456)
        XCTAssertEqual(neg, TelegramThemeEngine.peerAvatarGradient(for: 100123456))
    }
}

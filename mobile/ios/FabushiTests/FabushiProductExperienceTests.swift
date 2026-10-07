import XCTest
@testable import Fabushi

final class FabushiProductExperienceTests: XCTestCase {
    func testCanonicalBrandContractMatchesDesktopAuthority() {
        XCTAssertEqual(FabushiBrand.name, "法布施")
        XCTAssertEqual(FabushiBrand.englishName, "大乘")
        XCTAssertEqual(FabushiBrand.tagline, "经文、禅修、法流与全球法布施，一处安静开始。")
        XCTAssertEqual(FabushiBrand.mission, "用现代产品体验承接佛法传播、修行记录、禅修冥想与同行连接。")
        XCTAssertEqual(FabushiBrand.domain, "ombhrum.com")
    }

    func testCanonicalProductModulesRemainAvailableOnIOS() {
        XCTAssertEqual(
            FabushiProductModule.allCases.map(\.rawValue),
            ["globalDharma", "flashcards", "sutra", "meditation", "faliu", "ai"]
        )
        XCTAssertEqual(FabushiProductModule.globalDharma.title, "全球法布施")
        XCTAssertEqual(FabushiProductModule.flashcards.title, "背诵闪卡")
        XCTAssertEqual(FabushiProductModule.sutra.title, "经文听诵")
        XCTAssertEqual(FabushiProductModule.meditation.title, "禅室修行")
        XCTAssertEqual(FabushiProductModule.faliu.title, "法流学习")
        XCTAssertEqual(FabushiProductModule.ai.title, "大乘 AI")
    }

    func testFlashcardReviewIntervalsPreserveAgainHardGoodEasyOrdering() {
        let current = 4
        let again = flashcardNextIntervalDays(rating: .again, currentIntervalDays: current)
        let hard = flashcardNextIntervalDays(rating: .hard, currentIntervalDays: current)
        let good = flashcardNextIntervalDays(rating: .good, currentIntervalDays: current)
        let easy = flashcardNextIntervalDays(rating: .easy, currentIntervalDays: current)

        XCTAssertEqual(again, 1)
        XCTAssertGreaterThan(hard, again)
        XCTAssertGreaterThan(good, hard)
        XCTAssertGreaterThan(easy, good)
    }

    func testFlashcardIntervalsNeverCollapseBelowSafeMinimums() {
        XCTAssertEqual(
            flashcardNextIntervalDays(rating: .again, currentIntervalDays: 0),
            1
        )
        XCTAssertGreaterThanOrEqual(
            flashcardNextIntervalDays(rating: .hard, currentIntervalDays: 0),
            2
        )
        XCTAssertGreaterThanOrEqual(
            flashcardNextIntervalDays(rating: .good, currentIntervalDays: 0),
            3
        )
        XCTAssertGreaterThanOrEqual(
            flashcardNextIntervalDays(rating: .easy, currentIntervalDays: 0),
            5
        )
    }
}

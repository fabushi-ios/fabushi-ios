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

    func testDachengHeroPromptsAndRegionsMatchDesktopContract() {
        XCTAssertEqual(fabushiHeroPrompts.count, 5)
        XCTAssertEqual(
            fabushiHeroPrompts.map(\.label),
            ["你是谁", "全球法布施", "背诵闪卡", "原来是这样", "我今天修什么？"]
        )
        XCTAssertEqual(fabushiHeroPrompts[1].tool, .globalDharma)
        XCTAssertEqual(fabushiHeroPrompts[2].tool, .flashcards)
        XCTAssertEqual(fabushiGlobalDharmaRegions.count, 12)
        XCTAssertEqual(fabushiGlobalDharmaRegions.first, "中国")
        XCTAssertEqual(fabushiGlobalDharmaRegions.last, "南非")
    }

    func testFlashcardGenerationCreatesClozeAndBidirectionalCardsPerSentence() {
        var nextID = 0
        let cards = makeFabushiFlashcards(
            from: "色不异空空不异色。应无所住而生其心。"
        ) {
            nextID += 1
            return "card-\(nextID)"
        }

        XCTAssertEqual(cards.count, 4)
        XCTAssertEqual(cards.map(\.id), ["card-1", "card-2", "card-3", "card-4"])
        XCTAssertEqual(cards.map(\.kind), ["挖空", "双向", "挖空", "双向"])
        XCTAssertTrue(cards[0].front.contains("〔……〕"))
        XCTAssertTrue(cards[1].front.hasPrefix("请背诵并解释："))
        XCTAssertTrue(cards.allSatisfy { $0.reviews == 0 && $0.due == "现在" })
    }

    func testDesktopReviewDueLabelsAndGlobalDharmaFallbackRemainExact() {
        XCTAssertEqual(flashcardDueLabel(for: .again), "10 分钟后")
        XCTAssertEqual(flashcardDueLabel(for: .hard), "明天")
        XCTAssertEqual(flashcardDueLabel(for: .good), "3 天后")
        XCTAssertEqual(flashcardDueLabel(for: .easy), "7 天后")

        let checklist = buildFabushiGlobalDharmaChecklist("  ")
        XCTAssertEqual(checklist.count, 12)
        XCTAssertEqual(checklist.first?.region, "中国")
        XCTAssertEqual(
            checklist.first?.text,
            "愿以此功德，普及于一切，我等与众生，皆共成佛道。"
        )
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

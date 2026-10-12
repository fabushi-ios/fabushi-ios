import XCTest
@testable import Fabushi

final class SharedAutomationScheduleParityTests: XCTestCase {
    func testTriggerValidationNormalizationAndDescriptions() {
        XCTAssertEqual(normalizeReactionEmoji("::Thumbs_Up::"), "thumbs_up")
        XCTAssertTrue(isValidReactionEmoji("thumbs_up"))
        XCTAssertFalse(isValidReactionEmoji("thumbs up"))
        XCTAssertTrue(isValidGithubRepo("owner/repo"))
        XCTAssertFalse(isValidGithubRepo("owner/repo/extra"))
        XCTAssertTrue(isValidGitBranch("feature/test"))
        XCTAssertFalse(isValidGitBranch("bad..branch"))

        let github = GithubTrigger(repo: "owner/repo", events: ["ci-failed"], ciBranch: "main", userAllowlist: ["alice"])
        XCTAssertEqual(describeGithubListener(github), "When CI fails on main in owner/repo (by @alice)")
        let slack = SlackTrigger(channel: "*", match: .keyword("urgent"))
        XCTAssertEqual(describeSlackListener(slack), "When \"urgent\" is mentioned anywhere on Slack")
    }

    func testCronFieldsAliasesAndDomDowSemantics() {
        XCTAssertEqual(parseCronField("*/15", min: 0, max: 59), Set([0,15,30,45]))
        XCTAssertNil(parseCronField("60", min: 0, max: 59))
        XCTAssertEqual(expandCronAlias("@daily"), "0 0 * * *")
        let matcher = parseCron("0 9 1 * 1")
        XCTAssertNotNil(matcher)
        XCTAssertTrue(cronDayMatches(matcher!, wall: .init(year: 2026, minute: 0, hour: 9, month: 9, dayOfMonth: 1, dayOfWeek: 4)))
        XCTAssertTrue(cronDayMatches(matcher!, wall: .init(year: 2026, minute: 0, hour: 9, month: 9, dayOfMonth: 2, dayOfWeek: 1)))
        XCTAssertFalse(cronDayMatches(matcher!, wall: .init(year: 2026, minute: 0, hour: 9, month: 9, dayOfMonth: 2, dayOfWeek: 2)))
    }

    func testEveryAndCronNextRun() {
        XCTAssertEqual(parseEveryIntervalMs("@every 5m"), 300_000)
        XCTAssertNil(parseEveryIntervalMs("@every 0m"))
        XCTAssertEqual(computeNextRunAt("@every 5m", afterMs: 1_000), 301_000)

        let after: Int64 = 1_795_027_230_000
        let next = computeNextRunAt("0 * * * *", afterMs: after, timeZone: "UTC")
        XCTAssertNotNil(next)
        if let next {
            XCTAssertEqual(wallClockOfInstant(next, timeZone: "UTC").minute, 0)
        }
    }

    func testTimeZoneCompilationAndHumanDescriptions() {
        XCTAssertNotNil(compileCronMatcher("CRON_TZ=America/Los_Angeles 0 9 * * 1-5"))
        XCTAssertNil(compileCronMatcher("CRON_TZ=Not/AZone 0 9 * * *"))
        XCTAssertEqual(describeSchedule("@every 1h"), "Every hour")
        XCTAssertEqual(describeSchedule("@daily"), "Every day at 12:00 AM")
        XCTAssertEqual(describeSchedule("0 9 * * 1-5"), "Weekdays at 9:00 AM")
    }

    func testGroupedTriggerDescriptionAndAnchor() {
        let trigger = AutomationTrigger.group([
            .cron(.init(schedule: "@daily")),
            .slack(.init(channel: "C1", match: .mention)),
        ])
        XCTAssertEqual(describeTrigger(trigger), "Every day at 12:00 AM or when @mentioned in C1")
        XCTAssertEqual(triggerCronSchedules(trigger), ["@daily"])
        XCTAssertEqual(triggerListeners(trigger).count, 1)
        XCTAssertEqual(automationAnchor(createdAt: 10, lastRunAt: 20), 20)
        XCTAssertEqual(automationAnchor(createdAt: 10, lastRunAt: nil), 10)
    }
}

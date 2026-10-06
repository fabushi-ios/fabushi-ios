import XCTest
@testable import Fabushi

final class GrokMobileRoutinesControllerTests: XCTestCase {
    private let valid: [String: Any] = [
        "id": "routine-1",
        "agentId": "agent-1",
        "name": "Daily research",
        "prompt": "Summarize sources",
        "schedule": "@daily",
        "enabled": true,
        "createdAtMs": 10,
        "runs": [
            [
                "id": "run-1",
                "status": "ok",
                "startedAt": 20,
                "detail": "Completed",
            ],
        ],
        "lastRunAtMs": 20,
        "nextRunAtMs": 30,
    ]

    func testRoutineProjectionRequiresDesktopLifecycleFields() {
        let projected = MobileBotRoutinesModel.parseAutomation(valid)
        XCTAssertEqual(projected?.id, "routine-1")
        XCTAssertEqual(projected?.agentId, "agent-1")
        XCTAssertEqual(projected?.name, "Daily research")
        XCTAssertEqual(projected?.prompt, "Summarize sources")
        XCTAssertEqual(projected?.schedule, "@daily")
        XCTAssertEqual(projected?.isEnabled, true)
        XCTAssertEqual(projected?.createdAtMs, 10)
        XCTAssertEqual(projected?.runs.count, 1)
        XCTAssertEqual(projected?.runs.first?.status, .ok)
        XCTAssertEqual(projected?.runs.first?.startedAt, 20)
        XCTAssertEqual(projected?.runs.first?.detail, "Completed")
        XCTAssertEqual(projected?.lastRunAtMs, 20)
        XCTAssertEqual(projected?.nextRunAtMs, 30)
    }

    func testRoutineProjectionFailsClosedOnMalformedRows() {
        for field in ["id", "agentId", "name", "prompt", "schedule", "enabled", "createdAtMs"] {
            var row = valid
            row.removeValue(forKey: field)
            XCTAssertNil(
                MobileBotRoutinesModel.parseAutomation(row),
                "missing \(field) must fail closed"
            )
        }

        for field in ["id", "agentId"] {
            var row = valid
            row[field] = "   "
            XCTAssertNil(
                MobileBotRoutinesModel.parseAutomation(row),
                "blank \(field) must fail closed"
            )
        }

        var badEnabled = valid
        badEnabled["enabled"] = "true"
        XCTAssertNil(MobileBotRoutinesModel.parseAutomation(badEnabled))

        var badLastRun = valid
        badLastRun["lastRunAtMs"] = "later"
        XCTAssertNil(MobileBotRoutinesModel.parseAutomation(badLastRun))
    }

    func testRoutineListRejectsPartialProjectionAndFiltersAgentScope() throws {
        var other = valid
        other["id"] = "routine-2"
        other["agentId"] = "agent-2"

        let rows = try MobileBotRoutinesModel.parseAutomations(
            [valid, other],
            agentId: "agent-1"
        )
        XCTAssertEqual(rows.map(\.id), ["routine-1"])

        var malformed = valid
        malformed.removeValue(forKey: "schedule")
        XCTAssertThrowsError(
            try MobileBotRoutinesModel.parseAutomations(
                [valid, malformed],
                agentId: "agent-1"
            )
        )
    }

    func testSnapshotPreservesDesktopLoadingReadyFailedAndUnavailableStates() {
        let routine = MobileBotRoutinesModel.parseAutomation(valid)!

        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: nil,
                error: nil,
                refreshing: false,
                capabilityUnavailable: false
            ),
            .loading(previous: [])
        )
        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: [],
                error: nil,
                refreshing: false,
                capabilityUnavailable: false
            ),
            .empty
        )
        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: [routine],
                error: nil,
                refreshing: true,
                capabilityUnavailable: false
            ),
            .ready([routine])
        )
        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: [routine],
                error: "network",
                refreshing: false,
                capabilityUnavailable: false
            ),
            .failed(
                value: [routine],
                previous: [routine],
                message: "network"
            )
        )
        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: nil,
                error: "source/capability-unavailable",
                refreshing: false,
                capabilityUnavailable: true
            ),
            .unavailable
        )
    }

    func testPendingPolicyDeduplicatesOnlyMatchingAutomation() {
        let pending = Set(["routine-1"])
        XCTAssertFalse(
            MobileBotRoutinesModel.canBegin("routine-1", pending: pending)
        )
        XCTAssertTrue(
            MobileBotRoutinesModel.canBegin("routine-2", pending: pending)
        )
    }

    func testCommandsStayOnCanonicalHostAutomationSurface() {
        let list = MobileBotRoutinesModel.commandList(
            agentId: "agent-1",
            requestId: "req-list"
        )
        XCTAssertEqual(list["type"] as? String, "automation.list")
        XCTAssertEqual(list["agentId"] as? String, "agent-1")

        let enabled = MobileBotRoutinesModel.commandSetEnabled(
            agentId: "agent-1",
            automationId: "routine-1",
            isEnabled: false,
            requestId: "req-enabled"
        )
        XCTAssertEqual(enabled["type"] as? String, "automation.setEnabled")
        XCTAssertEqual(enabled["id"] as? String, "routine-1")
        XCTAssertEqual(enabled["enabled"] as? Bool, false)

        let run = MobileBotRoutinesModel.commandRun(
            agentId: "agent-1",
            automationId: "routine-1",
            requestId: "req-run"
        )
        XCTAssertEqual(run["type"] as? String, "automation.run")
    }
    func testRoutineProjectionFailsClosedOnMalformedRunHistory() {
        var missingRuns = valid
        missingRuns.removeValue(forKey: "runs")
        XCTAssertNil(MobileBotRoutinesModel.parseAutomation(missingRuns))

        for mutation in [
            { (row: inout [String: Any]) in row["status"] = "done" },
            { (row: inout [String: Any]) in row["startedAt"] = "later" },
            { (row: inout [String: Any]) in row["detail"] = 42 },
            { (row: inout [String: Any]) in row["event"] = false },
        ] {
            var row = (valid["runs"] as! [[String: Any]])[0]
            mutation(&row)
            var automation = valid
            automation["runs"] = [row]
            XCTAssertNil(MobileBotRoutinesModel.parseAutomation(automation))
        }
    }

    func testRunHistoryPresentationMatchesDesktopRelativeAndStatusContract() {
        let now: Int64 = 1_700_000_000_000
        let running = MobileBotRoutineRun(
            id: "running",
            status: .running,
            startedAt: now - 30_000,
            detail: nil,
            event: "message.created"
        )
        let ok = MobileBotRoutineRun(
            id: "ok",
            status: .ok,
            startedAt: now - 120_000,
            detail: "Completed",
            event: nil
        )
        let failed = MobileBotRoutineRun(
            id: "failed",
            status: .error,
            startedAt: now + 120_000,
            detail: "Network",
            event: nil
        )

        let history = MobileBotRoutineRunHistoryModel.presentHistory(
            [running, ok, failed],
            now: now,
            timeZoneIdentifier: "UTC"
        )
        XCTAssertFalse(history.empty)
        XCTAssertEqual(history.rows[0].timestampLabel, "Just now")
        XCTAssertEqual(history.rows[0].accessibilityLabel, "Running")
        XCTAssertEqual(history.rows[0].iconName, "loading")
        XCTAssertTrue(history.rows[0].statusRole)
        XCTAssertEqual(history.rows[0].title, "message.created")
        XCTAssertEqual(history.rows[1].timestampLabel, "2 min ago")
        XCTAssertEqual(history.rows[1].accessibilityLabel, "Succeeded")
        XCTAssertEqual(history.rows[1].iconName, "check")
        XCTAssertEqual(history.rows[1].title, "Completed")
        XCTAssertEqual(history.rows[2].timestampLabel, "In 2 min")
        XCTAssertEqual(history.rows[2].accessibilityLabel, "Failed")
        XCTAssertEqual(history.rows[2].iconName, "close")
    }

    func testRunHistoryClockLazilyTicksAndPublishesTimeZoneChanges() {
        var scheduledName: String?
        var scheduledInterval: Int?
        var tick: (() -> Void)?
        var cancellations = 0

        let clock = MobileBotRoutineRunHistoryClock(
            initialTimeZone: MobileBotRoutineTimeZoneState(
                detectedTimeZone: "America/Phoenix",
                overrideTimeZone: nil
            ),
            now: { Date(timeIntervalSince1970: 123) },
            scheduler: { name, interval, callback in
                scheduledName = name
                scheduledInterval = interval
                tick = callback
                return {
                    cancellations += 1
                }
            }
        )

        XCTAssertEqual(clock.nowMilliseconds, 123_000)
        XCTAssertEqual(clock.timeZoneIdentifier, "America/Phoenix")
        XCTAssertNil(scheduledName)

        var notifications = 0
        let stop = clock.subscribe {
            notifications += 1
        }

        XCTAssertEqual(scheduledName, "agents-now-tick")
        XCTAssertEqual(scheduledInterval, 30_000)

        tick?()
        XCTAssertEqual(notifications, 1)

        clock.ingestTimeZone(
            MobileBotRoutineTimeZoneState(
                detectedTimeZone: "America/Phoenix",
                overrideTimeZone: "Asia/Tokyo"
            )
        )
        XCTAssertEqual(clock.timeZoneIdentifier, "Asia/Tokyo")
        XCTAssertEqual(notifications, 2)

        clock.ingestTimeZone(
            MobileBotRoutineTimeZoneState(
                detectedTimeZone: "America/Phoenix",
                overrideTimeZone: "Asia/Tokyo"
            )
        )
        XCTAssertEqual(notifications, 2)

        stop()
        XCTAssertEqual(cancellations, 1)
    }

    func testRunHistoryClockFallsBackToUTCAndDisposeStopsNotifications() {
        var tick: (() -> Void)?
        var cancellations = 0
        let clock = MobileBotRoutineRunHistoryClock(
            initialTimeZone: MobileBotRoutineTimeZoneState(
                detectedTimeZone: nil,
                overrideTimeZone: nil
            ),
            scheduler: { _, _, callback in
                tick = callback
                return {
                    cancellations += 1
                }
            }
        )

        XCTAssertEqual(clock.timeZoneIdentifier, "UTC")
        var notifications = 0
        _ = clock.subscribe {
            notifications += 1
        }

        clock.dispose()
        XCTAssertEqual(cancellations, 1)
        tick?()
        XCTAssertEqual(notifications, 0)

        let stopAfterDispose = clock.subscribe {
            notifications += 1
        }
        stopAfterDispose()
        XCTAssertEqual(notifications, 0)
        XCTAssertEqual(cancellations, 1)
    }

    @MainActor
    private final class FakeRoutinesController: MobileBotRoutinesControlling {
        var snapshot: MobileBotRoutinesSnapshot
        var runPending: Set<String> = []
        var resetCalls = 0
        var refreshAgentIds: [String] = []
        var runCalls: [(agentId: String, automationId: String)] = []
        var subscriptions = 0
        var unsubscriptions = 0
        private var listeners: [UUID: () -> Void] = [:]

        init(snapshot: MobileBotRoutinesSnapshot) {
            self.snapshot = snapshot
        }

        @discardableResult
        func subscribe(_ listener: @escaping () -> Void) -> () -> Void {
            subscriptions += 1
            let id = UUID()
            listeners[id] = listener
            return { [weak self] in
                guard let self else { return }
                if self.listeners.removeValue(forKey: id) != nil {
                    self.unsubscriptions += 1
                }
            }
        }

        func refresh(agentId: String) async {
            refreshAgentIds.append(agentId)
            emit()
        }

        func runNow(agentId: String, automationId: String) async throws {
            runCalls.append((agentId, automationId))
            emit()
        }

        func reset() {
            resetCalls += 1
            snapshot = .loading(previous: [])
            runPending.removeAll()
            emit()
        }

        func emit() {
            for listener in Array(listeners.values) {
                listener()
            }
        }
    }

    @MainActor
    private func providerRoutine() -> MobileBotRoutine {
        MobileBotRoutine(
            id: "routine-1",
            agentId: "agent-1",
            name: "Daily research",
            prompt: "Summarize sources",
            schedule: "@daily",
            isEnabled: true,
            createdAtMs: 10,
            runs: [
                MobileBotRoutineRun(
                    id: "run-1",
                    status: .ok,
                    startedAt: 20,
                    detail: "Completed",
                    event: nil
                ),
            ],
            lastRunAtMs: 20,
            nextRunAtMs: 30
        )
    }

    @MainActor
    func testRunHistoryProviderLazilySubscribesAndStopsWithLastListener() {
        let fake = FakeRoutinesController(snapshot: .ready([providerRoutine()]))
        var clockStarts = 0
        var clockStops = 0
        let clock = MobileBotRoutineRunHistoryClock(
            initialTimeZone: MobileBotRoutineTimeZoneState(
                detectedTimeZone: "UTC",
                overrideTimeZone: nil
            ),
            now: { Date(timeIntervalSince1970: 1) },
            scheduler: { _, _, _ in
                clockStarts += 1
                return { clockStops += 1 }
            }
        )
        let provider = MobileBotRoutineRunHistoryProvider(
            controller: fake,
            clock: clock,
            initialScope: MobileBotRoutineRunHistoryScope(
                accountKey: "account-a",
                agentId: "agent-1",
                automationId: "routine-1"
            )
        )

        XCTAssertEqual(fake.subscriptions, 0)
        XCTAssertEqual(clockStarts, 0)

        let stopFirst = provider.subscribe {}
        XCTAssertEqual(fake.subscriptions, 1)
        XCTAssertEqual(clockStarts, 1)

        let stopSecond = provider.subscribe {}
        XCTAssertEqual(fake.subscriptions, 1)
        XCTAssertEqual(clockStarts, 1)

        stopFirst()
        XCTAssertEqual(fake.unsubscriptions, 0)
        XCTAssertEqual(clockStops, 0)

        stopSecond()
        XCTAssertEqual(fake.unsubscriptions, 1)
        XCTAssertEqual(clockStops, 1)
    }

    @MainActor
    func testRunHistoryProviderScopesRefreshReconnectAndRunNow() async throws {
        let fake = FakeRoutinesController(snapshot: .ready([providerRoutine()]))
        fake.runPending = ["routine-1"]
        let clock = MobileBotRoutineRunHistoryClock(
            initialTimeZone: MobileBotRoutineTimeZoneState(
                detectedTimeZone: "UTC",
                overrideTimeZone: nil
            ),
            now: { Date(timeIntervalSince1970: 0.020) },
            scheduler: { _, _, _ in { } }
        )
        let provider = MobileBotRoutineRunHistoryProvider(
            controller: fake,
            clock: clock,
            initialScope: MobileBotRoutineRunHistoryScope(
                accountKey: "account-a",
                agentId: "agent-1",
                automationId: "routine-1"
            )
        )

        switch provider.snapshot() {
        case .ready(let scope, let rows, let pending):
            XCTAssertEqual(scope.accountKey, "account-a")
            XCTAssertEqual(scope.agentId, "agent-1")
            XCTAssertEqual(scope.automationId, "routine-1")
            XCTAssertEqual(rows.map(\.id), ["run-1"])
            XCTAssertEqual(rows.first?.title, "Completed")
            XCTAssertEqual(rows.first?.timestampLabel, "Just now")
            XCTAssertTrue(pending)
        default:
            XCTFail("expected ready run-history snapshot")
        }

        _ = await provider.refresh()
        _ = await provider.refreshOnReconnect()
        XCTAssertEqual(fake.refreshAgentIds, ["agent-1", "agent-1"])

        let didRun = try await provider.runNow()
        XCTAssertTrue(didRun)
        XCTAssertEqual(fake.runCalls.count, 1)
        XCTAssertEqual(fake.runCalls.first?.agentId, "agent-1")
        XCTAssertEqual(fake.runCalls.first?.automationId, "routine-1")

        fake.snapshot = .empty
        let missingDidRun = try await provider.runNow()
        XCTAssertFalse(missingDidRun)
        XCTAssertEqual(fake.runCalls.count, 1)

        provider.setScope(
            MobileBotRoutineRunHistoryScope(
                accountKey: "account-b",
                agentId: "agent-1",
                automationId: "routine-1"
            )
        )
        XCTAssertEqual(fake.resetCalls, 1)
        if case .loading(let scope, _, _) = provider.snapshot() {
            XCTAssertEqual(scope.accountKey, "account-b")
        } else {
            XCTFail("account scope reset must return loading")
        }
    }

    @MainActor
    func testRunHistoryProviderDisposeUnsubscribesAndFencesNotifications() {
        let fake = FakeRoutinesController(snapshot: .ready([providerRoutine()]))
        var tick: (() -> Void)?
        var clockStops = 0
        let clock = MobileBotRoutineRunHistoryClock(
            initialTimeZone: MobileBotRoutineTimeZoneState(
                detectedTimeZone: "UTC",
                overrideTimeZone: nil
            ),
            scheduler: { _, _, callback in
                tick = callback
                return { clockStops += 1 }
            }
        )
        let provider = MobileBotRoutineRunHistoryProvider(
            controller: fake,
            clock: clock,
            initialScope: MobileBotRoutineRunHistoryScope(
                accountKey: "account-a",
                agentId: "agent-1",
                automationId: "routine-1"
            )
        )

        var notifications = 0
        _ = provider.subscribe { notifications += 1 }
        fake.emit()
        tick?()
        XCTAssertEqual(notifications, 2)

        provider.dispose()
        XCTAssertEqual(fake.unsubscriptions, 1)
        XCTAssertEqual(clockStops, 1)
        XCTAssertEqual(provider.snapshot(), .unavailable)

        fake.emit()
        tick?()
        XCTAssertEqual(notifications, 2)
    }


    func testSchedulePickerMatchesDesktopQuarterHourContract() {
        let options = MobileBotRoutineSchedule.pickerOptions()
        XCTAssertEqual(mobileBotRoutineScheduleIntervalMinutes, 15)
        XCTAssertEqual(options.count, 96)
        XCTAssertEqual(options.first?.label, "12:00 AM")
        XCTAssertEqual(options.first?.schedule, "0 0 * * *")
        XCTAssertEqual(options[1].label, "12:15 AM")
        XCTAssertEqual(options[1].schedule, "15 0 * * *")
        XCTAssertEqual(options.last?.label, "11:45 PM")
        XCTAssertEqual(options.last?.schedule, "45 23 * * *")
    }

    func testCustomScheduleBlurNormalizesValidatesAndFailsClosed() {
        XCTAssertEqual(
            MobileBotRoutineSchedule.resolveCustomBlur("  15   9  * * 1-5  "),
            MobileBotRoutineCustomScheduleBlurResult(
                schedule: "15 9 * * 1-5",
                isInvalid: false,
                shouldCommit: true
            )
        )
        XCTAssertEqual(
            MobileBotRoutineSchedule.resolveCustomBlur("   "),
            MobileBotRoutineCustomScheduleBlurResult(
                schedule: "",
                isInvalid: false,
                shouldCommit: false
            )
        )
        XCTAssertEqual(
            MobileBotRoutineSchedule.resolveCustomBlur("61 25 * * *"),
            MobileBotRoutineCustomScheduleBlurResult(
                schedule: "61 25 * * *",
                isInvalid: true,
                shouldCommit: false
            )
        )
    }

    func testScheduleValidationAcceptsDesktopAliasesIntervalsAndTimeZones() {
        for schedule in [
            "@hourly",
            "@daily",
            "@weekly",
            "@monthly",
            "@every 15m",
            "@every 2h",
            "0 9 * * 1-5",
            "*/15 8-18 * * 1-5",
            "CRON_TZ=America/Phoenix 0 9 * * *",
            "TZ=UTC 0 0 * * *",
        ] {
            XCTAssertTrue(
                MobileBotRoutineSchedule.isValid(schedule),
                "expected valid schedule: \(schedule)"
            )
        }
    }

    func testScheduleValidationRejectsMalformedOrOutOfRangeForms() {
        for schedule in [
            "",
            "@every 0m",
            "@every nope",
            "0 0 * *",
            "60 0 * * *",
            "0 24 * * *",
            "0 0 0 * *",
            "0 0 * 13 *",
            "0 0 * * 8",
            "*/0 * * * *",
            "CRON_TZ=Not/AZone 0 9 * * *",
        ] {
            XCTAssertFalse(
                MobileBotRoutineSchedule.isValid(schedule),
                "expected invalid schedule: \(schedule)"
            )
        }
    }


    func testRoutineTriggerSchemaRoundTripsSupportedNativeForms() {
        let forms: [RoutineTriggerForm] = [
            .schedule(" 0 9 * * 1-5 "),
            .slack(channel: "alerts", match: .keyword("regression")),
            .github(
                repo: "openai/example",
                events: ["pr-opened", "ci-failed"],
                userAllowlist: "@Alice, alice Bob",
                ciBranch: "main"
            ),
            .microsoftTeams(
                tenantId: "tenant-a",
                teamIds: "team-a, team-b",
                channelIds: "channel-a",
                messageContains: "urgent",
                messageContainsIsRegex: false,
                blockUnauthenticatedTeamsUsers: true
            ),
            .linear(
                eventCase: "statusChanged",
                statusIds: "done, blocked",
                cycleIds: "",
                projectIds: "project-a",
                teamIds: "team-a"
            ),
            .sentry(eventCase: "issueResolved", projectIds: "web, api"),
            .pagerduty(eventCase: "incidentTriggered", serviceIds: "payments"),
        ]

        let trigger = routineTriggerFromForms(forms)
        XCTAssertNotNil(trigger)
        XCTAssertEqual(trigger.map(triggerList).map(\.count), forms.count)

        let roundTripped = trigger.flatMap(routineTriggerForms(from:))
        XCTAssertEqual(roundTripped?.count, forms.count)

        guard let roundTripped else {
            return XCTFail("expected trigger forms")
        }
        guard case .schedule(let schedule) = roundTripped[0] else {
            return XCTFail("expected schedule form")
        }
        XCTAssertEqual(schedule, "0 9 * * 1-5")

        guard case .github(let repo, let events, let users, let branch) = roundTripped[2] else {
            return XCTFail("expected github form")
        }
        XCTAssertEqual(repo, "openai/example")
        XCTAssertEqual(events, ["pr-opened", "ci-failed"])
        XCTAssertEqual(users, "Alice, Bob")
        XCTAssertEqual(branch, "main")
    }

    func testRoutineTriggerSchemaFailsClosedOnInvalidFormsAndRowCounts() {
        let invalid: [RoutineTriggerForm] = [
            .schedule("61 25 * * *"),
            .slack(channel: "   ", match: .message),
            .slack(channel: "alerts", match: .keyword("   ")),
            .github(repo: "not-a-repo", events: ["pr-opened"], userAllowlist: "", ciBranch: ""),
            .github(repo: "openai/example", events: ["ci-failed"], userAllowlist: "", ciBranch: "bad branch"),
            .microsoftTeams(
                tenantId: "",
                teamIds: "team-a",
                channelIds: "",
                messageContains: "",
                messageContainsIsRegex: false,
                blockUnauthenticatedTeamsUsers: false
            ),
            .linear(eventCase: "unknown", statusIds: "", cycleIds: "", projectIds: "", teamIds: ""),
            .sentry(eventCase: "unknown", projectIds: ""),
            .pagerduty(eventCase: "unknown", serviceIds: ""),
        ]

        for form in invalid {
            XCTAssertFalse(routineTriggerFormIsValid(form), "invalid trigger form must fail closed: \(form)")
        }

        XCTAssertNil(routineTriggerFromForms([]))
        XCTAssertNil(
            routineTriggerFromForms(
                Array(repeating: RoutineTriggerForm.schedule("@daily"), count: TRIGGER_MAX_GROUP_LISTENERS + 1)
            )
        )
    }

    func testRoutineTriggerSchemaNormalizesReactionEmojiAndGitHubAllowlist() {
        let reaction = routineTriggerFormToMember(
            .slack(
                channel: "*",
                match: .reaction(
                    emoji: [":Ship:", "ship", "bad emoji", "rocket::skin", "rocket"],
                    bySelf: true
                )
            )
        )
        guard case .slack(let slack)? = reaction,
              case .reaction(let emoji, let bySelf) = slack.match
        else {
            return XCTFail("expected reaction listener")
        }
        XCTAssertEqual(emoji, ["ship", "rocket"])
        XCTAssertEqual(bySelf, true)

        let github = routineTriggerFormToMember(
            .github(
                repo: "owner/repo",
                events: ["pr-opened"],
                userAllowlist: "@Alice alice BOB",
                ciBranch: ""
            )
        )
        guard case .github(let value)? = github else {
            return XCTFail("expected github listener")
        }
        XCTAssertEqual(value.userAllowlist, ["Alice", "BOB"])
        XCTAssertNil(value.ciBranch)
    }


    @MainActor
    func testTriggerDraftControllerEnforcesEightRowsAndCommitsOnlyValidDrafts() async {
        var committed: [[RoutineTriggerForm]] = []
        let controller = MobileBotRoutineTriggerDraftController(
            initialRows: [.schedule("@daily")],
            onDraftCommit: { committed.append($0) }
        )

        let didCommit = await controller.addRowAndCommit(
            .github(
                repo: "owner/repo",
                events: ["pr-opened"],
                userAllowlist: "",
                ciBranch: ""
            )
        )
        XCTAssertTrue(didCommit)
        XCTAssertEqual(controller.rows.count, 2)
        XCTAssertEqual(controller.lastValidRows.count, 2)
        XCTAssertEqual(committed.count, 1)

        while controller.rows.count < MobileBotRoutineTriggerDraftController.maximumRows {
            _ = await controller.addRow(.schedule("@hourly"))
        }
        XCTAssertEqual(controller.rows.count, TRIGGER_MAX_GROUP_LISTENERS)
        XCTAssertFalse(await controller.addRowAndCommit(.schedule("@weekly")))
        XCTAssertEqual(controller.rows.count, TRIGGER_MAX_GROUP_LISTENERS)
    }

    @MainActor
    func testTriggerDraftControllerCustomScheduleFailsClosedAndReturnsFocus() async {
        var changes: [[RoutineTriggerForm]] = []
        var closeCommits: [[RoutineTriggerForm]] = []
        let controller = MobileBotRoutineTriggerDraftController(
            initialRows: [.schedule("@daily")],
            onDraftChange: { changes.append($0) },
            onCommitOrRevert: { closeCommits.append($0) }
        )

        controller.openEditor(0)
        let invalid = await controller.blurCustomSchedule(0, value: "61 25 * * *")
        XCTAssertFalse(invalid)
        XCTAssertTrue(controller.customInvalid)
        XCTAssertEqual(changes.last, [.schedule("61 25 * * *")])
        XCTAssertEqual(controller.lastValidRows, [.schedule("@daily")])

        let closed = await controller.closeEditor()
        XCTAssertTrue(closed)
        XCTAssertEqual(closeCommits.last, [.schedule("@daily")])
        XCTAssertEqual(controller.focusReturnRow, 0)
        controller.clearFocusReturnRow()
        XCTAssertNil(controller.focusReturnRow)
    }

    @MainActor
    func testTriggerDraftControllerFencesPendingCompletionAfterDispose() async {
        let gate = AsyncStream<Void>.makeStream()
        var iterator = gate.stream.makeAsyncIterator()
        let controller = MobileBotRoutineTriggerDraftController(
            initialRows: [.schedule("@daily")],
            onDraftCommit: { _ in
                _ = await iterator.next()
            }
        )

        let task = Task {
            await controller.addRowAndCommit(.schedule("@weekly"))
        }
        await Task.yield()
        XCTAssertTrue(controller.pending)
        controller.dispose()
        gate.continuation.yield(())
        gate.continuation.finish()
        let result = await task.value

        XCTAssertFalse(result)
        XCTAssertFalse(controller.pending)
        XCTAssertEqual(controller.lastValidRows, [.schedule("@daily")])
    }

    @MainActor
    func testTriggerDraftControllerSingleRowRemovalKeepsLastValidDraftForRevert() async {
        var changes: [[RoutineTriggerForm]] = []
        let controller = MobileBotRoutineTriggerDraftController(
            initialRows: [.schedule("@daily")],
            onDraftChange: { changes.append($0) }
        )

        let committed = await controller.removeRow(0)
        XCTAssertFalse(committed)
        XCTAssertEqual(controller.rows, [])
        XCTAssertEqual(changes.last, [])
        XCTAssertEqual(controller.lastValidRows, [.schedule("@daily")])
    }


    func testRoutineUpsertCommandCarriesCanonicalGroupedTriggerWireShape() {
        let trigger = routineTriggerFromForms([
            .schedule("@daily"),
            .github(
                repo: "owner/repo",
                events: ["pr-opened", "ci-failed"],
                userAllowlist: "@Alice Bob",
                ciBranch: "main"
            ),
        ])
        XCTAssertNotNil(trigger)

        let spec = MobileBotRoutineSpec(
            name: "Review",
            prompt: "Summarize matching events.",
            schedule: "@daily",
            isEnabled: true,
            trigger: trigger
        )
        let command = MobileBotRoutinesModel.commandUpsert(
            agentId: "agent-1",
            id: "routine-1",
            spec: spec,
            requestId: "request-1"
        )

        XCTAssertEqual(command["type"] as? String, "automation.upsert")
        XCTAssertEqual(command["schedule"] as? String, "@daily")
        guard let wire = command["trigger"] as? [String: Any] else {
            return XCTFail("missing trigger wire")
        }
        XCTAssertEqual(wire["kind"] as? String, "group")
        guard let listeners = wire["listeners"] as? [[String: Any]] else {
            return XCTFail("missing group listeners")
        }
        XCTAssertEqual(listeners.count, 2)
        XCTAssertEqual(listeners[0]["kind"] as? String, "schedule")
        XCTAssertEqual(listeners[0]["schedule"] as? String, "@daily")
        XCTAssertEqual(listeners[1]["kind"] as? String, "event")
        XCTAssertEqual(listeners[1]["source"] as? String, "github")
        XCTAssertEqual(listeners[1]["event"] as? String, "*")
        guard let filters = listeners[1]["filters"] as? [String: Any] else {
            return XCTFail("missing structured filters")
        }
        XCTAssertEqual(filters["repo"] as? String, "owner/repo")
        XCTAssertEqual(filters["events"] as? [String], ["pr-opened", "ci-failed"])
        XCTAssertEqual(filters["ciBranch"] as? String, "main")
        XCTAssertEqual(filters["actorAllowlist"] as? [String], ["Alice", "Bob"])
    }

}

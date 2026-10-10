import XCTest
@testable import Fabushi

private struct FixedProductAnalyticsGate: ProductAnalyticsGate {
    let enabled: Bool
    let shouldThrow: Bool

    func checkGate(_ name: String) async throws -> Bool {
        XCTAssertEqual(name, SAND_PRODUCT_ANALYTICS_GATE)
        if shouldThrow {
            throw URLError(.cannotConnectToHost)
        }
        return enabled
    }
}

private actor ProductAnalyticsRecorder: ProductAnalyticsClient {
    private(set) var batches: [[ProductAnalyticsEvent]] = []
    var fail = false

    func trackEvents(_ events: [ProductAnalyticsEvent]) async throws {
        if fail { throw URLError(.networkConnectionLost) }
        batches.append(events)
    }

    func setFail(_ value: Bool) {
        fail = value
    }

    func allEvents() -> [ProductAnalyticsEvent] {
        batches.flatMap { $0 }
    }
}

final class SharedProductAnalyticsParityTests: XCTestCase {
    func testDeferredEventsActivateAndFlushWithBaseProps() async {
        let analytics = SandProductAnalytics(
            hostInBox: false,
            baseProps: ["sand_version": .string("1.2.3")]
        )
        let recorder = ProductAnalyticsRecorder()

        await analytics.trackEvent("before", props: ["feature": .string("chat")])
        let bufferedBefore = await analytics.bufferedEventCount()
        XCTAssertEqual(bufferedBefore, 1)

        await analytics.activate(
            gate: FixedProductAnalyticsGate(enabled: true, shouldThrow: false),
            client: recorder
        )
        let activeState = await analytics.state
        XCTAssertEqual(activeState, .active)
        await analytics.flush()

        let events = await recorder.allEvents()
        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events.first?.eventName, "before")
        XCTAssertEqual(events.first?.props["client"], .string("sand"))
        XCTAssertEqual(events.first?.props["os"], .string("ios"))
        XCTAssertEqual(events.first?.props["feature"], .string("chat"))
        let bufferedAfter = await analytics.bufferedEventCount()
        XCTAssertEqual(bufferedAfter, 0)
    }

    func testGateFailureLeavesDeferredBufferIntact() async {
        let analytics = SandProductAnalytics(hostInBox: false)
        let recorder = ProductAnalyticsRecorder()
        await analytics.trackEvent("queued")

        await analytics.activate(
            gate: FixedProductAnalyticsGate(enabled: false, shouldThrow: false),
            client: recorder
        )
        let deferredState = await analytics.state
        XCTAssertEqual(deferredState, .deferred)
        let bufferedDeferred = await analytics.bufferedEventCount()
        XCTAssertEqual(bufferedDeferred, 1)
    }

    func testDeferredBufferIsBounded() async {
        let analytics = SandProductAnalytics(hostInBox: true)
        for index in 0..<(MAX_DEFERRED_PRODUCT_ANALYTICS_EVENTS + 20) {
            await analytics.trackEvent("event", props: ["index": .int(index)])
        }
        let cappedCount = await analytics.bufferedEventCount()
        XCTAssertEqual(cappedCount, MAX_DEFERRED_PRODUCT_ANALYTICS_EVENTS)
    }

    func testMarkActiveDeduplicatesReasonPerUtcDay() async {
        let analytics = SandProductAnalytics(hostInBox: false)
        let day = Date(timeIntervalSince1970: 1_700_000_000)
        await analytics.markActive(reason: "foreground", now: day)
        await analytics.markActive(reason: "foreground", now: day.addingTimeInterval(60))
        await analytics.markActive(reason: "resume", now: day.addingTimeInterval(60))

        let events = await analytics.snapshotEvents()
        XCTAssertEqual(events.map(\.eventName), ["sand.app.active", "sand.app.active"])
        XCTAssertEqual(events.first?.props["reason"], .string("foreground"))
        XCTAssertEqual(events.last?.props["reason"], .string("resume"))
    }

    func testFailedFlushIsIsolatedAndRetainsPendingEvents() async {
        let analytics = SandProductAnalytics(hostInBox: false)
        let recorder = ProductAnalyticsRecorder()
        await analytics.activate(
            gate: FixedProductAnalyticsGate(enabled: true, shouldThrow: false),
            client: recorder
        )
        await analytics.trackEvent("retry-me")
        await recorder.setFail(true)
        await analytics.flush()
        let retained = await analytics.bufferedEventCount()
        XCTAssertEqual(retained, 1)

        await recorder.setFail(false)
        await analytics.flush()
        let drained = await analytics.bufferedEventCount()
        XCTAssertEqual(drained, 0)
    }

    func testOptOutStartsDisabledAndDropsEvents() async {
        let analytics = SandProductAnalytics(hostInBox: false, telemetryOptedOut: true)
        await analytics.trackEvent("drop-me")
        let disabledState = await analytics.state
        XCTAssertEqual(disabledState, .disabled)
        let canRecord = await analytics.canRecordEvents()
        XCTAssertFalse(canRecord)
        let disabledCount = await analytics.bufferedEventCount()
        XCTAssertEqual(disabledCount, 0)
    }
}

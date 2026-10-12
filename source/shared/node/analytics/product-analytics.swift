import Foundation

let SAND_PRODUCT_ANALYTICS_GATE = "sand_product_analytics"
let MAX_DEFERRED_PRODUCT_ANALYTICS_EVENTS = 256

enum ProductAnalyticsValue: Equatable, Sendable {
    case string(String)
    case bool(Bool)
    case int(Int)
    case double(Double)
}

struct ProductAnalyticsEvent: Equatable, Sendable {
    let eventName: String
    let props: [String: ProductAnalyticsValue]
    let timestamp: Date
}

protocol ProductAnalyticsClient: Sendable {
    func trackEvents(_ events: [ProductAnalyticsEvent]) async throws
}

protocol ProductAnalyticsGate: Sendable {
    func checkGate(_ name: String) async throws -> Bool
}

enum SandProductAnalyticsState: String, Equatable, Sendable {
    case disabled
    case deferred
    case active
}

actor SandProductAnalytics {
    private(set) var state: SandProductAnalyticsState
    private let baseProps: [String: ProductAnalyticsValue]
    private var deferredEvents: [ProductAnalyticsEvent] = []
    private var pendingEvents: [ProductAnalyticsEvent] = []
    private var lastActiveDayKeys: [String: String] = [:]
    private var client: (any ProductAnalyticsClient)?
    private var activated = false

    init(
        hostInBox: Bool,
        telemetryOptedOut: Bool = false,
        baseProps: [String: ProductAnalyticsValue] = [:]
    ) {
        var enriched = baseProps
        enriched["client"] = .string("sand")
        enriched["os"] = .string("ios")
        enriched["host_in_box"] = .bool(hostInBox)
        self.baseProps = enriched
        self.state = telemetryOptedOut ? .disabled : .deferred
    }

    func activate(
        gate: any ProductAnalyticsGate,
        client: any ProductAnalyticsClient
    ) async {
        guard !activated else { return }
        activated = true
        await evaluateGate(gate: gate, client: client)
    }

    func refreshGate(
        gate: any ProductAnalyticsGate,
        client: any ProductAnalyticsClient
    ) async {
        guard state == .deferred else { return }
        await evaluateGate(gate: gate, client: client)
    }

    func disable() {
        deferredEvents.removeAll()
        pendingEvents.removeAll()
        client = nil
        state = .disabled
    }

    func trackEvent(
        _ event: String,
        props: [String: ProductAnalyticsValue] = [:],
        timestamp: Date = Date()
    ) {
        guard state == .deferred || state == .active else { return }

        var enriched = baseProps
        for (key, value) in props {
            enriched[key] = value
        }
        let record = ProductAnalyticsEvent(eventName: event, props: enriched, timestamp: timestamp)

        switch state {
        case .deferred:
            guard deferredEvents.count < MAX_DEFERRED_PRODUCT_ANALYTICS_EVENTS else { return }
            deferredEvents.append(record)
        case .active:
            pendingEvents.append(record)
        case .disabled:
            break
        }
    }

    func markActive(
        reason: String,
        now: Date = Date(),
        calendar: Calendar = Calendar(identifier: .gregorian)
    ) {
        guard state == .deferred || state == .active else { return }

        var utc = calendar
        utc.timeZone = TimeZone(secondsFromGMT: 0) ?? .gmt
        let parts = utc.dateComponents([.year, .month, .day], from: now)
        let dayKey = String(format: "%04d-%02d-%02d", parts.year ?? 0, parts.month ?? 0, parts.day ?? 0)
        guard lastActiveDayKeys[reason] != dayKey else { return }
        lastActiveDayKeys[reason] = dayKey
        trackEvent("sand.app.active", props: ["reason": .string(reason)], timestamp: now)
    }

    func canRecordEvents() -> Bool {
        state == .deferred || state == .active
    }

    func bufferedEventCount() -> Int {
        deferredEvents.count + pendingEvents.count
    }

    func snapshotEvents() -> [ProductAnalyticsEvent] {
        deferredEvents + pendingEvents
    }

    func flush(timeoutMs: Int = 2_500) async {
        guard state == .active, let client, !pendingEvents.isEmpty else { return }
        let batch = pendingEvents
        do {
            try await client.trackEvents(batch)
            if pendingEvents.starts(with: batch) {
                pendingEvents.removeFirst(batch.count)
            }
        } catch {
            // Product analytics is non-critical. Delivery failures stay buffered
            // and must never fail the owning product flow.
        }
        _ = timeoutMs
    }

    func dispose() async {
        await flush()
        client = nil
    }

    private func evaluateGate(
        gate: any ProductAnalyticsGate,
        client: any ProductAnalyticsClient
    ) async {
        guard state == .deferred else { return }
        let enabled: Bool
        do {
            enabled = try await gate.checkGate(SAND_PRODUCT_ANALYTICS_GATE)
        } catch {
            return
        }
        guard enabled else { return }

        self.client = client
        pendingEvents.append(contentsOf: deferredEvents)
        deferredEvents.removeAll()
        state = .active
    }
}

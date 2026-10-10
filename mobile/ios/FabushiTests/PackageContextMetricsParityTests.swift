import XCTest
@testable import Fabushi

private final class RecordingPackageMetricsBackend: PackageMetricsBackend {
    struct Call: Equatable {
        let operation: String
        let name: String
        let value: Double
        let labels: PackageMetricLabels?
        let path: [String]
    }

    private(set) var calls: [Call] = []

    func record(context: PackageContext, handle: PackageMetricHandle, value: Double, labels: PackageMetricLabels?) {
        calls.append(.init(operation: "record", name: handle.name, value: value, labels: labels, path: context.getPath()))
    }

    func increment(context: PackageContext, handle: PackageMetricHandle, value: Double, labels: PackageMetricLabels?) {
        calls.append(.init(operation: "increment", name: handle.name, value: value, labels: labels, path: context.getPath()))
    }

    func gauge(context: PackageContext, handle: PackageMetricHandle, value: Double, labels: PackageMetricLabels?) {
        calls.append(.init(operation: "gauge", name: handle.name, value: value, labels: labels, path: context.getPath()))
    }

    func histogram(context: PackageContext, handle: PackageMetricHandle, value: Double, labels: PackageMetricLabels?) {
        calls.append(.init(operation: "histogram", name: handle.name, value: value, labels: labels, path: context.getPath()))
    }
}

private enum PackageContextTestError: LocalizedError {
    case failed

    var errorDescription: String? { "context failed" }
}

final class PackageContextMetricsParityTests: XCTestCase {
    func testContextKeysInheritOverrideAndPreserveNamedPath() {
        let key = PackageContextKey(defaultValue: "default")
        let root = PackageContext.root().withName("root")
        XCTAssertEqual(root.get(key), "default")

        let child = root.with(key, value: "child").withName("agent")
        XCTAssertEqual(child.get(key), "child")
        XCTAssertEqual(child.getPath(), ["root", "agent"])
        XCTAssertNotNil(child.getParent())
    }

    func testOptionalContextKeyInheritsAndCanExplicitlyShadowWithNil() {
        let key = PackageContextKey<String?>(defaultValue: "default")
        let parent = PackageContext.root().with(key, value: "parent")

        XCTAssertEqual(parent.withName("named").get(key), "parent")

        let cleared = parent
            .with(key, value: Optional<String>.none)
            .withName("named-cleared")
        XCTAssertNil(cleared.get(key))
    }

    func testCancellationPropagatesButDetachedContextStaysLive() {
        let root = PackageContext.root()
        let (child, cancel) = root.withCancel()
        let detached = child.withDetached()

        cancel("user stopped")
        XCTAssertTrue(child.cancelled)
        XCTAssertEqual(child.reason, "user stopped")
        XCTAssertFalse(detached.cancelled)
    }

    func testDeadlineAlreadyElapsedCancelsImmediately() {
        let expired = PackageContext.root().withDeadline(Date(timeIntervalSinceNow: -1))
        XCTAssertTrue(expired.cancelled)
        XCTAssertEqual(expired.reason, "context deadline exceeded")
    }

    func testAbortReasonProjectionMatchesReferenceCategories() {
        XCTAssertEqual(packageAbortReasonInfo(nil), .init(abortReasonType: "undefined"))
        XCTAssertEqual(
            packageAbortReasonInfo("stop"),
            .init(abortReasonType: "string", abortReasonMessage: "stop")
        )
        let error = packageAbortReasonInfo(PackageContextTestError.failed)
        XCTAssertEqual(error.abortReasonType, "error")
        XCTAssertEqual(error.abortReasonName, "PackageContextTestError")
        XCTAssertEqual(error.abortReasonMessage, "context failed")
        XCTAssertEqual(packageAbortReasonInfo(true).abortReasonType, "boolean")
        XCTAssertEqual(packageAbortReasonInfo(42).abortReasonType, "number")
    }

    func testMetricsResolveBackendFromContextAndPreserveHandles() {
        let backend = RecordingPackageMetricsBackend()
        let context = PackageContext.root()
            .with(packageMetricsKey, value: backend)
            .withName("send")

        let counter = packageCreateCounter(
            "agent.turns",
            description: "Accepted turns",
            labelNames: ["model"]
        )
        counter.increment(context, labels: ["model": "deepseek"])
        counter.record(context, value: 3)

        let gauge = packageCreateGauge("agent.queue")
        gauge.gauge(context, value: 2)

        let histogram = packageCreateHistogram("agent.first_token_ms")
        histogram.histogram(context, value: 125)

        XCTAssertEqual(counter.handle.type, .counter)
        XCTAssertEqual(counter.handle.description, "Accepted turns")
        XCTAssertEqual(counter.handle.labelNames, ["model"])
        XCTAssertEqual(
            backend.calls,
            [
                .init(operation: "increment", name: "agent.turns", value: 1, labels: ["model": "deepseek"], path: ["send"]),
                .init(operation: "record", name: "agent.turns", value: 3, labels: nil, path: ["send"]),
                .init(operation: "gauge", name: "agent.queue", value: 2, labels: nil, path: ["send"]),
                .init(operation: "histogram", name: "agent.first_token_ms", value: 125, labels: nil, path: ["send"]),
            ]
        )
    }
}

import XCTest
@testable import Fabushi

private final class RecordingPackageLoggerBackend: PackageContextLoggerBackend {
    private(set) var entries: [PackageContextLogEntry] = []

    func log(context _: PackageContext, entry: PackageContextLogEntry) {
        entries.append(entry)
    }
}

private final class RecordingPackageSpan: PackageSpanHandle {
    let contextData: PackageSpanContextData
    private(set) var attributes: [String: PackageSpanAttributeValue]
    private(set) var endCount = 0

    init(
        contextData: PackageSpanContextData,
        attributes: [String: PackageSpanAttributeValue] = [:]
    ) {
        self.contextData = contextData
        self.attributes = attributes
    }

    func setAttribute(_ key: String, value: PackageSpanAttributeValue) {
        attributes[key] = value
    }

    func end(at _: Date?) {
        endCount += 1
    }
}

private final class RecordingPackageTracerBackend: PackageTracerBackend {
    private(set) var spans: [RecordingPackageSpan] = []
    private var next = 1

    func startSpan(
        name _: String,
        parent: PackageSpanContextData?,
        attributes: [String: PackageSpanAttributeValue]
    ) -> any PackageSpanHandle {
        let traceId = parent?.traceId ?? String(format: "%032x", next)
        let spanId = String(format: "%016x", next)
        next += 1
        let span = RecordingPackageSpan(
            contextData: .init(
                traceId: traceId,
                spanId: spanId,
                traceFlags: parent?.traceFlags ?? 1,
                traceState: parent?.traceState
            ),
            attributes: attributes
        )
        spans.append(span)
        return span
    }
}

private enum PackageLoggingTestError: LocalizedError {
    case boom
    var errorDescription: String? { "boom detail" }
}

final class PackageContextObservabilityParityTests: XCTestCase {
    func testLoggerUsesContextBackendPathAndMergedAttributes() {
        let backend = RecordingPackageLoggerBackend()
        let root = PackageContext.root()
            .with(packageLoggerKey, value: backend)
            .withName("root")
        let child = packageWithLogAttributes(
            root.withName("agent"),
            attributes: [
                "account": .string("a1"),
                "shared": .string("outer"),
            ]
        )
        let logger = packageCreateLogger("agent")

        logger.info(
            child,
            "started",
            metadata: [
                "shared": .string("inner"),
                "attempt": .number(2),
            ]
        )
        logger.error(child, "failed", error: PackageLoggingTestError.boom)

        XCTAssertEqual(backend.entries.count, 2)
        XCTAssertEqual(backend.entries[0].level, .info)
        XCTAssertEqual(backend.entries[0].contextPath, ["root", "agent"])
        XCTAssertEqual(backend.entries[0].metadata["account"], .string("a1"))
        XCTAssertEqual(backend.entries[0].metadata["shared"], .string("inner"))
        XCTAssertEqual(backend.entries[1].errorName, "PackageLoggingTestError")
        XCTAssertEqual(backend.entries[1].errorMessage, "boom detail")
    }

    func testSpanContextInheritanceSuppressionAndRemoteCreation() throws {
        let tracer = RecordingPackageTracerBackend()
        var context = PackageContext.root()
            .with(packageTracerBackendKey, value: tracer)
            .withName("root")
        context = packageWithInheritableAttribute(
            context,
            key: "agent.id",
            value: .string("agent-1")
        )
        let root = packageWithSpan(context)

        let rootData = try XCTUnwrap(packageGetSpanContextData(root))
        XCTAssertTrue(rootData.isValid)
        XCTAssertEqual(tracer.spans.first?.attributes["agent.id"], .string("agent-1"))

        let child = packageWithSpan(root.withName("child"))
        let childData = try XCTUnwrap(packageGetSpanContextData(child))
        XCTAssertEqual(childData.traceId, rootData.traceId)
        XCTAssertNotEqual(childData.spanId, rootData.spanId)

        let suppressed = packageWithSuppressedChildSpans(child)
        let unchanged = packageWithSpan(suppressed.withName("suppressed"))
        let suppressedData = try XCTUnwrap(packageGetSpanContextData(unchanged))
        XCTAssertEqual(suppressedData.spanId, childData.spanId)
        XCTAssertEqual(suppressedData.traceFlags, 0)

        let remoteData = PackageSpanContextData(
            traceId: "0123456789abcdef0123456789abcdef",
            spanId: "0123456789abcdef",
            traceFlags: 1
        )
        let remote = packageCreateContextFromRemoteSpanContext(remoteData, name: "remote")
        XCTAssertEqual(packageGetSpanContextData(remote), remoteData)

        let remoteChild = packageCreateContextFromSpanContext(
            remoteData,
            name: "remote-child",
            existingContext: PackageContext.root().with(packageTracerBackendKey, value: tracer)
        )
        XCTAssertEqual(packageGetSpanContextData(remoteChild)?.traceId, remoteData.traceId)
        XCTAssertNotEqual(packageGetSpanContextData(remoteChild)?.spanId, remoteData.spanId)
    }

    func testCompletedSpanAndEventUseParentedSpanAndInheritedAttributes() {
        let tracer = RecordingPackageTracerBackend()
        var context = PackageContext.root()
            .with(packageTracerBackendKey, value: tracer)
            .withName("turn")
        context = packageWithInheritableAttribute(
            context,
            key: "model",
            value: .string("deepseek")
        )
        context = packageWithSpan(context)

        let completed = packageRecordCompletedSpanIfParented(context)
        XCTAssertNotNil(completed)

        packageReportEvent(
            context,
            name: "first-token",
            now: Date(timeIntervalSinceNow: 0.01)
        )
        XCTAssertGreaterThanOrEqual(tracer.spans.count, 3)
        XCTAssertEqual(tracer.spans.last?.attributes["model"], .string("deepseek"))
        XCTAssertEqual(tracer.spans.last?.endCount, 1)
    }
}

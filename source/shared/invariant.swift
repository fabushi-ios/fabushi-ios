import Foundation

struct SandInvariantReport: Equatable, Sendable {
    let name: String
    let frame: String?
}

struct SandInvariantViolation: LocalizedError, Equatable, Sendable {
    let message: String
    let callStack: [String]

    init(_ message: String, callStack: [String] = Thread.callStackSymbols) {
        self.message = message
        self.callStack = callStack
    }

    var errorDescription: String? { message }
    var name: String { "SandInvariantViolation" }
}

let STRIPPED_INVARIANT_MESSAGE =
    "Invariant violation (message stripped in packaged builds; the stack identifies the site)"

private final class SandInvariantReporterRegistry: @unchecked Sendable {
    private let lock = NSLock()
    private var reporter: ((SandInvariantReport) -> Void)?

    func install(_ newReporter: @escaping (SandInvariantReport) -> Void) throws -> () -> Void {
        lock.lock()
        defer { lock.unlock() }
        guard reporter == nil else {
            throw SandInvariantViolation(STRIPPED_INVARIANT_MESSAGE)
        }
        reporter = newReporter
        return { [weak self] in
            guard let self else { return }
            self.lock.lock()
            defer { self.lock.unlock() }
            self.reporter = nil
        }
    }

    func report(_ report: SandInvariantReport) {
        lock.lock()
        let current = reporter
        lock.unlock()
        current?(report)
    }
}

private let sandInvariantReporterRegistry = SandInvariantReporterRegistry()

func installInvariantReporter(
    _ reporter: @escaping (SandInvariantReport) -> Void
) throws -> () -> Void {
    try sandInvariantReporterRegistry.install(reporter)
}

func invariantMessagesStripped() -> Bool {
    true
}

func topApplicationFrame(_ violation: SandInvariantViolation) -> String? {
    violation.callStack
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .first { frame in
            !frame.isEmpty
                && !frame.contains("SandInvariantViolation")
                && !frame.contains("invariant(")
                && !frame.contains("installInvariantReporter")
        }
}

func invariant(
    _ condition: @autoclosure () -> Bool,
    _ message: @autoclosure () -> String = ""
) throws {
    guard condition() else {
        let violationMessage = invariantMessagesStripped()
            ? STRIPPED_INVARIANT_MESSAGE
            : message()
        let violation = SandInvariantViolation(violationMessage)
        sandInvariantReporterRegistry.report(.init(
            name: violation.name,
            frame: topApplicationFrame(violation)
        ))
        throw violation
    }
}

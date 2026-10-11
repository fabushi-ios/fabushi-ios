import Foundation

@MainActor
final class IOSCoordinatorRuntime {
    enum State: Equatable, Sendable {
        case stopped
        case running(generation: UInt64)
        case restarting(generation: UInt64)
        case disposed
    }

    private let main: IOSMainRuntime
    private var handle: IOSCoordinatorLaunchHandle?
    private var highestAcceptedGeneration: UInt64 = 0

    private(set) var state: State = .stopped

    init(main: IOSMainRuntime) {
        self.main = main
    }

    @discardableResult
    func start() throws -> IOSCoordinatorLaunchHandle {
        if let handle { return handle }
        let launched = try IOSCoordinatorLauncher.launch(main: main)
        highestAcceptedGeneration &+= 1
        main.coordinatorDidLaunchForDevControls()
        handle = launched
        state = .running(generation: highestAcceptedGeneration)
        return launched
    }

    func accepts(generation: UInt64) -> Bool {
        guard generation >= highestAcceptedGeneration else { return false }
        highestAcceptedGeneration = generation
        return true
    }

    func restart() throws {
        guard state != .disposed else { return }
        highestAcceptedGeneration &+= 1
        state = .restarting(generation: highestAcceptedGeneration)
        handle?.dispose()
        handle = nil
        do {
            handle = try IOSCoordinatorLauncher.launch(main: main)
            main.coordinatorDidLaunchForDevControls()
            state = .running(generation: highestAcceptedGeneration)
        } catch {
            state = .stopped
            throw error
        }
    }

    func dispose() {
        guard state != .disposed else { return }
        handle?.dispose()
        handle = nil
        state = .disposed
    }
}


enum IOSCoordinatorTransportFailure: Equatable, Sendable {
    case portClosed
    case protocolBreach
}

struct IOSCoordinatorTransportRelaunchPlan: Equatable, Sendable {
    let generation: UInt64
    let relaunchSequence: Int
    let exitClass: CoordinatorExitClass
    let uptimeMilliseconds: Int
    let delayMilliseconds: Int
}

@MainActor
final class IOSCoordinatorTransportRecoveryController {
    typealias NowMilliseconds = @MainActor () -> Int

    private let nowMilliseconds: NowMilliseconds
    private var launchedAtMilliseconds: Int
    private var fastExitAttempt = 0
    private(set) var generation: UInt64 = 1
    private(set) var relaunchSequence = 0
    private(set) var disposed = false

    init(
        nowMilliseconds: @escaping NowMilliseconds = {
            Int(ProcessInfo.processInfo.systemUptime * 1_000)
        }
    ) {
        self.nowMilliseconds = nowMilliseconds
        launchedAtMilliseconds = nowMilliseconds()
    }

    func unexpectedFailure(
        _ failure: IOSCoordinatorTransportFailure
    ) -> IOSCoordinatorTransportRelaunchPlan? {
        guard !disposed else { return nil }
        let now = nowMilliseconds()
        let uptime = max(0, now - launchedAtMilliseconds)
        relaunchSequence += 1
        fastExitAttempt = uptime < CoordinatorTelemetry.healthyUptimeMilliseconds
            ? fastExitAttempt + 1
            : 1
        generation &+= 1
        if generation == 0 { generation = 1 }
        return IOSCoordinatorTransportRelaunchPlan(
            generation: generation,
            relaunchSequence: relaunchSequence,
            exitClass: failure == .protocolBreach ? .breach : .other,
            uptimeMilliseconds: uptime,
            delayMilliseconds: CoordinatorTelemetry.relaunchDelayMilliseconds(
                attempt: max(0, fastExitAttempt - 1)
            )
        )
    }

    func beginIntentionalRestart() -> (generation: UInt64, relaunchSequence: Int)? {
        guard !disposed else { return nil }
        fastExitAttempt = 0
        relaunchSequence += 1
        generation &+= 1
        if generation == 0 { generation = 1 }
        return (generation, relaunchSequence)
    }

    func shouldRelaunch(generation: UInt64) -> Bool {
        !disposed && generation == self.generation
    }

    @discardableResult
    func didRelaunch(generation: UInt64) -> Bool {
        guard shouldRelaunch(generation: generation) else { return false }
        launchedAtMilliseconds = nowMilliseconds()
        return true
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        generation &+= 1
        if generation == 0 { generation = 1 }
    }
}

import Foundation

let PRE_PIN_BUFFER_CAP = 64

struct ExperimentDiagnostic: Equatable, Sendable {
    let kind: String
    var metadata: [String: String] = [:]
}

private final class ExperimentsDiagnosticsState: @unchecked Sendable {
    let lock = NSLock()
    var reporter: ((ExperimentDiagnostic) -> Void)?
    var buffered: [ExperimentDiagnostic] = []
}

private let EXPERIMENTS_DIAGNOSTICS_STATE = ExperimentsDiagnosticsState()

func pinExperimentsDiagnosticsReporter(
    _ reporter: ((ExperimentDiagnostic) -> Void)?
) {
    let backlog: [ExperimentDiagnostic]
    EXPERIMENTS_DIAGNOSTICS_STATE.lock.lock()
    EXPERIMENTS_DIAGNOSTICS_STATE.reporter = reporter
    backlog = EXPERIMENTS_DIAGNOSTICS_STATE.buffered
    EXPERIMENTS_DIAGNOSTICS_STATE.buffered.removeAll()
    EXPERIMENTS_DIAGNOSTICS_STATE.lock.unlock()
    guard let reporter else { return }
    for diagnostic in backlog { reporter(diagnostic) }
}

func reportExperimentsDiagnostic(_ diagnostic: ExperimentDiagnostic) {
    let reporter: ((ExperimentDiagnostic) -> Void)?
    EXPERIMENTS_DIAGNOSTICS_STATE.lock.lock()
    reporter = EXPERIMENTS_DIAGNOSTICS_STATE.reporter
    if reporter == nil, EXPERIMENTS_DIAGNOSTICS_STATE.buffered.count < PRE_PIN_BUFFER_CAP {
        EXPERIMENTS_DIAGNOSTICS_STATE.buffered.append(diagnostic)
    }
    EXPERIMENTS_DIAGNOSTICS_STATE.lock.unlock()
    reporter?(diagnostic)
}

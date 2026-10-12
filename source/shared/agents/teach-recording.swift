import Foundation

let SAND_TEACH_MAX_DURATION_MS = 10 * 60 * 1_000

struct TeachRecordingStatus: Equatable, Sendable {
    enum State: String, Equatable, Sendable {
        case idle
        case recording
    }

    let state: State
    let agentId: String?
    let startedAtMs: Int?
    let maxDurationMs: Int
    let capturePath: String?

    init(
        state: State,
        agentId: String?,
        startedAtMs: Int?,
        maxDurationMs: Int,
        capturePath: String? = nil
    ) {
        self.state = state
        self.agentId = agentId
        self.startedAtMs = startedAtMs
        self.maxDurationMs = maxDurationMs
        self.capturePath = capturePath
    }
}

let IDLE_TEACH_RECORDING_STATUS = TeachRecordingStatus(
    state: .idle,
    agentId: nil,
    startedAtMs: nil,
    maxDurationMs: SAND_TEACH_MAX_DURATION_MS
)

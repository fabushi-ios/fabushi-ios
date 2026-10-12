import Foundation

let GATEWAY_LOCAL_EXEC_REQUESTS_PATH = "/local-exec/requests"
let GATEWAY_LOCAL_EXEC_RESPONSES_PATH = "/local-exec/responses"
let SAND_NO_LOCAL_MACHINE_MESSAGE =
    "Your local machine isn't connected right now (the Fabushi desktop app must be open and online to run commands on it). Try again once it's reachable."

private let COMPUTER_UNAVAILABLE_SUFFIX =
    "is unavailable — it looks disconnected. Reconnect it (or focus the computer you want commands to run on) and try again."

let SAND_COMPUTER_UNAVAILABLE_MESSAGE = "Your computer \(COMPUTER_UNAVAILABLE_SUFFIX)"

func sandComputerUnavailableMessage(_ label: String? = nil) -> String {
    guard let label, !label.isEmpty else { return SAND_COMPUTER_UNAVAILABLE_MESSAGE }
    return "Your computer \"\(label)\" \(COMPUTER_UNAVAILABLE_SUFFIX)"
}

let SAND_LOCAL_EXEC_LIVENESS_WINDOW_MS = 30_000
let SAND_LOCAL_EXEC_RESPONSE_TIMEOUT_MS = 10_000
let SAND_LOCAL_EXEC_HEARTBEAT_INTERVAL_MS = 10_000
let SAND_LOCAL_EXEC_CONTROL_POST_TIMEOUT_MS = 10_000
let SAND_LOCAL_EXEC_DATA_POST_TIMEOUT_MS = 120_000
let DEFAULT_MAX_LOCAL_EXEC_FILE_BYTES = 100 * 1024 * 1024

func describeLocalExecBytes(_ bytes: Int) -> String {
    String(format: "%.1f MiB", Double(bytes) / (1024 * 1024))
}

func localExecFileTooLargeMessage(
    actualBytes: Int,
    maxBytes: Int
) -> String {
    "File is \(describeLocalExecBytes(actualBytes)), which exceeds Fabushi's " +
        "\(describeLocalExecBytes(maxBytes)) limit for reading or transferring a single file over local-exec. " +
        "Read a slice with offset/limit, or use a shell command (grep, head, tail) to extract just what you need."
}

func localExecUploadFrameTooLargeMessage(_ maxBytes: Int) -> String {
    "The upload exceeds Fabushi's \(describeLocalExecBytes(maxBytes)) limit for transferring a single file over local-exec " +
        "and was refused before being read into memory. Transfer a smaller file, or split it into parts."
}

func maxLocalExecUploadFrameBytes(_ maxFileBytes: Int) -> Int {
    Int(ceil(Double(maxFileBytes) * 4.0 / 3.0)) + 64 * 1024
}

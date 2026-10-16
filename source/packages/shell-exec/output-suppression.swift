import Foundation

struct ShellOutputSuppressionDefaults: Equatable, Sendable {
    let windowMs: Int
    let minimumThresholdCharsPerSecond: Int
    let thresholdCharsPerSecondWithoutPressure: Int
    let minChars: Int
}

let SHELL_OUTPUT_SUPPRESSION_DEFAULTS = ShellOutputSuppressionDefaults(
    windowMs: 60_000,
    minimumThresholdCharsPerSecond: 64 * 1024,
    thresholdCharsPerSecondWithoutPressure: 2 * 1024 * 1024,
    minChars: 256 * 1024
)

let SHELL_OUTPUT_SUPPRESSED_NOTICE = "\n[This shell is producing too much output to stream. The command will still run.]\n"

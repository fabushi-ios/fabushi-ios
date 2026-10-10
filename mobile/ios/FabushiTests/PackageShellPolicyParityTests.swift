import XCTest
@testable import Fabushi

final class PackageShellPolicyParityTests: XCTestCase {
    func testEventLoopPressureDefaultsMatchReference() {
        XCTAssertEqual(EventLoopPressureThresholdDefault.eventLoopDelayP95Ms, 50)
        XCTAssertEqual(EventLoopPressureThresholdDefault.eventLoopUtilization, 0.7)
        XCTAssertEqual(EventLoopPressureTrackerDefault.sampleIntervalMs, 250)
        XCTAssertEqual(EventLoopPressureTrackerDefault.eventLoopResolutionMs, 20)
    }

    func testKnownShellExecutorWireValuesAndEnvironmentOverridesMatchReference() {
        XCTAssertEqual(KnownShellExecutor.allCases.map(\.rawValue), ["zsh", "zsh-light", "bash", "powershell", "naive"])
        XCTAssertEqual(SHELL_ENV_OVERRIDES["TERM"], "dumb")
        XCTAssertEqual(SHELL_ENV_OVERRIDES["NO_COLOR"], "1")
        XCTAssertEqual(SHELL_ENV_OVERRIDES["FORCE_COLOR"], "0")
        XCTAssertEqual(SHELL_ENV_OVERRIDES["_ZO_DOCTOR"], "0")
    }

    func testOutputSuppressionPolicyMatchesReferenceWithoutExecutingProcesses() {
        XCTAssertEqual(SHELL_OUTPUT_SUPPRESSION_DEFAULTS.windowMs, 60_000)
        XCTAssertEqual(SHELL_OUTPUT_SUPPRESSION_DEFAULTS.minimumThresholdCharsPerSecond, 64 * 1024)
        XCTAssertEqual(SHELL_OUTPUT_SUPPRESSION_DEFAULTS.thresholdCharsPerSecondWithoutPressure, 2 * 1024 * 1024)
        XCTAssertEqual(SHELL_OUTPUT_SUPPRESSION_DEFAULTS.minChars, 256 * 1024)
        XCTAssertTrue(SHELL_OUTPUT_SUPPRESSED_NOTICE.contains("too much output to stream"))
    }

    func testNetworkPolicyDefaultsAndEnablementMatchReference() {
        XCTAssertEqual(
            ShellExecNetworkPolicyUtils.effective(nil),
            ShellExecNetworkPolicy(defaultAction: .deny)
        )
        XCTAssertFalse(ShellExecNetworkPolicyUtils.isNetworkEnabled(nil))
        XCTAssertFalse(
            ShellExecNetworkPolicyUtils.isNetworkEnabled(
                ShellExecNetworkPolicy(defaultAction: .deny)
            )
        )
        XCTAssertTrue(
            ShellExecNetworkPolicyUtils.isNetworkEnabled(
                ShellExecNetworkPolicy(defaultAction: .allow)
            )
        )
        XCTAssertTrue(
            ShellExecNetworkPolicyUtils.isNetworkEnabled(
                ShellExecNetworkPolicy(defaultAction: .deny, allow: ["api.example.com"])
            )
        )
        XCTAssertEqual(
            ShellExecNetworkPolicyUtils.networkAllowAllPolicy().defaultAction,
            .allow
        )
    }
}

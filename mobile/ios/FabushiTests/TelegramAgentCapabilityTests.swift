//
//  TelegramAgentCapabilityTests.swift
//  FabushiTests
//
//  Comprehensive test suite covering Features 30, 31, 32, and 33:
//  - Feature 30: WebRTC Audio/Video Call Screen
//  - Feature 31: AI Multi-Agent ACP Runner UI
//  - Feature 32: 7-State Global Task Center
//  - Feature 33: MCP Tool Dock & Approval Gate
//

#if canImport(XCTest)
import XCTest
#endif
#if canImport(Fabushi)
@testable import Fabushi
#endif
import SwiftUI

private final class TestSendableBox<T>: @unchecked Sendable {
    var value: T
    init(_ value: T) { self.value = value }
}

@MainActor
public final class TelegramAgentCapabilityTests: XCTestCase, @unchecked Sendable {

    // =========================================================================
    // MARK: - FEATURE 30: WebRTC Audio/Video Call Screen
    // =========================================================================

    public func testTC_T1_F30_01_CallCanvasAndPulseRings() {
        let baseRadius: CGFloat = 70.0

        // Zero energy -> rings at base
        let radii0 = CallAudioPulseCalculator.calculateRingRadii(baseRadius: baseRadius, energy: 0.0)
        XCTAssertEqual(radii0.count, 3)
        XCTAssertEqual(radii0[0], baseRadius, accuracy: 0.001)

        // Half energy (0.5)
        let radiiHalf = CallAudioPulseCalculator.calculateRingRadii(baseRadius: baseRadius, energy: 0.5)
        XCTAssertTrue(radiiHalf[0] > baseRadius)
        XCTAssertTrue(radiiHalf[1] > radiiHalf[0])
        XCTAssertTrue(radiiHalf[2] > radiiHalf[1])

        // Full energy (1.0)
        let radiiFull = CallAudioPulseCalculator.calculateRingRadii(baseRadius: baseRadius, energy: 1.0)
        XCTAssertEqual(radiiFull[0], baseRadius * 1.28, accuracy: 0.001)
        XCTAssertEqual(radiiFull[1], baseRadius * 1.58, accuracy: 0.001)
        XCTAssertEqual(radiiFull[2], baseRadius * 1.95, accuracy: 0.001)

        // Ring opacities proportional to energy
        let opacities = CallAudioPulseCalculator.calculateRingOpacities(energy: 1.0)
        XCTAssertEqual(opacities[0], 0.55, accuracy: 0.001)
        XCTAssertEqual(opacities[1], 0.35, accuracy: 0.001)
        XCTAssertEqual(opacities[2], 0.18, accuracy: 0.001)
    }

    public func testTC_T1_F30_02_MuteAndSpeakerRouteToggle() {
        let vm = CallViewModel(peerName: "Alice")
        XCTAssertFalse(vm.isMuted)
        XCTAssertFalse(vm.isSpeakerOn)

        vm.toggleMute()
        XCTAssertTrue(vm.isMuted)

        vm.toggleSpeaker()
        XCTAssertTrue(vm.isSpeakerOn)

        vm.flipCamera()
        XCTAssertFalse(vm.isCameraFront)
    }

    public func testTC_T2_F30_01_PacketLossReconnectingBadge() {
        let vm = CallViewModel(peerName: "Bob")
        vm.transitionToConnected()

        if case .connected = vm.state {
            // Connected successfully
        } else {
            XCTFail("State should be connected")
        }

        // Simulate 10-second UDP packet loss
        vm.handlePacketLoss()
        XCTAssertEqual(vm.state, .reconnecting)
        XCTAssertEqual(vm.state.title, "Reconnecting...")

        // Packets recover
        vm.handlePacketRecovery()
        if case .connected = vm.state {
            // Recovered successfully
        } else {
            XCTFail("State should have recovered to connected")
        }
    }

    // =========================================================================
    // MARK: - FEATURE 31: AI Multi-Agent ACP Runner UI
    // =========================================================================

    public func testTC_T1_F31_02_ThinkingTokenStreaming() {
        let vm = AgentCardViewModel(agentName: "CodeAssistant")
        XCTAssertEqual(vm.phase, .idle)

        vm.appendThinkingToken("Analyzing query...", elapsedMs: 120)
        if case .thinking(let tokens, let ms) = vm.phase {
            XCTAssertEqual(tokens, "Analyzing query...")
            XCTAssertEqual(ms, 120)
        } else {
            XCTFail("Expected thinking phase")
        }

        vm.appendThinkingToken(" Fetching tools...", elapsedMs: 250)
        if case .thinking(let tokens, let ms) = vm.phase {
            XCTAssertEqual(tokens, "Analyzing query... Fetching tools...")
            XCTAssertEqual(ms, 250)
        } else {
            XCTFail("Expected thinking phase with accumulated tokens")
        }
    }

    public func testTC_T1_F31_05_StopGenerationAbortSignal() {
        let vm = AgentCardViewModel(agentName: "Agent")
        let stopped = TestSendableBox(false)
        vm.onStop = { stopped.value = true }

        vm.appendThinkingToken("Thinking", elapsedMs: 100)
        XCTAssertTrue(vm.phase.isGenerating)

        vm.abortTurn()
        XCTAssertEqual(vm.phase, .aborted)
        XCTAssertFalse(vm.phase.isGenerating)
        XCTAssertTrue(stopped.value)
    }

    public func testTC_T2_F31_02_MalformedToolCallJson() {
        // Valid JSON
        let validJson = "{\"command\": \"read_file\", \"path\": \"/tmp/test.txt\"}"
        let validResult = ToolCallPayloadParser.parse(rawJson: validJson)
        if case .success(let pretty) = validResult {
            XCTAssertTrue(pretty.contains("read_file"))
        } else {
            XCTFail("Expected success parse result")
        }

        // Malformed JSON (syntax error)
        let malformedJson = "{\"command\": \"read_file\", missing_brace:"
        let invalidResult = ToolCallPayloadParser.parse(rawJson: malformedJson)
        if case .syntaxError(let raw, let err) = invalidResult {
            XCTAssertEqual(raw, malformedJson)
            XCTAssertFalse(err.isEmpty)
        } else {
            XCTFail("Expected syntax error card for malformed JSON")
        }
    }

    // =========================================================================
    // MARK: - FEATURE 32: 7-State Global Task Center
    // =========================================================================

    public func testTC_T1_F32_02_TaskFsmStateTransitions() throws {
        let task = AgentTaskModel(id: "task_1", title: "Data ETL Pipeline")
        let vm = TaskCenterViewModel(tasks: [task])

        // pending -> assigned
        try vm.transitionTask(id: "task_1", to: .assigned)
        XCTAssertEqual(vm.tasks.first?.state, .assigned)

        // assigned -> running
        try vm.transitionTask(id: "task_1", to: .running)
        XCTAssertEqual(vm.tasks.first?.state, .running)

        // running -> waitingAuth
        try vm.transitionTask(id: "task_1", to: .waitingAuth)
        XCTAssertEqual(vm.tasks.first?.state, .waitingAuth)

        // waitingAuth -> running
        try vm.transitionTask(id: "task_1", to: .running)
        XCTAssertEqual(vm.tasks.first?.state, .running)

        // running -> completed (terminal)
        try vm.transitionTask(id: "task_1", to: .completed)
        XCTAssertEqual(vm.tasks.first?.state, .completed)
        XCTAssertTrue(vm.tasks.first?.state.isTerminal == true)
    }

    public func testTC_T1_F32_04_FiveFieldCronHumanizer() {
        let expr1 = "0 9 * * 1-5"
        XCTAssertEqual(CronRoutineParser.humanize(expression: expr1), "Weekdays at 9:00 AM")

        let expr2 = "0 9 * * *"
        XCTAssertEqual(CronRoutineParser.humanize(expression: expr2), "Every day at 9:00 AM")

        let expr3 = "0 0 * * *"
        XCTAssertEqual(CronRoutineParser.humanize(expression: expr3), "Every day at midnight")

        let expr4 = "*/15 * * * *"
        XCTAssertEqual(CronRoutineParser.humanize(expression: expr4), "Every 15 minutes")
    }

    public func testTC_T2_F32_01_SevenFieldCronRejection() {
        // 7 fields: rejected
        let sevenFields = "* * * * * * *"
        XCTAssertThrowsError(try CronRoutineParser.validate(expression: sevenFields)) { err in
            guard case CronError.invalidFormat = err else {
                XCTFail("Expected invalidFormat error, got \(err)")
                return
            }
        }

        // 4 fields: rejected
        let fourFields = "* * * *"
        XCTAssertThrowsError(try CronRoutineParser.validate(expression: fourFields))

        // 5 fields valid: accepted
        XCTAssertNoThrow(try CronRoutineParser.validate(expression: "0 9 * * 1-5"))
    }

    public func testTC_T2_F32_04_TerminalStateTransitionRejection() throws {
        let task = AgentTaskModel(id: "task_term", title: "Finished Task", state: .completed)
        let vm = TaskCenterViewModel(tasks: [task])

        // Attempting to move from completed back to running must throw
        XCTAssertThrowsError(try vm.transitionTask(id: "task_term", to: .running)) { err in
            guard case TaskFsmError.illegalStateTransition = err else {
                XCTFail("Expected illegalStateTransition, got \(err)")
                return
            }
        }
    }

    // =========================================================================
    // MARK: - FEATURE 33: MCP Tool Dock & Approval Gate
    // =========================================================================

    public func testTC_T1_F33_01_ApprovalGate10MinuteCountdown() {
        let req = McpApprovalRequest(id: "appr_1", toolName: "deploy", arguments: "{}", ttlSeconds: 600)
        let vm = McpApprovalViewModel(requests: [req])

        XCTAssertEqual(vm.remainingSeconds, 600)
        XCTAssertEqual(vm.formattedTime, "10:00")
        XCTAssertEqual(req.riskLevel, .low)
    }

    public func testTC_T1_F33_03_CriticalCommandScreening() {
        // Critical destructive commands
        let rmCommand = "rm -rf /var/data"
        let level1 = ToolRiskEvaluator.evaluate(toolName: "bash", command: rmCommand)
        XCTAssertEqual(level1, .critical)

        // Obfuscated with escaped spaces
        let rmObfuscated = "rm\\ -rf /"
        let level2 = ToolRiskEvaluator.evaluate(toolName: "bash", command: rmObfuscated)
        XCTAssertEqual(level2, .critical)

        // Eval base64
        let evalCommand = "eval(base64_decode('...'))"
        let level3 = ToolRiskEvaluator.evaluate(toolName: "evaluator", command: evalCommand)
        XCTAssertEqual(level3, .critical)

        // Drop table
        let sqlDrop = "DROP TABLE users;"
        let level4 = ToolRiskEvaluator.evaluate(toolName: "sqlite", command: sqlDrop)
        XCTAssertEqual(level4, .critical)

        // Read-only
        let readCmd = "query: select * from messages"
        let level5 = ToolRiskEvaluator.evaluate(toolName: "read_query", command: readCmd)
        XCTAssertEqual(level5, .readOnly)
    }

    public func testTC_T2_F33_01_TimerZeroAutoRejection() {
        let req = McpApprovalRequest(id: "appr_fast", toolName: "write", arguments: "{}", ttlSeconds: 1)
        let vm = McpApprovalViewModel(requests: [req])
        let resolvedDecision = TestSendableBox<ApprovalDecision?>(nil)

        vm.onDecisionResolved = { _, decision in
            resolvedDecision.value = decision
        }

        // Simulate timeout trigger
        vm.handleAppBackgrounded()
        // Simulate waiting past expiry
        vm.remainingSeconds = 0
        vm.handleAppForegrounded()

        XCTAssertEqual(vm.lastDecision, .timeout)
        XCTAssertEqual(resolvedDecision.value, .timeout)
    }

    public func testTC_T2_F33_04_BackgroundWallClockExpiry() {
        let req = McpApprovalRequest(id: "appr_bg", toolName: "format", arguments: "{}", ttlSeconds: 300)
        let vm = McpApprovalViewModel(requests: [req])

        vm.handleAppBackgrounded()
        // Simulate app suspended for 350 seconds
        vm.remainingSeconds = 0 // represents time expired
        vm.handleAppForegrounded()

        XCTAssertEqual(vm.lastDecision, .timeout)
    }
}

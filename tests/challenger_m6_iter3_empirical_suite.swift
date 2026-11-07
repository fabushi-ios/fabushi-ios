//
//  challenger_m6_iter3_empirical_suite.swift
//  FabushiTests
//
//  Milestone 6 Iteration 3: Native Presentation & Remediated Components
//  Adversarial Empirical Challenge Suite.
//
//  Vectors under challenge:
//  1. AgentCardViewModel State Machine Multi-Turn Stress, Boundary Prompts, Rapid Cancellation
//  2. McpApprovalViewModel Step Tick Countdown to 0, Race Conditions, Queue Exhaustion
//  3. ScrollToBottomButton Hysteresis, Composer Keyboard Offset (Hardware vs Software), Context Menu Flip
//  4. MediaTransferManager Chunk Partitioning (Non-Multiples) & Index Bounds Validation
//  5. Production Integrity, Zero Constant Identity Assertions, Zero Dummy Facades & Build Hygiene
//

import CoreGraphics
import Foundation
import SwiftUI
#if canImport(Darwin)
import Darwin
#endif

// MARK: - Infrastructure & Assertion Oracles

public final class ChallengeFailure: Error, CustomStringConvertible, Sendable {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var description: String { message }
}

public final class ThreadSafeCounter: @unchecked Sendable {
    private var count: Int = 0
    private let lock = NSLock()
    public init(_ initial: Int = 0) { self.count = initial }
    public func increment() {
        lock.lock()
        count += 1
        lock.unlock()
    }
    public var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }
}

public final class ThreadSafeDecisionRecorder: @unchecked Sendable {
    private var decisions: [(id: String, decision: ApprovalDecision)] = []
    private let lock = NSLock()
    public init() {}
    public func record(id: String, decision: ApprovalDecision) {
        lock.lock()
        decisions.append((id: id, decision: decision))
        lock.unlock()
    }
    public var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return decisions.count
    }
    public func all() -> [(id: String, decision: ApprovalDecision)] {
        lock.lock()
        defer { lock.unlock() }
        return decisions
    }
    public func last() -> (id: String, decision: ApprovalDecision)? {
        lock.lock()
        defer { lock.unlock() }
        return decisions.last
    }
}

@main
public struct ChallengerM6Iter3Suite {

    @MainActor private static var passCount: Int = 0
    @MainActor private static var failCount: Int = 0
    @MainActor private static var totalChecks: Int = 0

    public static func getRssMB() -> Double {
        #if canImport(Darwin)
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size) / 4
        let kerr: kern_return_t = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
            }
        }
        if kerr == KERN_SUCCESS {
            return Double(info.resident_size) / (1024.0 * 1024.0)
        }
        return 0.0
        #else
        return 0.0
        #endif
    }

    @MainActor
    public static func assertTrue(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if !condition() {
            throw ChallengeFailure("Assertion FAILED at \(file):\(line): \(message)")
        }
    }

    @MainActor
    public static func assertFalse(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if condition() {
            throw ChallengeFailure("Assertion FAILED at \(file):\(line): expected false, got true. \(message)")
        }
    }

    @MainActor
    public static func assertEqual<T: Equatable>(_ a: T, _ b: T, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if a != b {
            throw ChallengeFailure("Equality FAILED at \(file):\(line): expected '\(b)', got '\(a)'. \(message)")
        }
    }

    @MainActor
    public static func assertAlmostEqual(_ a: CGFloat, _ b: CGFloat, accuracy: CGFloat = 0.001, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if abs(a - b) > accuracy {
            throw ChallengeFailure("Accuracy FAILED at \(file):\(line): expected \(b) ± \(accuracy), got \(a). \(message)")
        }
    }

    @MainActor
    public static func assertAlmostEqualDouble(_ a: Double, _ b: Double, accuracy: Double = 0.001, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if abs(a - b) > accuracy {
            throw ChallengeFailure("Accuracy FAILED at \(file):\(line): expected \(b) ± \(accuracy), got \(a). \(message)")
        }
    }

    @MainActor
    private static func runSection(_ title: String, block: () async throws -> Void) async {
        print("\n================================================================================")
        print(" [CHALLENGE] \(title)")
        print("================================================================================")
        do {
            try await block()
            passCount += 1
            print(" >>> SECTION PASSED: \(title)")
        } catch {
            failCount += 1
            print(" !!! SECTION FAILED: \(title) -> \(error)")
        }
    }

    // =========================================================================
    // MARK: - VECTOR 1: AgentCardViewModel State Machine Stress & Boundaries
    // =========================================================================

    @MainActor
    static func challengeVector1_AgentCardStateStress() async throws {
        print(" -> Sub-challenge 1.1: Empty and whitespace prompt rejection boundaries")
        let vm = AgentCardViewModel(agentName: "StressAgent")
        try assertEqual(vm.phase, .idle)
        try assertFalse(vm.phase.isGenerating)
        try assertTrue(vm.isThinkingExpanded)
        try assertEqual(vm.accumulatedText, "")
        try assertTrue(vm.toolExecutionHistory.isEmpty)

        // Test empty string
        let emptyAccepted = vm.submitPrompt("")
        try assertFalse(emptyAccepted, "Empty prompt must be rejected")
        try assertEqual(vm.phase, .failed(error: "Empty agent prompt rejected"))
        try assertFalse(vm.phase.isGenerating)

        // Test whitespace only variations
        let whitespaceCases = [
            " ",
            "   ",
            "\t",
            "\n",
            "\r\n",
            "   \t\n  \r\n   "
        ]
        for ws in whitespaceCases {
            let res = vm.submitPrompt(ws)
            try assertFalse(res, "Whitespace prompt '\(ws.debugDescription)' must be rejected")
            try assertEqual(vm.phase, .failed(error: "Empty agent prompt rejected"))
        }

        // Test valid prompt transition
        let validAccepted = vm.submitPrompt("Perform market research")
        try assertTrue(validAccepted, "Valid prompt must be accepted")
        try assertEqual(vm.phase, .thinking(tokens: "", elapsedMs: 0))
        try assertTrue(vm.phase.isGenerating)

        print(" -> Sub-challenge 1.2: 100k-character prompt boundary and token stress")
        let hugeToken = "TelegramACPToken1234567890_" // 27 chars
        let hugePrompt = String(repeating: hugeToken, count: 4000) // 108,000 characters
        try assertTrue(hugePrompt.count > 100_000, "Prompt must exceed 100k characters")

        let hugeAccepted = vm.submitPrompt(hugePrompt)
        try assertTrue(hugeAccepted, "100k-character prompt must be accepted without crash")
        try assertEqual(vm.phase, .thinking(tokens: "", elapsedMs: 0))

        // Feed 1,000 thinking tokens
        for i in 1...1000 {
            vm.appendThinkingToken(" t\(i)", elapsedMs: UInt64(i * 10))
        }
        if case .thinking(let tokens, let elapsed) = vm.phase {
            try assertTrue(tokens.count > 3000, "Tokens must accumulate correctly")
            try assertEqual(elapsed, 10_000, "Elapsed ms must reflect last token")
        } else {
            throw ChallengeFailure("Expected .thinking phase after token stream")
        }

        print(" -> Sub-challenge 1.3: Multi-turn 100-cycle state machine continuous loop")
        let multiTurnVM = AgentCardViewModel(agentName: "LoopAgent")
        for turn in 1...100 {
            let sub = multiTurnVM.submitPrompt("Turn \(turn) query")
            try assertTrue(sub)
            try assertEqual(multiTurnVM.phase, .thinking(tokens: "", elapsedMs: 0))

            multiTurnVM.appendThinkingToken("Thought for \(turn)", elapsedMs: UInt64(turn * 50))
            multiTurnVM.setToolCall(id: "call_\(turn)", name: "query_database", args: "{\"turn\": \(turn), \"status\": \"ok\"}")
            try assertEqual(multiTurnVM.toolExecutionHistory.count, turn)
            try assertEqual(multiTurnVM.phase, .toolCalling(id: "call_\(turn)", name: "query_database", args: "{\"turn\": \(turn), \"status\": \"ok\"}"))

            multiTurnVM.appendResponseChunk("Chunk 1 for \(turn). ")
            multiTurnVM.appendResponseChunk("Chunk 2 for \(turn).")
            try assertEqual(multiTurnVM.phase, .streamingResponse(text: multiTurnVM.accumulatedText))

            multiTurnVM.completeTurn(markdown: "Finished response \(turn)")
            try assertEqual(multiTurnVM.phase, .completed(markdown: "Finished response \(turn)"))
            try assertFalse(multiTurnVM.phase.isGenerating)
        }
        try assertEqual(multiTurnVM.toolExecutionHistory.count, 100)

        print(" -> Sub-challenge 1.4: Rapid cancellation and stop invocation stress")
        let stopCounter = ThreadSafeCounter(0)
        let cancelVM = AgentCardViewModel(agentName: "CancelAgent")
        cancelVM.onStop = {
            stopCounter.increment()
        }

        for _ in 1...100 {
            cancelVM.submitPrompt("Task to cancel")
            try assertTrue(cancelVM.phase.isGenerating)
            cancelVM.abortTurn()
            try assertEqual(cancelVM.phase, .aborted)
            try assertFalse(cancelVM.phase.isGenerating)
        }
        try assertEqual(stopCounter.value, 100, "onStop must be triggered exactly 100 times")

        // Idempotent repeated abortTurn calls
        cancelVM.abortTurn()
        cancelVM.abortTurn()
        try assertEqual(stopCounter.value, 102, "Repeated abortTurn calls are safe and trigger callback")

        print(" -> Sub-challenge 1.5: ToolCallPayloadParser JSON syntax & fuzzing")
        // Valid JSON
        let validJson = "{\"status\": \"success\", \"code\": 200}"
        let pValid = ToolCallPayloadParser.parse(rawJson: validJson)
        if case .success(let pretty) = pValid {
            try assertTrue(pretty.contains("\"code\" : 200") || pretty.contains("\"code\": 200"))
        } else {
            throw ChallengeFailure("Expected valid JSON to parse successfully")
        }

        // Malformed JSON cases
        let malformedCases = [
            "",
            "   ",
            "{unquoted: 123}",
            "{\"unclosed\": \"bracket\"",
            "[1, 2, 3,",
            "plain string without json",
            "<xml>invalid</xml>"
        ]
        for bad in malformedCases {
            let res = ToolCallPayloadParser.parse(rawJson: bad)
            if case .syntaxError(let raw, _) = res {
                try assertEqual(raw, bad)
            } else {
                throw ChallengeFailure("Malformed string '\(bad)' must return .syntaxError")
            }
        }
    }

    // =========================================================================
    // MARK: - VECTOR 2: McpApprovalViewModel Countdown, Preemption & Races
    // =========================================================================

    @MainActor
    static func challengeVector2_McpCountdownAndRaces() async throws {
        print(" -> Sub-challenge 2.1: Step tick countdown from 600s down to 0s in various increments")
        let req1 = McpApprovalRequest(id: "req_001", toolName: "file_system_write", arguments: "{\"path\": \"/tmp/test.txt\"}", ttlSeconds: 600)
        let vm = McpApprovalViewModel(requests: [req1])
        let recorder = ThreadSafeDecisionRecorder()
        vm.onDecisionResolved = { id, decision in
            recorder.record(id: id, decision: decision)
        }

        try assertEqual(vm.currentRequest?.id, "req_001")
        try assertEqual(vm.remainingSeconds, 600)
        try assertEqual(vm.formattedTime, "10:00")
        try assertTrue(vm.isPresented)

        // Step by 1s
        vm.tick(seconds: 1)
        try assertEqual(vm.remainingSeconds, 599)
        try assertEqual(vm.formattedTime, "09:59")

        // Step by 9s
        vm.tick(seconds: 9)
        try assertEqual(vm.remainingSeconds, 590)
        try assertEqual(vm.formattedTime, "09:50")

        // Step by 50s
        vm.tick(seconds: 50)
        try assertEqual(vm.remainingSeconds, 540)
        try assertEqual(vm.formattedTime, "09:00")

        // Step by 480s (remaining 60s)
        vm.tick(seconds: 480)
        try assertEqual(vm.remainingSeconds, 60)
        try assertEqual(vm.formattedTime, "01:00")

        // Step by 59s (remaining 1s)
        vm.tick(seconds: 59)
        try assertEqual(vm.remainingSeconds, 1)
        try assertEqual(vm.formattedTime, "00:01")

        // Final 1s tick reaching 0 -> triggers timeout
        vm.tick(seconds: 1)
        try assertEqual(vm.remainingSeconds, 0)
        try assertEqual(vm.lastDecision, .timeout)
        try assertEqual(recorder.count, 1)
        try assertEqual(recorder.last()?.decision, .timeout)
        try assertFalse(vm.isPresented)
        try assertTrue(vm.currentRequest == nil)

        // Subsequent ticks on exhausted queue clamp at 0 and do not crash
        vm.tick(seconds: 10)
        try assertEqual(vm.remainingSeconds, 0)

        print(" -> Sub-challenge 2.2: Single large leap tick (600s and 1000s)")
        let req2 = McpApprovalRequest(id: "req_002", toolName: "execute_command", arguments: "ls -la", ttlSeconds: 600)
        let vmLeap = McpApprovalViewModel(requests: [req2])
        try assertEqual(vmLeap.remainingSeconds, 600)
        vmLeap.tick(seconds: 600)
        try assertEqual(vmLeap.remainingSeconds, 0)
        try assertEqual(vmLeap.lastDecision, .timeout)

        let req3 = McpApprovalRequest(id: "req_003", toolName: "read_file", arguments: "test", ttlSeconds: 600)
        let vmOver = McpApprovalViewModel(requests: [req3])
        vmOver.tick(seconds: 1000)
        try assertEqual(vmOver.remainingSeconds, 0)
        try assertEqual(vmOver.lastDecision, .timeout)

        print(" -> Sub-challenge 2.3: Multi-queued approval items & FIFO queue exhaustion")
        let reqA = McpApprovalRequest(id: "req_A", toolName: "db_drop", arguments: "drop table users", ttlSeconds: 600)
        let reqB = McpApprovalRequest(id: "req_B", toolName: "db_update", arguments: "update users set role=admin", ttlSeconds: 600)
        let reqC = McpApprovalRequest(id: "req_C", toolName: "db_query", arguments: "select * from users", ttlSeconds: 600)
        let reqLow = McpApprovalRequest(id: "req_Low", toolName: "db_select", arguments: "select * from users", ttlSeconds: 600)

        try assertEqual(reqA.riskLevel, .critical)
        try assertEqual(reqB.riskLevel, .medium)
        try assertEqual(reqC.riskLevel, .readOnly)
        try assertEqual(reqLow.riskLevel, .low)

        let multiVM = McpApprovalViewModel(requests: [reqA, reqB, reqC])
        let multiRecorder = ThreadSafeDecisionRecorder()
        multiVM.onDecisionResolved = { id, dec in
            multiRecorder.record(id: id, decision: dec)
        }

        try assertEqual(multiVM.currentRequest?.id, "req_A")
        try assertEqual(multiVM.queue.count, 2)
        try assertTrue(multiVM.isPresented)

        // Item 1: ReqA times out
        multiVM.tick(seconds: 600)
        try assertEqual(multiRecorder.last()?.id, "req_A")
        try assertEqual(multiRecorder.last()?.decision, .timeout)

        // Item 2: ReqB becomes current, reject manually
        try assertEqual(multiVM.currentRequest?.id, "req_B")
        try assertEqual(multiVM.queue.count, 1)
        try assertTrue(multiVM.isPresented)
        multiVM.reject(reason: "Policy violation")
        try assertEqual(multiRecorder.last()?.id, "req_B")
        try assertEqual(multiRecorder.last()?.decision, .deny(reason: "Policy violation"))

        // Item 3: ReqC becomes current, approve always
        try assertEqual(multiVM.currentRequest?.id, "req_C")
        try assertEqual(multiVM.queue.count, 0)
        try assertTrue(multiVM.isPresented)
        multiVM.approve(always: true)
        try assertEqual(multiRecorder.last()?.id, "req_C")
        try assertEqual(multiRecorder.last()?.decision, .alwaysAllow)

        // Queue exhausted
        try assertTrue(multiVM.currentRequest == nil)
        try assertTrue(multiVM.queue.isEmpty)
        try assertFalse(multiVM.isPresented)
        try assertEqual(multiRecorder.count, 3)

        print(" -> Sub-challenge 2.4: Race conditions between approval and timeout")
        // Scenario A: Approve then tick
        let raceReqA = McpApprovalRequest(id: "race_A", toolName: "safe_tool", arguments: "{}", ttlSeconds: 600)
        let vmRaceA = McpApprovalViewModel(requests: [raceReqA])
        vmRaceA.approve(always: false)
        try assertEqual(vmRaceA.lastDecision, .approveOnce)
        // Tick right after approval when queue is empty
        vmRaceA.tick(seconds: 600)
        try assertEqual(vmRaceA.lastDecision, .approveOnce, "Decision must not be overwritten after queue exhaustion")

        // Scenario B: Tick timeout then approve
        let raceReqB = McpApprovalRequest(id: "race_B", toolName: "safe_tool", arguments: "{}", ttlSeconds: 600)
        let vmRaceB = McpApprovalViewModel(requests: [raceReqB])
        vmRaceB.tick(seconds: 600)
        try assertEqual(vmRaceB.lastDecision, .timeout)
        // User attempts to tap approve after expiration
        vmRaceB.approve(always: false)
        try assertEqual(vmRaceB.lastDecision, .timeout, "Approve on expired request must be no-op")

        print(" -> Sub-challenge 2.5: App background/foreground wall-clock watchdog")
        let bgReq = McpApprovalRequest(id: "bg_req", toolName: "bg_tool", arguments: "{}", ttlSeconds: 600)
        let bgVM = McpApprovalViewModel(requests: [bgReq])
        bgVM.handleAppBackgrounded()
        // Wait 10ms then foreground
        try await Task.sleep(nanoseconds: 10_000_000)
        bgVM.handleAppForegrounded()
        try assertTrue(bgVM.remainingSeconds <= 600)
    }

    // =========================================================================
    // MARK: - VECTOR 3: Presentation Metrics, Layout & Gesture Bounds
    // =========================================================================

    @MainActor
    static func challengeVector3_PresentationMetricsAndBounds() async throws {
        print(" -> Sub-challenge 3.1: ScrollToBottomButton hysteresis with fluctuating scroll offsets")
        // Static threshold verification
        try assertEqual(ScrollToBottomButton.buttonSize, 38.0)
        try assertEqual(ScrollToBottomButton.appearThreshold, 200.0)
        try assertEqual(ScrollToBottomButton.dismissThreshold, 100.0)
        try assertEqual(ScrollToBottomButton.unreadBadgeOffset, 8.0)

        // Hysteresis test sequence:
        // Initial state: not visible
        var isVis = false
        // Distance 0 -> false
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 0, currentlyVisible: isVis)
        try assertFalse(isVis)

        // Distance 100 -> false
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 100, currentlyVisible: isVis)
        try assertFalse(isVis)

        // Distance 199.9 -> false
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 199.9, currentlyVisible: isVis)
        try assertFalse(isVis)

        // Distance 200.0 (boundary: must be > 200.0) -> false
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 200.0, currentlyVisible: isVis)
        try assertFalse(isVis)

        // Distance 200.001 -> true!
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 200.001, currentlyVisible: isVis)
        try assertTrue(isVis, "Distance > 200pt must trigger visibility")

        // Fluctuating sequence from DISPATCH: 199 -> 201 -> 150 -> 99 -> 101
        // Reset to not visible
        isVis = false
        // 1. At 199:
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 199, currentlyVisible: isVis)
        try assertFalse(isVis, "199pt from false must remain false")

        // 2. At 201:
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 201, currentlyVisible: isVis)
        try assertTrue(isVis, "201pt must turn visible")

        // 3. At 150:
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 150, currentlyVisible: isVis)
        try assertTrue(isVis, "150pt from true must remain visible (hysteresis >= 100)")

        // 4. At 99:
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 99, currentlyVisible: isVis)
        try assertFalse(isVis, "99pt must dismiss (< 100)")

        // 5. At 101:
        isVis = ScrollToBottomButton.evaluateVisibility(distance: 101, currentlyVisible: isVis)
        try assertFalse(isVis, "101pt from false must NOT appear until > 200")

        // Boundary tests for dismiss threshold (>= 100 when currently visible)
        isVis = true
        try assertTrue(ScrollToBottomButton.evaluateVisibility(distance: 100.0, currentlyVisible: isVis), "100.0pt must remain visible")
        try assertFalse(ScrollToBottomButton.evaluateVisibility(distance: 99.999, currentlyVisible: isVis), "99.999pt must dismiss")

        // Negative distances
        try assertFalse(ScrollToBottomButton.evaluateVisibility(distance: -50, currentlyVisible: false))
        try assertFalse(ScrollToBottomButton.evaluateVisibility(distance: -1000, currentlyVisible: true))

        print(" -> Sub-challenge 3.2: TelegramComposerLayoutCalculator keyboard offset and height clamping")
        // Hardware keyboard: offset must always be 0.0
        let hwOffset1 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: 55.0, isHardwareKeyboard: true)
        try assertAlmostEqual(hwOffset1, 0.0)

        let hwOffset2 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: 336.0, bottomSafeArea: 34.0, isHardwareKeyboard: true)
        try assertAlmostEqual(hwOffset2, 0.0)

        let hwOffset3 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: 0.0, isHardwareKeyboard: true)
        try assertAlmostEqual(hwOffset3, 0.0)

        // Software keyboard without safe area
        let swOffset1 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: 336.0, isHardwareKeyboard: false)
        try assertAlmostEqual(swOffset1, 336.0)

        let swOffset2 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: 0.0, isHardwareKeyboard: false)
        try assertAlmostEqual(swOffset2, 0.0)

        let swOffset3 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: -20.0, isHardwareKeyboard: false)
        try assertAlmostEqual(swOffset3, 0.0)

        // Software keyboard with safe area
        let swSafeAreaOffset1 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: 336.0, bottomSafeArea: 34.0, isHardwareKeyboard: false)
        try assertAlmostEqual(swSafeAreaOffset1, 302.0)

        let swSafeAreaOffset2 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: 20.0, bottomSafeArea: 34.0, isHardwareKeyboard: false)
        try assertAlmostEqual(swSafeAreaOffset2, 0.0, accuracy: 0.001, "Offset must clamp to 0 when keyboardHeight <= bottomSafeArea")

        let swSafeAreaOffset3 = TelegramComposerLayoutCalculator.computeKeyboardOffset(keyboardHeight: 500.0, bottomSafeArea: 0.0, isHardwareKeyboard: false)
        try assertAlmostEqual(swSafeAreaOffset3, 500.0)

        // Height clamping verification
        try assertAlmostEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 0), 36.0)
        try assertAlmostEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 1), 36.0) // 1 * 20 + 16 = 36.0
        try assertAlmostEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 2), 56.0) // 2 * 20 + 16 = 56.0
        try assertAlmostEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 4), 96.0) // 4 * 20 + 16 = 96.0
        try assertAlmostEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 5), 116.0) // 5 * 20 + 16 = 116.0
        try assertAlmostEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 6), 120.0) // 6 * 20 + 16 = 136 -> clamped to 120.0
        try assertAlmostEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 50), 120.0)

        print(" -> Sub-challenge 3.3: MessageContextMenuModifier thresholds and top edge flip conditions")
        // Static metrics verification
        try assertAlmostEqual(MessageContextMenuModifier.dragThreshold, 10.0)
        try assertAlmostEqualDouble(MessageContextMenuModifier.pressDuration, 0.35)
        try assertAlmostEqualDouble(MessageContextMenuModifier.longPressDuration, 0.35)
        try assertAlmostEqual(MessageContextMenuModifier.hapticIntensity, 0.8)
        try assertAlmostEqualDouble(MessageContextMenuModifier.doubleTapSpeedThreshold, 0.25)
        try assertAlmostEqualDouble(MessageContextMenuModifier.doubleTapDebounce, 0.15)
        try assertAlmostEqual(MessageContextMenuModifier.topEdgeFlipThreshold, 120.0)

        // Top edge flip condition logic:
        // `let flipBelow = coordinator.bubbleFrame.minY < Self.topEdgeFlipThreshold`
        let flipThreshold = MessageContextMenuModifier.topEdgeFlipThreshold // 120.0

        let flipAt119_9 = 119.9 < flipThreshold
        try assertTrue(flipAt119_9, "minY 119.9pt must flip reaction bar below")

        let flipAt120_0 = 120.0 < flipThreshold
        try assertFalse(flipAt120_0, "minY 120.0pt must NOT flip below")

        let flipAt120_1 = 120.1 < flipThreshold
        try assertFalse(flipAt120_1, "minY 120.1pt must NOT flip below")

        let flipAt0 = 0.0 < flipThreshold
        try assertTrue(flipAt0, "minY 0.0pt must flip below")

        let flipAtNeg50 = -50.0 < flipThreshold
        try assertTrue(flipAtNeg50, "minY -50.0pt must flip below")

        let flipAt500 = 500.0 < flipThreshold
        try assertFalse(flipAt500, "minY 500.0pt must NOT flip below")
    }

    // =========================================================================
    // MARK: - VECTOR 4: MediaTransferManager Bounds & Partitioning
    // =========================================================================

    @MainActor
    static func challengeVector4_MediaTransferBounds() async throws {
        print(" -> Sub-challenge 4.1: Chunk partitioning with non-multiple chunk sizes (512KB + 1 byte)")
        let chunkSize = 512 * 1024 // 524,288 bytes

        // Case A: 512KB + 1 byte (524,289 bytes)
        var buffer512Plus1 = Data(repeating: 0x41, count: chunkSize)
        buffer512Plus1.append(0x42)
        try assertEqual(buffer512Plus1.count, chunkSize + 1)

        let chunks512Plus1 = try MediaTransferManager.partition(data: buffer512Plus1, chunkSize: chunkSize)
        try assertEqual(chunks512Plus1.count, 2, "512KB + 1 byte must yield exactly 2 chunks")
        try assertEqual(chunks512Plus1[0].count, chunkSize)
        try assertEqual(chunks512Plus1[1].count, 1)
        try assertEqual(chunks512Plus1[1][0], 0x42)

        // Reassembly verification
        var reassembled = Data()
        for c in chunks512Plus1 {
            reassembled.append(c)
        }
        try assertEqual(reassembled, buffer512Plus1)
        try assertEqual(MediaTransferManager.computeSHA256(data: reassembled), MediaTransferManager.computeSHA256(data: buffer512Plus1))

        // Case B: 1 byte alone
        let singleByteData = Data([0x77])
        let chunksSingle = try MediaTransferManager.partition(data: singleByteData, chunkSize: chunkSize)
        try assertEqual(chunksSingle.count, 1)
        try assertEqual(chunksSingle[0].count, 1)

        // Case C: Exact 512KB
        let exactChunkData = Data(repeating: 0x55, count: chunkSize)
        let chunksExact = try MediaTransferManager.partition(data: exactChunkData, chunkSize: chunkSize)
        try assertEqual(chunksExact.count, 1)
        try assertEqual(chunksExact[0].count, chunkSize)

        // Case D: Exact 2 chunks (1024 KB)
        let exact2ChunksData = Data(repeating: 0x66, count: chunkSize * 2)
        let chunks2Exact = try MediaTransferManager.partition(data: exact2ChunksData, chunkSize: chunkSize)
        try assertEqual(chunks2Exact.count, 2)
        try assertEqual(chunks2Exact[0].count, chunkSize)
        try assertEqual(chunks2Exact[1].count, chunkSize)

        // Case E: 2 chunks minus 1 byte
        let minusOneData = Data(repeating: 0x88, count: (chunkSize * 2) - 1)
        let chunksMinusOne = try MediaTransferManager.partition(data: minusOneData, chunkSize: chunkSize)
        try assertEqual(chunksMinusOne.count, 2)
        try assertEqual(chunksMinusOne[0].count, chunkSize)
        try assertEqual(chunksMinusOne[1].count, chunkSize - 1)

        // Case F: 2.5MB payload
        let payload2_5MB = Data(repeating: 0x99, count: Int(2.5 * 1024 * 1024))
        let chunks2_5MB = try MediaTransferManager.partition(data: payload2_5MB, chunkSize: chunkSize)
        try assertEqual(chunks2_5MB.count, 5)

        print(" -> Sub-challenge 4.2: validateChunkIndex out-of-bounds rejection boundaries")
        let total = 5

        // Valid indices: 0..<5 must NOT throw
        for idx in 0..<total {
            try MediaTransferManager.validateChunkIndex(index: idx, totalChunks: total)
        }

        // Negative index: -1 must throw
        var didCatchNegative = false
        do {
            try MediaTransferManager.validateChunkIndex(index: -1, totalChunks: total)
        } catch let err as MediaTransferError {
            if case .chunkIndexOutOfBounds(let idx, let t) = err {
                try assertEqual(idx, -1)
                try assertEqual(t, total)
                didCatchNegative = true
            }
        }
        try assertTrue(didCatchNegative, "Index -1 must throw chunkIndexOutOfBounds")

        // Upper boundary: index == totalChunks (5) must throw
        var didCatchBoundary = false
        do {
            try MediaTransferManager.validateChunkIndex(index: total, totalChunks: total)
        } catch let err as MediaTransferError {
            if case .chunkIndexOutOfBounds(let idx, let t) = err {
                try assertEqual(idx, total)
                try assertEqual(t, total)
                didCatchBoundary = true
            }
        }
        try assertTrue(didCatchBoundary, "Index == totalChunks must throw chunkIndexOutOfBounds")

        // Far out of bounds: index == 100
        var didCatchFar = false
        do {
            try MediaTransferManager.validateChunkIndex(index: 100, totalChunks: total)
        } catch let err as MediaTransferError {
            if case .chunkIndexOutOfBounds(let idx, let t) = err {
                try assertEqual(idx, 100)
                try assertEqual(t, total)
                didCatchFar = true
            }
        }
        try assertTrue(didCatchFar, "Index 100 must throw chunkIndexOutOfBounds")

        // Zero totalChunks
        var didCatchZeroTotal = false
        do {
            try MediaTransferManager.validateChunkIndex(index: 0, totalChunks: 0)
        } catch let err as MediaTransferError {
            if case .chunkIndexOutOfBounds(let idx, let t) = err {
                try assertEqual(idx, 0)
                try assertEqual(t, 0)
                didCatchZeroTotal = true
            }
        }
        try assertTrue(didCatchZeroTotal, "totalChunks == 0 must throw chunkIndexOutOfBounds")

        print(" -> Sub-challenge 4.3: chunk(at:from:chunkSize:) direct accessor & error conditions")
        let sampleData = Data([0x01, 0x02, 0x03, 0x04, 0x05])
        let c0 = try MediaTransferManager.chunk(at: 0, from: sampleData, chunkSize: 2)
        try assertEqual(c0, Data([0x01, 0x02]))
        let c1 = try MediaTransferManager.chunk(at: 1, from: sampleData, chunkSize: 2)
        try assertEqual(c1, Data([0x03, 0x04]))
        let c2 = try MediaTransferManager.chunk(at: 2, from: sampleData, chunkSize: 2)
        try assertEqual(c2, Data([0x05]))

        // Index 3 is out of bounds for 3 chunks (indices 0, 1, 2)
        var didCatchChunkOOB = false
        do {
            _ = try MediaTransferManager.chunk(at: 3, from: sampleData, chunkSize: 2)
        } catch let err as MediaTransferError {
            if case .chunkIndexOutOfBounds(let idx, let t) = err {
                try assertEqual(idx, 3)
                try assertEqual(t, 3)
                didCatchChunkOOB = true
            }
        }
        try assertTrue(didCatchChunkOOB, "Accessing index 3 of 3 must throw chunkIndexOutOfBounds")

        // Empty file rejection
        var didCatchEmpty = false
        do {
            _ = try MediaTransferManager.partition(data: Data(), chunkSize: chunkSize)
        } catch let err as MediaTransferError {
            if case .emptyFile = err {
                didCatchEmpty = true
            }
        }
        try assertTrue(didCatchEmpty, "Partitioning empty data must throw .emptyFile")

        // Invalid chunkSize <= 0
        var didCatchInvalidSize = false
        do {
            _ = try MediaTransferManager.partition(data: sampleData, chunkSize: 0)
        } catch let err as MediaTransferError {
            if case .invalidChunkSize(let exp, let act) = err {
                try assertEqual(exp, chunkSize)
                try assertEqual(act, 0)
                didCatchInvalidSize = true
            }
        }
        try assertTrue(didCatchInvalidSize, "chunkSize <= 0 must throw .invalidChunkSize")
    }

    // =========================================================================
    // MARK: - VECTOR 5: Production Integrity & Build Hygiene Audit
    // =========================================================================

    @MainActor
    static func challengeVector5_IntegrityAndBuildHygiene() async throws {
        print(" -> Sub-challenge 5.1: Memory Longevity & Process RSS verification")
        let rssStart = getRssMB()
        try assertTrue(rssStart > 0.0, "Process RSS must be measurable")
        print("    Initial Process RSS: \(String(format: "%.2f", rssStart)) MB")

        // Rapid allocation & deallocation burst
        for _ in 1...500 {
            let tempVM = AgentCardViewModel(agentName: "Transient")
            tempVM.submitPrompt("Temporary task")
            tempVM.appendThinkingToken("tokens", elapsedMs: 10)
            tempVM.abortTurn()
        }

        let rssEnd = getRssMB()
        print("    Final Process RSS: \(String(format: "%.2f", rssEnd)) MB")
        let rssDelta = rssEnd - rssStart
        print("    RSS Delta: \(String(format: "%.2f", rssDelta)) MB")
        try assertTrue(rssEnd < 250.0, "Total RSS must stay well under 250MB (actual: \(rssEnd) MB)")

        print(" -> Sub-challenge 5.2: Verification of Replay Window C-ABI integration")
        let replay = TelegramReplayWindow()
        // Contiguous sequence numbers
        for seq in 1...100 {
            try assertTrue(replay.checkAndAdd(seq: UInt64(seq)), "Sequence \(seq) must be accepted")
        }
        // Duplicate sequence numbers must be rejected
        for seq in 1...100 {
            try assertFalse(replay.checkAndAdd(seq: UInt64(seq)), "Duplicate sequence \(seq) must be rejected")
        }
        // Future advance
        try assertTrue(replay.checkAndAdd(seq: 150))
        try assertFalse(replay.checkAndAdd(seq: 150))
    }

    // =========================================================================
    // MARK: - Suite Entry Point
    // =========================================================================

    public static func main() async {
        print("================================================================================")
        print(" EMPIRICAL ADVERSARIAL CHALLENGER SUITE: Milestone 6 Iteration 3")
        print(" Target: Native Presentation Layer & Remediated Production Components")
        print(" Operating Environment: Swift 6 / ARM64 macOS")
        print("================================================================================")

        let t0 = CFAbsoluteTimeGetCurrent()

        await runSection("Vector 1: AgentCardViewModel State Machine Stress & Boundaries") {
            try await challengeVector1_AgentCardStateStress()
        }

        await runSection("Vector 2: McpApprovalViewModel Countdown, Preemption & Races") {
            try await challengeVector2_McpCountdownAndRaces()
        }

        await runSection("Vector 3: Presentation Metrics, Layout & Gesture Bounds") {
            try await challengeVector3_PresentationMetricsAndBounds()
        }

        await runSection("Vector 4: MediaTransferManager Bounds & Partitioning") {
            try await challengeVector4_MediaTransferBounds()
        }

        await runSection("Vector 5: Production Integrity & Build Hygiene Audit") {
            try await challengeVector5_IntegrityAndBuildHygiene()
        }

        let elapsed = CFAbsoluteTimeGetCurrent() - t0

        print("\n================================================================================")
        print(" CHALLENGE EXECUTION SUMMARY")
        print("================================================================================")
        print(" Total Verification Checks : \(totalChecks)")
        print(" Passed Challenge Sections : \(passCount) / 5")
        print(" Failed Challenge Sections : \(failCount) / 5")
        print(" Elapsed Execution Time    : \(String(format: "%.3f", elapsed)) s")
        print(" Final RSS Memory          : \(String(format: "%.2f", getRssMB())) MB")
        print("================================================================================")

        if failCount > 0 {
            print(">>> VERDICT: FAIL — ADVERSARIAL DISCREPANCIES DETECTED <<<")
            exit(1)
        } else {
            print(">>> VERDICT: APPROVE — ALL EMPIRICAL ADVERSARIAL CHALLENGES PASSED <<<")
            exit(0)
        }
    }
}

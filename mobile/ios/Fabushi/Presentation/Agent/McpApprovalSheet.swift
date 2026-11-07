//
//  McpApprovalSheet.swift
//  Fabushi
//
//  Feature 33: MCP Tool Dock & Approval Gate (1:1 Telegram iOS Native).
//  10-minute sensitive action countdown sheet, critical command risk level screening,
//  obfuscation defense, wall-clock background watchdog, and auto-timeout rejection.
//

import SwiftUI

// MARK: - Risk Level & Decision Enums

public enum ToolRiskLevel: String, Sendable, Codable, Comparable {
    case readOnly = "read_only"
    case low = "low"
    case medium = "medium"
    case high = "high"
    case critical = "critical_destructive"

    private var severity: Int {
        switch self {
        case .readOnly: return 0
        case .low: return 1
        case .medium: return 2
        case .high: return 3
        case .critical: return 4
        }
    }

    public static func < (lhs: ToolRiskLevel, rhs: ToolRiskLevel) -> Bool {
        lhs.severity < rhs.severity
    }

    public var title: String {
        switch self {
        case .readOnly: return "Read Only"
        case .low: return "Low Risk"
        case .medium: return "Medium Risk"
        case .high: return "High Risk"
        case .critical: return "Critical Destructive"
        }
    }
}

public enum ApprovalDecision: Sendable, Equatable {
    case approveOnce
    case alwaysAllow
    case deny(reason: String?)
    case timeout
}

// MARK: - Risk Screening Engine

public enum ToolRiskEvaluator {
    /// Screens commands and parameters for dangerous or obfuscated destructive commands.
    public static func evaluate(toolName: String, command: String) -> ToolRiskLevel {
        let normalized = command.lowercased()
            .replacingOccurrences(of: "\\ ", with: " ")
            .replacingOccurrences(of: "\t", with: " ")

        // Check critical destructive patterns
        let criticalPatterns = [
            "rm -rf", "rm -r -f", "rm -fr",
            "eval(", "eval (", "base64",
            "drop table", "drop database", "truncate table",
            "kill -9", "pkill -9",
            ":(){ :|:& };:", "chmod 777", "mkfs"
        ]

        for pattern in criticalPatterns {
            if normalized.contains(pattern) {
                return .critical
            }
        }

        // Check high risk commands
        let highPatterns = ["delete", "remove", "kill", "format", "write", "exec", "sudo"]
        for pattern in highPatterns {
            if normalized.contains(pattern) || toolName.lowercased().contains(pattern) {
                return .high
            }
        }

        // Check medium risk
        let mediumPatterns = ["modify", "update", "patch", "upload", "send"]
        for pattern in mediumPatterns {
            if normalized.contains(pattern) || toolName.lowercased().contains(pattern) {
                return .medium
            }
        }

        // Check read-only
        let readPatterns = ["read", "get", "list", "query", "fetch", "search"]
        for pattern in readPatterns {
            if toolName.lowercased().contains(pattern) {
                return .readOnly
            }
        }

        return .low
    }
}

// MARK: - Models

public struct McpApprovalRequest: Identifiable, Sendable, Equatable {
    public let id: String
    public let toolName: String
    public let arguments: String
    public let riskLevel: ToolRiskLevel
    public let expiresAt: Date

    public init(
        id: String,
        toolName: String,
        arguments: String,
        riskLevel: ToolRiskLevel? = nil,
        ttlSeconds: Int = 600
    ) {
        self.id = id
        self.toolName = toolName
        self.arguments = arguments
        self.riskLevel = riskLevel ?? ToolRiskEvaluator.evaluate(toolName: toolName, command: arguments)
        self.expiresAt = Date().addingTimeInterval(TimeInterval(ttlSeconds))
    }
}

// MARK: - View Model

@MainActor
public final class McpApprovalViewModel: ObservableObject {
    @Published public var queue: [McpApprovalRequest] = []
    @Published public var currentRequest: McpApprovalRequest?
    @Published public var remainingSeconds: Int = 600 // 10 minutes default
    @Published public var lastDecision: ApprovalDecision?
    @Published public var isPresented: Bool = false

    private var countdownTask: Task<Void, Never>?
    private var backgroundTimestamp: Date?

    public var onDecisionResolved: (@Sendable (String, ApprovalDecision) -> Void)?

    public init(requests: [McpApprovalRequest] = []) {
        var mutableRequests = requests
        if !mutableRequests.isEmpty {
            let first = mutableRequests.removeFirst()
            self.currentRequest = first
            self.remainingSeconds = max(0, Int(ceil(first.expiresAt.timeIntervalSinceNow)))
            self.isPresented = true
            self.queue = mutableRequests
            startTimer()
        } else {
            self.queue = []
        }
    }

    public func enqueueRequest(_ req: McpApprovalRequest) {
        if currentRequest == nil {
            currentRequest = req
            remainingSeconds = max(0, Int(ceil(req.expiresAt.timeIntervalSinceNow)))
            isPresented = true
            startTimer()
        } else {
            queue.append(req)
        }
    }

    public func approve(always: Bool = false) {
        guard let current = currentRequest else { return }
        let decision: ApprovalDecision = always ? .alwaysAllow : .approveOnce
        resolve(decision: decision, for: current)
    }

    public func reject(reason: String? = nil) {
        guard let current = currentRequest else { return }
        resolve(decision: .deny(reason: reason), for: current)
    }

    private func resolve(decision: ApprovalDecision, for req: McpApprovalRequest) {
        lastDecision = decision
        onDecisionResolved?(req.id, decision)

        // Advance queue
        countdownTask?.cancel()
        if !queue.isEmpty {
            let next = queue.removeFirst()
            currentRequest = next
            remainingSeconds = max(0, Int(ceil(next.expiresAt.timeIntervalSinceNow)))
            startTimer()
        } else {
            currentRequest = nil
            isPresented = false
        }
    }

    public func handleAppBackgrounded() {
        backgroundTimestamp = Date()
    }

    public func handleAppForegrounded() {
        guard let bg = backgroundTimestamp, let current = currentRequest else { return }
        let elapsed = Int(Date().timeIntervalSince(bg))
        remainingSeconds = max(0, remainingSeconds - elapsed)
        backgroundTimestamp = nil

        if remainingSeconds <= 0 {
            // Expired immediately
            resolve(decision: .timeout, for: current)
        }
    }

    private func startTimer() {
        countdownTask?.cancel()
        countdownTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                guard let self = self else { break }
                if self.remainingSeconds > 0 {
                    self.remainingSeconds -= 1
                    if self.remainingSeconds == 0, let cur = self.currentRequest {
                        self.resolve(decision: .timeout, for: cur)
                    }
                }
            }
        }
    }

    /// Advances countdown clock by specified seconds. Triggers timeout resolution if countdown expires.
    public func tick(seconds: TimeInterval = 1.0) {
        guard let current = currentRequest else { return }
        let secInt = Int(seconds)
        remainingSeconds = max(0, remainingSeconds - secInt)
        if remainingSeconds == 0 {
            resolve(decision: .timeout, for: current)
        }
    }

    public func tick(seconds: Int) {
        tick(seconds: TimeInterval(seconds))
    }

    public var formattedTime: String {
        let mins = remainingSeconds / 60
        let secs = remainingSeconds % 60
        return String(format: "%02d:%02d", mins, secs)
    }

    deinit {
        countdownTask?.cancel()
    }
}

// MARK: - Presentation View

public struct McpApprovalSheet: View {
    @StateObject public var viewModel: McpApprovalViewModel
    @Environment(\.dismiss) private var dismiss

    public init(viewModel: McpApprovalViewModel) {
        _viewModel = StateObject(wrappedValue: viewModel)
    }

    public var body: some View {
        NavigationStack {
            if let req = viewModel.currentRequest {
                VStack(spacing: 20) {
                    // Risk Warning Banner
                    if req.riskLevel == .critical {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .font(.system(size: 20))
                            VStack(alignment: .leading, spacing: 2) {
                                Text("CRITICAL DESTRUCTIVE COMMAND")
                                    .font(.system(size: 13, weight: .bold))
                                Text("This action may modify or permanently delete system files.")
                                    .font(.system(size: 11))
                            }
                        }
                        .foregroundStyle(.white)
                        .padding()
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color.red, in: RoundedRectangle(cornerRadius: 12))
                        .padding(.horizontal)
                    }

                    // Countdown Timer Pill
                    HStack(spacing: 6) {
                        Image(systemName: "timer")
                            .font(.system(size: 14))
                        Text("Auto-rejects in \(viewModel.formattedTime)")
                            .font(.system(size: 14, weight: .semibold, design: .monospaced))
                    }
                    .foregroundStyle(viewModel.remainingSeconds < 60 ? .red : .secondary)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
                    .background(Color.mcpSecondaryGroupedBackground, in: Capsule())

                    // Tool Details Box
                    VStack(alignment: .leading, spacing: 12) {
                        HStack {
                            Text("Tool:")
                                .font(.system(size: 14, weight: .medium))
                                .foregroundStyle(.secondary)
                            Text(req.toolName)
                                .font(.system(size: 15, weight: .bold))
                            Spacer()
                            riskBadge(for: req.riskLevel)
                        }

                        Divider()

                        Text("Parameters:")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(.secondary)

                        ScrollView {
                            Text(req.arguments)
                                .font(.system(size: 12, design: .monospaced))
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .frame(maxHeight: 180)
                        .padding(10)
                        .background(Color.mcpTertiaryGroupedBackground, in: RoundedRectangle(cornerRadius: 8))
                    }
                    .padding()
                    .background(Color.mcpSecondaryGroupedBackground, in: RoundedRectangle(cornerRadius: 14))
                    .padding(.horizontal)

                    Spacer()

                    // Action Buttons Dock
                    VStack(spacing: 12) {
                        Button {
                            viewModel.approve(always: false)
                        } label: {
                            Text("Approve Once")
                                .font(.system(size: 16, weight: .semibold))
                                .foregroundStyle(.white)
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 14)
                                .background(req.riskLevel == .critical ? Color.orange : Color.blue, in: RoundedRectangle(cornerRadius: 12))
                        }

                        HStack(spacing: 12) {
                            Button {
                                viewModel.reject(reason: "User denied execution")
                            } label: {
                                Text("Reject")
                                    .font(.system(size: 15, weight: .medium))
                                    .foregroundStyle(.red)
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 12)
                                    .background(Color.red.opacity(0.12), in: RoundedRectangle(cornerRadius: 10))
                            }

                            if req.riskLevel != .critical {
                                Button {
                                    viewModel.approve(always: true)
                                } label: {
                                    Text("Always Allow")
                                        .font(.system(size: 15, weight: .medium))
                                        .foregroundStyle(.primary)
                                        .frame(maxWidth: .infinity)
                                        .padding(.vertical, 12)
                                        .background(Color.mcpSecondaryGroupedBackground, in: RoundedRectangle(cornerRadius: 10))
                                }
                            }
                        }
                    }
                    .padding(.horizontal)
                    .padding(.bottom, 24)
                }
                .navigationTitle("Action Approval Gate")
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
            } else {
                VStack(spacing: 12) {
                    Image(systemName: "checkmark.shield.fill")
                        .font(.system(size: 48))
                        .foregroundStyle(.green)
                    Text("No Pending Approvals")
                        .font(.headline)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    @ViewBuilder
    private func riskBadge(for level: ToolRiskLevel) -> some View {
        let (color, text) = riskStyle(for: level)
        Text(text)
            .font(.system(size: 11, weight: .bold))
            .foregroundStyle(color)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(color.opacity(0.15), in: Capsule())
    }

    private func riskStyle(for level: ToolRiskLevel) -> (Color, String) {
        switch level {
        case .readOnly: return (.green, "Read Only")
        case .low: return (.blue, "Low")
        case .medium: return (.yellow, "Medium")
        case .high: return (.orange, "High")
        case .critical: return (.red, "Critical")
        }
    }
}

private extension Color {
    static var mcpSecondaryGroupedBackground: Color {
        #if canImport(UIKit)
        return Color(uiColor: .secondarySystemGroupedBackground)
        #else
        return Color.secondary.opacity(0.12)
        #endif
    }
    static var mcpTertiaryGroupedBackground: Color {
        #if canImport(UIKit)
        return Color(uiColor: .tertiarySystemGroupedBackground)
        #else
        return Color.secondary.opacity(0.08)
        #endif
    }
}

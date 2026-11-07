//
//  AgentCardView.swift
//  Fabushi
//
//  Feature 31: AI Multi-Agent ACP Runner UI (1:1 Telegram iOS Native).
//  Interactive turn-taking cards, streaming thinking tokens, tool execution pills,
//  structured JSON diff inspector, syntax error fallback card, and abort generation controls.
//

import SwiftUI

// MARK: - Phase & Models

public enum AgentTurnPhase: Sendable, Equatable {
    case idle
    case thinking(tokens: String, elapsedMs: UInt64)
    case toolCalling(id: String, name: String, args: String)
    case streamingResponse(text: String)
    case completed(markdown: String)
    case aborted
    case failed(error: String)

    public var isGenerating: Bool {
        switch self {
        case .thinking, .toolCalling, .streamingResponse:
            return true
        case .idle, .completed, .aborted, .failed:
            return false
        }
    }
}

public enum ToolCallParseResult: Sendable, Equatable {
    case success(prettyJson: String)
    case syntaxError(raw: String, errorDescription: String)
}

public enum ToolCallPayloadParser {
    public static func parse(rawJson: String) -> ToolCallParseResult {
        let trimmed = rawJson.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let data = trimmed.data(using: .utf8) else {
            return .syntaxError(raw: rawJson, errorDescription: "Invalid UTF-8 encoding")
        }

        do {
            let jsonObject = try JSONSerialization.jsonObject(with: data, options: [])
            let prettyData = try JSONSerialization.data(withJSONObject: jsonObject, options: [.prettyPrinted, .sortedKeys])
            let prettyString = String(data: prettyData, encoding: .utf8) ?? trimmed
            return .success(prettyJson: prettyString)
        } catch {
            return .syntaxError(raw: rawJson, errorDescription: error.localizedDescription)
        }
    }
}

// MARK: - View Model

@MainActor
public final class AgentCardViewModel: ObservableObject {
    @Published public var agentName: String
    @Published public var phase: AgentTurnPhase = .idle
    @Published public var isThinkingExpanded: Bool = true
    @Published public var toolExecutionHistory: [(id: String, name: String, result: ToolCallParseResult)] = []
    @Published public var accumulatedText: String = ""

    public var onStop: (@Sendable () -> Void)?
    public var onRetry: (@Sendable () -> Void)?

    public init(agentName: String = "Telegram Assistant", phase: AgentTurnPhase = .idle) {
        self.agentName = agentName
        self.phase = phase
    }

    public func appendThinkingToken(_ token: String, elapsedMs: UInt64) {
        switch phase {
        case .thinking(let currentTokens, _):
            phase = .thinking(tokens: currentTokens + token, elapsedMs: elapsedMs)
        default:
            phase = .thinking(tokens: token, elapsedMs: elapsedMs)
        }
    }

    public func setToolCall(id: String, name: String, args: String) {
        let parseResult = ToolCallPayloadParser.parse(rawJson: args)
        toolExecutionHistory.append((id: id, name: name, result: parseResult))
        phase = .toolCalling(id: id, name: name, args: args)
    }

    public func appendResponseChunk(_ chunk: String) {
        accumulatedText += chunk
        phase = .streamingResponse(text: accumulatedText)
    }

    public func completeTurn(markdown: String) {
        accumulatedText = markdown
        phase = .completed(markdown: markdown)
    }

    public func abortTurn() {
        phase = .aborted
        onStop?()
    }

    public func failTurn(error: String) {
        phase = .failed(error: error)
    }

    /// Submits a user prompt to the agent card. Rejects empty or whitespace-only prompts.
    @discardableResult
    public func submitPrompt(_ prompt: String) -> Bool {
        let trimmed = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            failTurn(error: "Empty agent prompt rejected")
            return false
        }
        phase = .thinking(tokens: "", elapsedMs: 0)
        return true
    }
}

// MARK: - Presentation View

public struct AgentCardView: View {
    @StateObject public var viewModel: AgentCardViewModel

    public init(viewModel: AgentCardViewModel) {
        _viewModel = StateObject(wrappedValue: viewModel)
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Header Bar
            HStack(spacing: 8) {
                Circle()
                    .fill(LinearGradient(colors: [.purple, .indigo], startPoint: .topLeading, endPoint: .bottomTrailing))
                    .frame(width: 28, height: 28)
                    .overlay(
                        Image(systemName: "sparkles")
                            .font(.system(size: 13, weight: .bold))
                            .foregroundStyle(.white)
                    )

                Text(viewModel.agentName)
                    .font(.system(size: 14, weight: .semibold))

                Text("[ACP]")
                    .font(.system(size: 10, weight: .bold))
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .background(Color.purple.opacity(0.15), in: Capsule())
                    .foregroundStyle(.purple)

                Spacer()

                if viewModel.phase.isGenerating {
                    Button(action: { viewModel.abortTurn() }) {
                        HStack(spacing: 4) {
                            Image(systemName: "stop.fill")
                                .font(.system(size: 10))
                            Text("Stop")
                                .font(.system(size: 12, weight: .medium))
                        }
                        .foregroundStyle(.red)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Color.red.opacity(0.12), in: Capsule())
                    }
                }
            }

            // Thinking Disclosure Block
            if case .thinking(let tokens, let elapsedMs) = viewModel.phase {
                thinkingDisclosureView(tokens: tokens, elapsedMs: elapsedMs, isLive: true)
            } else if case .completed = viewModel.phase, !viewModel.toolExecutionHistory.isEmpty {
                // Keep historical thinking / tools accessible if needed
            }

            // Tool Execution Cards
            ForEach(viewModel.toolExecutionHistory, id: \.id) { tool in
                toolExecutionCard(name: tool.name, result: tool.result)
            }

            // Response Text Block
            switch viewModel.phase {
            case .streamingResponse(let text):
                VStack(alignment: .leading, spacing: 4) {
                    Text(text)
                        .font(.system(size: 15))
                    ProgressView()
                        .scaleEffect(0.7)
                }

            case .completed(let markdown):
                Text(markdown)
                    .font(.system(size: 15))

            case .aborted:
                HStack(spacing: 6) {
                    Image(systemName: "exclamationmark.octagon.fill")
                        .foregroundStyle(.orange)
                    Text("Generation stopped by user.")
                        .font(.system(size: 13))
                        .foregroundStyle(.secondary)
                }

            case .failed(let err):
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 6) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                        Text("Generation Interrupted")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(.red)
                    }
                    Text(err)
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)

                    if let retry = viewModel.onRetry {
                        Button("Retry", action: retry)
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                    }
                }

            default:
                EmptyView()
            }
        }
        .padding(14)
        .background(Color.agentSecondaryGroupedBackground, in: RoundedRectangle(cornerRadius: 14))
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .stroke(
                    viewModel.phase.isGenerating ?
                    AnyShapeStyle(LinearGradient(colors: [.purple.opacity(0.6), .indigo.opacity(0.3)], startPoint: .topLeading, endPoint: .bottomTrailing)) :
                    AnyShapeStyle(Color.secondary.opacity(0.15)),
                    lineWidth: viewModel.phase.isGenerating ? 1.5 : 0.8
                )
        )
    }

    // MARK: - Thinking Disclosure
    private func thinkingDisclosureView(tokens: String, elapsedMs: UInt64, isLive: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Button(action: {
                withAnimation(.easeInOut(duration: 0.2)) {
                    viewModel.isThinkingExpanded.toggle()
                }
            }) {
                HStack(spacing: 6) {
                    if isLive {
                        ProgressView()
                            .scaleEffect(0.6)
                    }
                    Text("Thinking (\(Double(elapsedMs) / 1000.0, specifier: "%.1f")s)...")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.purple)

                    Spacer()

                    Image(systemName: viewModel.isThinkingExpanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.purple)
                }
            }
            .buttonStyle(.plain)

            if viewModel.isThinkingExpanded {
                Text(tokens)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(nil)
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.purple.opacity(0.06), in: RoundedRectangle(cornerRadius: 8))
            }
        }
    }

    // MARK: - Tool Execution Card
    private func toolExecutionCard(name: String, result: ToolCallParseResult) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "wrench.and.screwdriver.fill")
                    .font(.system(size: 12))
                    .foregroundStyle(Color.blue)

                Text("Tool Call: \(name)")
                    .font(.system(size: 12, weight: .medium))

                Spacer()

                switch result {
                case .success:
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundStyle(.green)
                case .syntaxError:
                    Image(systemName: "exclamationmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundStyle(.red)
                }
            }

            switch result {
            case .success(let prettyJson):
                Text(prettyJson)
                    .font(.system(size: 11, design: .monospaced))
                    .padding(6)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.blue.opacity(0.05), in: RoundedRectangle(cornerRadius: 6))

            case .syntaxError(let raw, let errorDescription):
                VStack(alignment: .leading, spacing: 4) {
                    Text("Malformed Tool JSON: \(errorDescription)")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.red)
                    Text(raw)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                }
                .padding(6)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))
            }
        }
        .padding(8)
        .background(Color.agentTertiaryGroupedBackground, in: RoundedRectangle(cornerRadius: 8))
    }
}

private extension Color {
    static var agentSecondaryGroupedBackground: Color {
        #if canImport(UIKit)
        return Color(uiColor: .secondarySystemGroupedBackground)
        #else
        return Color.secondary.opacity(0.12)
        #endif
    }
    static var agentTertiaryGroupedBackground: Color {
        #if canImport(UIKit)
        return Color(uiColor: .tertiarySystemGroupedBackground)
        #else
        return Color.secondary.opacity(0.08)
        #endif
    }
}

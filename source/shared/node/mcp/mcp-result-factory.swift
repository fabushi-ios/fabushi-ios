import Foundation

struct SandMcpTextContent: Equatable, Sendable {
    let text: String
}

struct SandMcpImageContent: Equatable, Sendable {
    let data: String
    let mimeType: String
}

enum SandMcpContent: Equatable, Sendable {
    case text(SandMcpTextContent)
    case image(SandMcpImageContent)
    case other(caseName: String, value: McpJSONValue?)
}

struct SandMcpContentItem: Equatable, Sendable {
    let content: SandMcpContent
}

struct SandMcpSuccess: Equatable, Sendable {
    let content: [SandMcpContentItem]
    let isError: Bool
    let structuredContent: McpJSONValue?
}

enum SandMcpResultPayload: Equatable, Sendable {
    case success(SandMcpSuccess)
    case error(String)
    case other(caseName: String, value: McpJSONValue?)
}

struct SandMcpResult: Equatable, Sendable {
    let result: SandMcpResultPayload
}

protocol McpResultFactory: Sendable {
    func textItem(_ text: String) -> SandMcpContentItem
    func success(_ original: SandMcpResult, content: [SandMcpContentItem]) -> SandMcpResult
    func error(_ message: String) -> SandMcpResult
}

struct GeneratedMcpResultFactory: McpResultFactory {
    func textItem(_ text: String) -> SandMcpContentItem {
        .init(content: .text(.init(text: text)))
    }

    func success(_ original: SandMcpResult, content: [SandMcpContentItem]) -> SandMcpResult {
        guard case .success(let current) = original.result else { return original }
        return .init(result: .success(.init(
            content: content,
            isError: current.isError,
            structuredContent: current.structuredContent
        )))
    }

    func error(_ message: String) -> SandMcpResult {
        .init(result: .error(message))
    }
}

let generatedMcpResultFactory = GeneratedMcpResultFactory()

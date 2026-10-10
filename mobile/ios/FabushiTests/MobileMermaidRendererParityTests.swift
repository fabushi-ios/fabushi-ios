import XCTest
@testable import Fabushi

final class MobileMermaidRendererParityTests: XCTestCase {
    func testMermaidFenceProjectionPreservesSurroundingAssistantText() {
        let fence = String(repeating: "\u{60}", count: 3)
        let source = "Before\n\(fence)mermaid\nflowchart LR\n  A[Start] -->|yes| B{Done}\n\(fence)\nAfter"
        XCTAssertEqual(
            splitMobileAssistantMermaid(source),
            [
                .init(id: 0, kind: .text, text: "Before\n"),
                .init(id: 1, kind: .mermaid, text: "flowchart LR\n  A[Start] -->|yes| B{Done}"),
                .init(id: 2, kind: .text, text: "\nAfter"),
            ]
        )
    }

    func testNativeMermaidParserProjectsFlowchartDirectionLabelsAndEdges() throws {
        let diagram = try parseMobileMermaidDiagram(
            """
            flowchart LR
              A[Start] -->|yes| B{Review}
              B --> C[Done]
            """
        )
        guard case let .graph(direction, nodes, edges) = diagram else {
            return XCTFail("expected graph")
        }
        XCTAssertEqual(direction, .leftRight)
        XCTAssertEqual(nodes.map(\.id), ["A", "B", "C"])
        XCTAssertEqual(nodes.map(\.label), ["Start", "Review", "Done"])
        XCTAssertEqual(edges.count, 2)
        XCTAssertEqual(edges[0].label, "yes")
    }

    func testNativeMermaidParserProjectsSequenceDiagramOffline() throws {
        let diagram = try parseMobileMermaidDiagram(
            """
            sequenceDiagram
              participant U as User
              participant H as Host
              U->>H: Send
              H-->>U: Ack
            """
        )
        guard case let .sequence(participants, messages) = diagram else {
            return XCTFail("expected sequence")
        }
        XCTAssertEqual(participants.map(\.label), ["User", "Host"])
        XCTAssertEqual(messages.map(\.label), ["Send", "Ack"])
        XCTAssertFalse(messages[0].dashed)
        XCTAssertTrue(messages[1].dashed)
    }

    func testNativeMermaidParserProjectsPieDiagram() throws {
        let diagram = try parseMobileMermaidDiagram(
            """
            pie title Usage
              "Native" : 75
              "Fallback" : 25
            """
        )
        guard case let .pie(slices) = diagram else {
            return XCTFail("expected pie")
        }
        XCTAssertEqual(slices.map(\.label), ["Native", "Fallback"])
        XCTAssertEqual(slices.map(\.value), [75, 25])
    }

    func testNativeMermaidStrictBoundaryRejectsRuntimeDirectivesAndOversizeInput() {
        XCTAssertThrowsError(
            try parseMobileMermaidDiagram(
                """
                %%{init: {'securityLevel':'loose'}}%%
                flowchart TD
                A-->B
                """
            )
        ) { error in
            XCTAssertEqual(error as? MobileMermaidParseError, .unsafeDirective)
        }

        let oversized = "flowchart TD\nA[" + String(repeating: "x", count: mobileMermaidSourceByteCap) + "]"
        XCTAssertThrowsError(try parseMobileMermaidDiagram(oversized)) { error in
            XCTAssertEqual(error as? MobileMermaidParseError, .tooLarge)
        }
    }

    func testUnsupportedValidFamilyFailsReadableInsteadOfExecutingWebRuntime() {
        XCTAssertThrowsError(
            try parseMobileMermaidDiagram("gantt\ntitle Project")
        ) { error in
            XCTAssertEqual(error as? MobileMermaidParseError, .unsupportedDiagram("gantt"))
        }
    }

    func testRenderScopeFencesAccountConversationAndEntryIdentity() {
        let base = mobileMermaidRenderScopeID(
            accountKey: "account-a",
            agentID: "agent-a",
            conversationID: "conversation-a",
            entryID: "message-a"
        )
        XCTAssertNotEqual(
            base,
            mobileMermaidRenderScopeID(
                accountKey: "account-b",
                agentID: "agent-a",
                conversationID: "conversation-a",
                entryID: "message-a"
            )
        )
        XCTAssertNotEqual(
            base,
            mobileMermaidRenderScopeID(
                accountKey: "account-a",
                agentID: "agent-a",
                conversationID: "conversation-b",
                entryID: "message-a"
            )
        )
        XCTAssertNotEqual(
            base,
            mobileMermaidRenderScopeID(
                accountKey: "account-a",
                agentID: "agent-a",
                conversationID: "conversation-a",
                entryID: "message-b"
            )
        )
    }
}

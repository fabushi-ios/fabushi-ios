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

    func testNativeMermaidParserProjectsGanttSectionsDatesAndDependenciesOffline() throws {
        let diagram = try parseMobileMermaidDiagram(
            """
            gantt
              title Release plan
              dateFormat YYYY-MM-DD
              section Build
              Compile :done, build, 2026-10-10, 2d
              Package :active, package, after build, 1d
              section Ship
              Release :milestone, release, 2026-10-13, 0d
            """
        )
        guard case let .gantt(title, tasks) = diagram else {
            return XCTFail("expected gantt")
        }
        XCTAssertEqual(title, "Release plan")
        XCTAssertEqual(tasks.map(\.section), ["Build", "Build", "Ship"])
        XCTAssertEqual(tasks.map(\.label), ["Compile", "Package", "Release"])
        XCTAssertEqual(tasks.map(\.status), [.done, .active, .milestone])
        XCTAssertEqual(tasks[0].startDay, 0, accuracy: 0.001)
        XCTAssertEqual(tasks[0].durationDays, 2, accuracy: 0.001)
        XCTAssertEqual(tasks[1].startDay, 2, accuracy: 0.001)
        XCTAssertEqual(tasks[1].durationDays, 1, accuracy: 0.001)
        XCTAssertEqual(tasks[2].startDay, 3, accuracy: 0.001)
        XCTAssertEqual(tasks[2].durationDays, 0, accuracy: 0.001)
    }

    func testNativeMermaidGanttFailsClosedForUnsupportedDateFormatAndComplexity() {
        XCTAssertThrowsError(
            try parseMobileMermaidDiagram(
                """
                gantt
                  dateFormat DD-MM-YYYY
                  Task : 10-10-2026, 2d
                """
            )
        )
        let tasks = (0...mobileMermaidNodeCap)
            .map { "Task \($0) : id\($0), 2026-10-10, 1d" }
            .joined(separator: "\n")
        XCTAssertThrowsError(try parseMobileMermaidDiagram("gantt\n" + tasks)) { error in
            XCTAssertEqual(error as? MobileMermaidParseError, .tooComplex)
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
    func testRenderQueueSerializesAndKeepsCacheBounded() async {
        let queue = MobileMermaidRenderQueue(capacity: 4)
        for index in 0..<10 {
            _ = await queue.resolve(
                source: "flowchart TD\nA\(index)-->B\(index)",
                theme: index.isMultiple(of: 2) ? "light" : "dark"
            )
        }
        let count = await queue.entryCount()
        XCTAssertEqual(count, 4)
    }

    func testNaturalSizeAndFitScaleUseViewportBounds() throws {
        let diagram = try parseMobileMermaidDiagram(
            """
            flowchart LR
            A[Start] --> B[Middle]
            B --> C[Done]
            """
        )
        let natural = mobileMermaidNaturalSize(diagram)
        XCTAssertGreaterThan(natural.width, 0)
        XCTAssertGreaterThan(natural.height, 0)

        let fit = mobileMermaidFitScale(
            diagram: CGSize(width: 1_000, height: 500),
            viewport: CGSize(width: 500, height: 500)
        )
        XCTAssertEqual(fit, 0.5, accuracy: 0.001)
        XCTAssertEqual(
            mobileMermaidFitScale(
                diagram: CGSize(width: 100, height: 100),
                viewport: CGSize(width: 500, height: 500)
            ),
            1,
            accuracy: 0.001
        )
    }


}

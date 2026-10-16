import XCTest
@testable import Fabushi

final class PackagePromptJSXParityTests: XCTestCase {
    func testJSXFiltersNullBooleanAndEmptyChildren() throws {
        let node = packagePromptJSX(
            "User",
            children: [
                .text("hello"),
                .null,
                .bool(true),
                .text(""),
                .number(2),
            ]
        )
        let messages = try PackagePromptRenderer().renderToMessages(node)
        XCTAssertEqual(messages, [
            .init(role: "user", content: "hello2")
        ])
    }

    func testRoleComponentsPreserveNamesAndToolCallId() throws {
        let renderer = PackagePromptRenderer()
        let node: PackagePromptNode = .array([
            packagePromptSystem(.init(children: [.text("rules")], name: "policy")),
            packagePromptAssistant(.init(children: [.text("working")])),
            packagePromptTool(.init(children: [.text("done")], name: "mcp", toolCallId: "call-1")),
        ])
        XCTAssertEqual(try renderer.renderToMessages(node), [
            .init(role: "system", content: "rules", name: "policy"),
            .init(role: "assistant", content: "working"),
            .init(role: "tool", content: "done", name: "mcp", toolCallId: "call-1"),
        ])
    }

    func testContentRendererPreservesHeadingsSectionsTagsAndParagraphBreaks() throws {
        let renderer = PackagePromptRenderer()
        let content: PackagePromptNode = .array([
            packagePromptJSX("h2", props: .init(children: [.text("Plan")])),
            packagePromptJSX("p", props: .init(children: [.text("first   paragraph")])),
            packagePromptJSX("section", props: .init(
                children: [.text("body")],
                title: "Tool Rules",
                attributes: ["mode": .string("safe\"quoted")]
            )),
            packagePromptJSX("x", props: .init(
                children: [.text("value")],
                tag: "meta",
                attributes: ["enabled": .bool(true), "skip": .bool(false)]
            )),
        ])

        XCTAssertEqual(
            try renderer.renderContent(content),
            """
            ## Plan

            first paragraph

            <tool-rules mode="safe&quot;quoted">
            body
            </tool-rules>

            <meta enabled>value</meta>
            """
        )
    }

    func testListsAndPreformattedContentRemainStructured() throws {
        let renderer = PackagePromptRenderer()
        let list = packagePromptJSX("ol", props: .init(children: [
            packagePromptJSX("li", props: .init(children: [.text("first")])),
            packagePromptJSX("li", props: .init(children: [
                .text("second"),
                packagePromptJSX("ul", props: .init(children: [
                    packagePromptJSX("li", props: .init(children: [.text("nested")]))
                ]))
            ])),
        ]))
        XCTAssertEqual(
            try renderer.renderContent(list),
            """
            1. first
            2. second
               - nested
            """
        )

        let pre = packagePromptJSX("pre", props: .init(children: [
            .text("a"),
            packagePromptJSX("br"),
            .text("  b")
        ]))
        XCTAssertEqual(try renderer.renderContent(pre), "a\n  b")
    }

    func testCustomComponentResolvesBeforeIntrinsicFallback() throws {
        let renderer = PackagePromptRenderer(components: [
            "Banner": { props in
                packagePromptJSX("p", props: .init(children: [
                    .text("banner:"),
                    .array(props.children ?? []),
                ]))
            }
        ])
        let node = packagePromptJSX("Banner", props: .init(children: [.text("hello")]))
        XCTAssertEqual(try renderer.renderContent(node), "banner:hello")
    }
}

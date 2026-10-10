import XCTest
@testable import Fabushi

final class SharedWorkflowModelParityTests: XCTestCase {
    func testParseAndSerializeWorkflowFrontmatter() throws {
        let raw = """
        ---
        name: "Morning Check"
        description: "Review overnight changes"
        metadata:
          source: "https://example.com/skill.md"
        trigger:
          schedule: "@daily"
          enabled: false
        ---
        # Run

        Check the queue.
        """
        let parsed = try XCTUnwrap(parseWorkflowFile(raw))
        XCTAssertEqual(parsed.name, "Morning Check")
        XCTAssertEqual(parsed.description, "Review overnight changes")
        XCTAssertEqual(parsed.sourceRef, "https://example.com/skill.md")
        XCTAssertEqual(parsed.trigger, .init(schedule: "@daily", isEnabled: false))
        XCTAssertTrue(parsed.body.contains("Check the queue."))

        let serialized = serializeWorkflowFile(.init(
            name: parsed.name,
            description: parsed.description,
            body: parsed.body,
            trigger: parsed.trigger,
            sourceRef: parsed.sourceRef
        ), existingData: parsed.data)
        let roundTrip = try XCTUnwrap(parseWorkflowFile(serialized))
        XCTAssertEqual(roundTrip.name, parsed.name)
        XCTAssertEqual(roundTrip.trigger, parsed.trigger)
        XCTAssertEqual(roundTrip.sourceRef, parsed.sourceRef)
    }

    func testWorkflowClampsSlugAndDerivedNames() {
        XCTAssertEqual(slugifyWorkflowName("  Café Review  "), "cafe-review")
        XCTAssertEqual(deriveWorkflowNameFromMarkdown("# Build Release\nbody"), "Build Release")
        XCTAssertEqual(deriveWorkflowNameFromUrl("https://example.com/my-skill.md"), "my skill")
        XCTAssertEqual(clampWorkflowName(String(repeating: "x", count: 100)).count, 80)
        let emojiName = clampWorkflowName(String(repeating: "😀", count: 60))
        XCTAssertEqual(emojiName.utf16.count, 80)
        XCTAssertEqual(emojiName.count, 40)
    }

    func testWorkflowAutomationProjectionRoundTrip() {
        let workflow = WorkflowRecord(
            id: "wf-1",
            name: "Daily",
            description: "desc",
            body: "Do it",
            trigger: .init(schedule: "@daily", isEnabled: true),
            source: .workflow,
            sourceRef: nil,
            isEnabledForAgent: true,
            scheduleDescription: nil,
            createdAt: 10,
            lastRunAt: 20,
            nextRunAt: 30,
            filePath: "/skills/daily/SKILL.md"
        )
        let projection = workflowToAutomation(workflow)
        XCTAssertEqual(projection?.triggerDescription, "Every day at 12:00 AM")
        XCTAssertEqual(projection?.schedule, "@daily")
        if let projection {
            let restored = automationToWorkflow(projection)
            XCTAssertEqual(restored.body, "Do it")
            XCTAssertEqual(restored.source, .automation)
        }
    }

    func testWorkflowSurfacingSkillsReferencesAndMentions() {
        let managed = WorkflowRecord(
            id: "managed-1", name: "Managed Skill", description: "managed", body: "x",
            trigger: nil, source: .managed, sourceRef: nil, isEnabledForAgent: true,
            createdAt: 1, filePath: "/m/SKILL.md"
        )
        let user = WorkflowRecord(
            id: "release", name: "Release Train", description: "release", body: "x",
            trigger: nil, source: .workflow, sourceRef: nil, isEnabledForAgent: true,
            createdAt: 1, filePath: "/u/SKILL.md"
        )
        XCTAssertEqual(agentSkillsFromWorkflows([managed, user]).count, 2)
        XCTAssertTrue(promptReferencesWorkflow("Please run @release train", workflow: user))
        XCTAssertTrue(promptReferencesWorkflow("Use sand-workflow:release", workflow: user))
        XCTAssertTrue(promptReferencesWorkflow("é@release", workflow: user))
        XCTAssertFalse(promptReferencesWorkflow("a@release", workflow: user))

        let rich = #"{"type":"doc","content":[{"type":"workflowReference","attrs":{"id":"release","teachQueueScope":"q1"}},{"type":"workflowReference","attrs":{"id":"release"}}]}"#
        XCTAssertEqual(collectWorkflowReferences(rich), [.init(id: "release", teachQueueScope: "q1")])
    }

    func testLiveSourceWorkflowAndSystemPrompt() {
        let spec = liveWorkflowSpecFromSource(
            name: "Remote Skill",
            source: " https://example.com/SKILL.md "
        )
        XCTAssertEqual(spec.sourceRef, "https://example.com/SKILL.md")
        XCTAssertTrue(spec.body.contains("live reference"))
        XCTAssertTrue(spec.body.contains("`https://example.com/SKILL.md`"))
        XCTAssertEqual(workflowDir("/skills/a/SKILL.md"), "/skills/a")
        XCTAssertTrue(renderWorkflowsSystemPrompt("/skills").contains("sand-workflow:<id>"))
    }
}

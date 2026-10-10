import XCTest
@testable import Fabushi

private struct TestAgentSummary: AgentSummaryIdentity, Equatable {
    let id: String
    let updatedAt: Double
}

final class SharedAgentContractsParityTests: XCTestCase {
    func testDefaultModelSelectionsMatchGrokContracts() {
        XCTAssertEqual(SAND_DEFAULT_MODEL_ID, "grok-4.5")
        XCTAssertEqual(SAND_DEFAULT_MODEL_SELECTION.modelId, "grok-4.5")
        XCTAssertTrue(SAND_DEFAULT_MODEL_SELECTION.maxMode)
        XCTAssertEqual(
            SAND_DEFAULT_MODEL_SELECTION.parameters,
            [
                .init(id: "effort", value: "high"),
                .init(id: "fast", value: "true"),
            ]
        )
        XCTAssertEqual(SAND_COMPUTER_USE_MODEL_SELECTION.modelId, "claude-opus-4-8")
        XCTAssertFalse(SAND_COMPUTER_USE_MODEL_SELECTION.maxMode)
    }

    func testAgentSummaryUpsertReplacesAndSortsNewestFirst() {
        let result = upsertAgentSummary(
            [
                TestAgentSummary(id: "a", updatedAt: 1),
                TestAgentSummary(id: "b", updatedAt: 3),
            ],
            updated: TestAgentSummary(id: "a", updatedAt: 5)
        )
        XCTAssertEqual(result.map(\.id), ["a", "b"])
        XCTAssertEqual(result.map(\.updatedAt), [5, 3])
    }

    func testAgentIdentityPoliciesAreFailClosed() {
        XCTAssertTrue(isSandAgentPurpose("disk-saver"))
        XCTAssertFalse(isSandAgentPurpose("other"))
        XCTAssertEqual(sanitizeTemplateId("safe-template-1"), "safe-template-1")
        XCTAssertNil(sanitizeTemplateId("../unsafe"))
        XCTAssertTrue(isSandDefaultAgentName(" New Bot "))
        XCTAssertEqual(SAND_AGENT_LIMIT_MESSAGE, "50 is the maximum")
    }

    func testDualSurfaceToolTelemetryPreservesSurface() {
        XCTAssertEqual(
            sandDualSurfaceToolTelemetry(SAND_BOX_READ_TOOL_NAME),
            .init(toolName: "Read", surface: .box)
        )
        XCTAssertEqual(
            sandDualSurfaceToolTelemetry(SAND_EXTERNAL_SHELL_TOOL_NAME),
            .init(toolName: "ExternalShell", surface: .external)
        )
        XCTAssertNil(sandDualSurfaceToolTelemetry("Other"))
    }

    func testRemoteAgentReferenceRoundTripsEncodedSegments() throws {
        let reference = RemoteAgentReference(
            ownerAuthId: "owner/a+b",
            agentId: "agent / 中文"
        )
        let encoded = formatRemoteAgentId(reference)
        XCTAssertTrue(encoded.hasPrefix(REMOTE_AGENT_ID_PREFIX))
        XCTAssertFalse(encoded.dropFirst(REMOTE_AGENT_ID_PREFIX.count).contains("/a+b/"))
        XCTAssertEqual(parseRemoteAgentId(encoded), reference)
        XCTAssertNil(parseRemoteAgentId("sand-remote:missing-separator"))
    }

    func testShareAvatarPolicyAcceptsOnlyBoundedImageDataUrls() {
        XCTAssertTrue(isPublishableShareAvatarDataUrl("data:image/png;base64,QUJD"))
        XCTAssertFalse(isPublishableShareAvatarDataUrl("data:text/plain;base64,QUJD"))
        XCTAssertFalse(isPublishableShareAvatarDataUrl("data:image/svg+xml;base64,QUJD"))
    }

    func testModelCatalogAliasesValuesAndCombinations() throws {
        let entry = SandModelCatalogEntry(
            id: "model-x",
            displayName: "Model X",
            aliases: ["mx"],
            params: [
                .init(
                    id: "effort",
                    name: "Effort",
                    type: .enumeration,
                    values: [.init(value: "low"), .init(value: "high")]
                ),
                .init(
                    id: "fast",
                    name: "Fast",
                    type: .boolean,
                    values: [.init(value: "true"), .init(value: "false")]
                ),
            ],
            variants: [
                [
                    .init(id: "effort", value: "low"),
                    .init(id: "fast", value: "true"),
                ],
                [
                    .init(id: "effort", value: "high"),
                    .init(id: "fast", value: "false"),
                ],
            ]
        )

        XCTAssertEqual(findCatalogEntry([entry], modelId: " MX ")?.id, "model-x")
        XCTAssertTrue(validateModelParams(
            entry,
            params: ["effort": "low", "fast": "true"]
        ).isEmpty)
        XCTAssertFalse(validateModelParams(
            entry,
            params: ["effort": "high", "fast": "true"]
        ).isEmpty)
        XCTAssertFalse(describeParamIncompatibilities(entry).isEmpty)
    }

    func testOnboardingAndTeachContractsUseStableIdentity() {
        XCTAssertEqual(introductionFailedTrayKey("agent-1"), "introduction-failed:agent-1")
        XCTAssertTrue(SAND_ONBOARDING_KICKSTART_PROMPT.contains("[first run]"))
        XCTAssertTrue(isSandSubagentId("sand-subagent-123"))
        XCTAssertEqual(IDLE_TEACH_RECORDING_STATUS.state, .idle)
        XCTAssertEqual(IDLE_TEACH_RECORDING_STATUS.maxDurationMs, 600_000)
    }
}

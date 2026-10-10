import XCTest
@testable import Fabushi

final class MobileComposerParityTests: XCTestCase {
    private let attachment = MobileComposerAttachment(
        id: "abc123",
        name: "notes.txt",
        path: "/private/fabushi/notes.txt",
        mimeType: "text/plain",
        sizeBytes: 42
    )

    func testAttachmentOnlyDraftIsSendable() {
        XCTAssertTrue(mobileComposerHasPayload(text: "", attachments: [attachment]))
        XCTAssertTrue(mobileComposerHasPayload(text: "hello", attachments: []))
        XCTAssertFalse(mobileComposerHasPayload(text: "  \n ", attachments: []))
        XCTAssertEqual(mobileComposerAttachmentLimit, 6)
    }

    func testVoiceTranscriptInsertsInsteadOfOverwritingExistingDraft() {
        XCTAssertEqual(
            mergeMobileComposerVoiceTranscript(existing: "Existing draft", transcript: "new words"),
            "Existing draft new words"
        )
        XCTAssertEqual(
            mergeMobileComposerVoiceTranscript(existing: "Existing draft ", transcript: "new words"),
            "Existing draft new words"
        )
        XCTAssertEqual(
            mergeMobileComposerVoiceTranscript(existing: "", transcript: "  hello  "),
            "hello"
        )
    }

    func testAttachmentCommandPayloadPreservesStoredIdentityAndMetadata() {
        let payload = mobileComposerAttachmentCommandPayload(attachment)
        XCTAssertEqual(payload["id"] as? String, "abc123")
        XCTAssertEqual(payload["name"] as? String, "notes.txt")
        XCTAssertEqual(payload["path"] as? String, "/private/fabushi/notes.txt")
        XCTAssertEqual(payload["mimeType"] as? String, "text/plain")
        XCTAssertEqual(payload["sizeBytes"] as? Int, 42)
    }

    func testAttachmentLimitsMatchDesktopComposerContract() {
        XCTAssertEqual(AttachmentLimits.attachmentByteLimit(forName: "notes.txt"), 25 * 1024 * 1024)
        XCTAssertEqual(AttachmentLimits.attachmentByteLimit(forName: "clip.mp4"), 200 * 1024 * 1024)
    }

    func testUnnamedStageFallbackMatchesDesktop() {
        XCTAssertEqual(
            mobileComposerStageFileName(
                proposedName: "",
                fallbackLastPathComponent: "",
                mimeType: "image/png"
            ),
            "image.png"
        )
        XCTAssertEqual(
            mobileComposerStageFileName(
                proposedName: nil,
                fallbackLastPathComponent: "",
                mimeType: "application/octet-stream"
            ),
            "file"
        )
        XCTAssertEqual(
            mobileComposerStageFileName(
                proposedName: " notes.txt ",
                fallbackLastPathComponent: "ignored.bin",
                mimeType: nil
            ),
            "notes.txt"
        )
    }

    func testMcpSuggestionsPreserveAccountSlotsCatalogStatusAndDeduplicateServerIdentifiers() {
        let servers = [
            MarketplaceMcpServer(
                serverId: "17",
                name: "GitHub",
                serverIdentifier: "github-work",
                rowServerIdentifier: "github",
                accountKey: "default",
                transport: "http",
                status: "connected",
                statusDetail: nil,
                toolCount: 5,
                disabledToolCount: 0,
                isTeamServer: false,
                pluginId: nil,
                isRequired: false,
                managedByTeamPluginPolicy: false
            ),
            MarketplaceMcpServer(
                serverId: "18",
                name: "GitHub",
                serverIdentifier: "github-personal",
                rowServerIdentifier: "github",
                accountKey: "personal",
                transport: "http",
                status: "needsAuth",
                statusDetail: "Authentication required",
                toolCount: 0,
                disabledToolCount: 0,
                isTeamServer: false,
                pluginId: nil,
                isRequired: false,
                managedByTeamPluginPolicy: false
            ),
            MarketplaceMcpServer(
                serverId: "99",
                name: "Duplicate",
                serverIdentifier: "github-work",
                rowServerIdentifier: "duplicate",
                accountKey: "other",
                transport: "http",
                status: "disabledByTeamAdminPolicy",
                statusDetail: nil,
                toolCount: 0,
                disabledToolCount: 0,
                isTeamServer: true,
                pluginId: nil,
                isRequired: true,
                managedByTeamPluginPolicy: true
            ),
        ]
        let catalog = [
            MobileConnectorCatalogEntry(
                id: "github",
                name: "GitHub",
                displayName: "GitHub",
                connectors: ["github"],
                iconURL: "https://example.invalid/github.png"
            ),
        ]

        let rows = projectMobileEditorMcpSuggestions(servers: servers, catalog: catalog)

        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows[0].label, "GitHub")
        XCTAssertEqual(rows[0].subtitle, "connected")
        XCTAssertEqual(rows[0].mcpReference?.workflowReferenceID, "mcp:17")
        XCTAssertEqual(rows[0].mcpReference?.accountKey, "default")
        XCTAssertEqual(rows[0].iconURL, "https://example.invalid/github.png")
        XCTAssertEqual(rows[1].label, "GitHub (personal)")
        XCTAssertEqual(rows[1].subtitle, "needs auth")
        XCTAssertEqual(rows[1].mcpReference?.accountKey, "personal")
    }

    func testMcpDisabledPolicyAndAppAccountAgentFencing() {
        let server = MarketplaceMcpServer(
            serverId: "22",
            name: "Linear",
            serverIdentifier: "linear",
            rowServerIdentifier: "linear",
            accountKey: "default",
            transport: "http",
            status: "disabledByTeamAdminPolicy",
            statusDetail: "Disabled by policy",
            toolCount: 0,
            disabledToolCount: 0,
            isTeamServer: true,
            pluginId: nil,
            isRequired: true,
            managedByTeamPluginPolicy: true
        )
        let current = projectScopedMobileEditorMcpSuggestions(
            servers: [server],
            catalog: [],
            ownedAccountKey: "app-account-a",
            currentAccountKey: "app-account-a",
            ownedAgentID: "agent-a",
            currentAgentID: "agent-a"
        )
        XCTAssertEqual(current.first?.subtitle, "disabled")
        XCTAssertEqual(current.first?.mcpReference?.accountKey, "default")
        XCTAssertTrue(projectScopedMobileEditorMcpSuggestions(
            servers: [server],
            catalog: [],
            ownedAccountKey: "app-account-a",
            currentAccountKey: "app-account-b",
            ownedAgentID: "agent-a",
            currentAgentID: "agent-a"
        ).isEmpty)
        XCTAssertTrue(projectScopedMobileEditorMcpSuggestions(
            servers: [server],
            catalog: [],
            ownedAccountKey: "app-account-a",
            currentAccountKey: "app-account-a",
            ownedAgentID: "agent-a",
            currentAgentID: "agent-b"
        ).isEmpty)
    }

    func testMcpWorkflowReferenceRichTextIsBoundToServerIdAndPrunedAfterEdit() throws {
        let reference = MobileComposerMcpReference(
            workflowReferenceID: "mcp:17",
            serverId: "17",
            serverIdentifier: "github",
            accountKey: "default",
            label: "GitHub",
            status: "connected",
            iconURL: nil
        )
        let richText = try XCTUnwrap(
            mobileComposerRichText(
                draft: "@GitHub inspect this PR",
                references: [reference]
            )
        )
        XCTAssertTrue(richText.contains(#"\"type\":\"workflowReference\""#))
        XCTAssertTrue(richText.contains(#"\"id\":\"mcp:17\""#))
        XCTAssertEqual(
            pruneMobileComposerMcpReferences(
                draft: "inspect this PR",
                references: [reference]
            ),
            []
        )
    }

    func testMcpAuthCompletionProjectionRefreshesStatusWithoutChangingIdentity() {
        let needsAuth = MarketplaceMcpServer(
            serverId: "17",
            name: "GitHub",
            serverIdentifier: "github",
            rowServerIdentifier: "github",
            accountKey: "work",
            transport: "http",
            status: "needsAuth",
            statusDetail: nil,
            toolCount: 0,
            disabledToolCount: 0,
            isTeamServer: false,
            pluginId: nil,
            isRequired: false,
            managedByTeamPluginPolicy: false
        )
        let connected = MarketplaceMcpServer(
            serverId: "17",
            name: "GitHub",
            serverIdentifier: "github",
            rowServerIdentifier: "github",
            accountKey: "work",
            transport: "http",
            status: "connected",
            statusDetail: nil,
            toolCount: 4,
            disabledToolCount: 0,
            isTeamServer: false,
            pluginId: nil,
            isRequired: false,
            managedByTeamPluginPolicy: false
        )
        let before = projectMobileEditorMcpSuggestions(servers: [needsAuth], catalog: [])
        let after = projectMobileEditorMcpSuggestions(servers: [connected], catalog: [])
        XCTAssertEqual(before.first?.id, after.first?.id)
        XCTAssertEqual(before.first?.mcpReference?.accountKey, "work")
        XCTAssertEqual(before.first?.subtitle, "needs auth")
        XCTAssertEqual(after.first?.subtitle, "connected")
    }

    func testStageFailureNoticeAggregatesLikeDesktop() {
        XCTAssertEqual(
            mobileComposerStageFailureNotice([
                .init(name: "a.txt", reason: .tooLarge),
                .init(name: "b.mp4", reason: .tooLarge),
            ]),
            "2 files are too large to attach (max 25 MB, or 200 MB for video)."
        )
        XCTAssertEqual(
            mobileComposerStageFailureNotice([
                .init(name: "empty.txt", reason: .empty),
            ]),
            "\"empty.txt\" is empty, so it wasn't attached."
        )
        XCTAssertEqual(
            mobileComposerStageFailureNotice([
                .init(name: "a.txt", reason: .empty),
                .init(name: "b.txt", reason: .failed),
            ]),
            "2 files couldn't be attached."
        )
    }
}

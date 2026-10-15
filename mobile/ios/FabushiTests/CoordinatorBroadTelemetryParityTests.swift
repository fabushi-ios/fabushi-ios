import XCTest
@testable import Fabushi

final class CoordinatorBroadTelemetryParityTests: XCTestCase {
    func testAgentsUnreachableMatchesDesktopContract() {
        let entered = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: [
                "phase": "entered",
                "isManual": false,
                "errorClass": "gateway_unreachable",
                "msSinceLastReachedMs": 1_250.4,
            ]
        )
        XCTAssertEqual(entered?.event, "agents_unreachable")
        XCTAssertEqual(entered?.level, .warn)
        XCTAssertEqual(entered?.metadata["phase"], "entered")
        XCTAssertEqual(entered?.metadata["is_manual"], "false")
        XCTAssertEqual(entered?.metadata["error_class"], "gateway_unreachable")
        XCTAssertEqual(entered?.metadata["ms_since_last_reached"], "1250")

        let retry = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "retry", "isManual": true]
        )
        XCTAssertEqual(retry?.level, .info)

        let recovered = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "recovered", "isManual": false]
        )
        XCTAssertEqual(recovered?.level, .info)

        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "failed", "isManual": false]
        ))
        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "entered"]
        ))
        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "entered", "isManual": false, "msSinceLastReachedMs": -1]
        ))
    }

    func testRecoveryActionMatchesDesktopFutilityThreshold() {
        let second = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: [
                "action": "retry",
                "failureClass": "Gateway_Unreachable",
                "consecutiveIdentical": 2,
            ]
        )
        XCTAssertEqual(second?.event, "recovery_action")
        XCTAssertEqual(second?.level, .info)
        XCTAssertEqual(second?.metadata["action"], "retry")
        XCTAssertEqual(second?.metadata["failure_class"], "gateway_unreachable")
        XCTAssertEqual(second?.metadata["consecutive_identical"], "2")

        let third = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: [
                "action": "recover",
                "failureClass": "timeout",
                "consecutiveIdentical": 3,
            ]
        )
        XCTAssertEqual(third?.level, .warn)

        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: ["outcome": "failed"]
        ))
        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: [
                "action": "retry",
                "failureClass": "contains space",
                "consecutiveIdentical": 1,
            ]
        ))
    }

    func testRebuildLifecycleMatchesDesktopProjection() {
        let escalation = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRebuildLifecycle",
            report: [
                "kind": "escalation",
                "rebuildKind": "update",
                "elapsedMs": 1_200.4,
                "action": "shown",
                "escalation": "taking-longer",
            ]
        )
        XCTAssertEqual(escalation?.event, "escalation")
        XCTAssertEqual(escalation?.level, .warn)
        XCTAssertEqual(escalation?.metadata["kind"], "update")
        XCTAssertEqual(escalation?.metadata["elapsed_ms"], "1200")

        let keepWaiting = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRebuildLifecycle",
            report: [
                "kind": "escalation",
                "rebuildKind": "recover",
                "elapsedMs": 2_000,
                "action": "keep_waiting",
                "escalation": "unreachable",
            ]
        )
        XCTAssertEqual(keepWaiting?.level, .info)

        let pending = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRebuildLifecycle",
            report: [
                "kind": "pending_stall",
                "rebuildKind": "reconnecting",
                "elapsedMs": 5_000,
                "hasAck": true,
                "streamAttached": false,
            ]
        )
        XCTAssertEqual(pending?.event, "pending_stall")
        XCTAssertEqual(pending?.level, .warn)
        XCTAssertEqual(pending?.metadata["has_ack"], "true")
        XCTAssertEqual(pending?.metadata["stream_attached"], "false")

        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRebuildLifecycle",
            report: ["phase": "escalation"]
        ))
    }

    func testReconciliationMatchesDesktopProjection() {
        let covered = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportReconciliation",
            report: [
                "kind": "replica_resync",
                "replica": "roster",
                "outcome": "covered",
                "attempts": 2,
                "durationMs": 60_000,
                "bufferOverflowed": false,
                "epochChanged": true,
            ]
        )
        XCTAssertEqual(covered?.event, "replica_resync")
        XCTAssertEqual(covered?.level, .info)
        XCTAssertEqual(covered?.metadata["attempts"], "2")
        XCTAssertEqual(covered?.metadata["duration_ms"], "60000")
        XCTAssertEqual(covered?.metadata["buffer_overflowed"], "false")
        XCTAssertEqual(covered?.metadata["epoch_changed"], "true")

        let slow = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportReconciliation",
            report: [
                "kind": "replica_resync",
                "replica": "transcript",
                "outcome": "superseded",
                "attempts": 1,
                "durationMs": 60_001,
                "bufferOverflowed": false,
                "epochChanged": false,
            ]
        )
        XCTAssertEqual(slow?.level, .warn)

        let parked = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportReconciliation",
            report: [
                "kind": "send_journal_restore",
                "outcome": "parked",
                "phase": "accepted-awaiting-echo",
                "ageMs": 4_000,
                "recordCount": 3,
            ]
        )
        XCTAssertEqual(parked?.event, "send_journal_restore")
        XCTAssertEqual(parked?.level, .warn)
        XCTAssertEqual(parked?.metadata["age_ms"], "4000")
        XCTAssertEqual(parked?.metadata["record_count"], "3")

        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportReconciliation",
            report: ["outcome": "partial"]
        ))
    }

    func testResyncProjectionRemainsBoundedAndUnknownMethodsAreRejected() {
        let long = String(repeating: "x", count: 800)
        let projection = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportResyncCompleted",
            report: ["failed": 1, "completed": 4, "detail": long, "nested": ["secret": "omit"]]
        )
        XCTAssertEqual(projection?.event, "resync_completed")
        XCTAssertEqual(projection?.level, .warn)
        XCTAssertEqual(projection?.metadata["detail"]?.count, 512)
        XCTAssertNil(projection?.metadata["nested"])
        XCTAssertNil(
            IOSCoordinatorBroadTelemetryProjection.project(
                method: "notTelemetry",
                report: [:]
            )
        )
    }

    func testMainRPCTelemetryAgentLoadAndAccessBlockedMatchDesktopContracts() {
        let agent = IOSMainRPCTelemetryProjection.project(
            method: "reportAgentLoad",
            report: [
                "durationMs": 950.4,
                "openRpcMs": 100,
                "queueMs": 20,
                "renderMs": 300,
                "cold": true,
                "stale": false,
                "boxState": "running",
                "outcome": "ready",
            ]
        )
        XCTAssertEqual(agent?.event, "agent_load")
        XCTAssertEqual(agent?.level, .info)
        XCTAssertEqual(agent?.metadata["duration_ms"], "950")
        XCTAssertEqual(agent?.metadata["box_state"], "running")

        let failedAgent = IOSMainRPCTelemetryProjection.project(
            method: "reportAgentLoad",
            report: [
                "durationMs": 10,
                "cold": false,
                "boxState": "future-state",
                "outcome": "failed",
            ]
        )
        XCTAssertEqual(failedAgent?.level, .warn)
        XCTAssertEqual(failedAgent?.metadata["box_state"], "other")

        let blocked = IOSMainRPCTelemetryProjection.project(
            method: "reportAccessBlocked",
            report: [
                "edge": "entered",
                "cause": "seat_revoked",
                "accessReason": "paywallTeamMember",
                "msBlocked": 999,
            ]
        )
        XCTAssertEqual(blocked?.event, "access_blocked")
        XCTAssertEqual(blocked?.level, .warn)
        XCTAssertEqual(blocked?.metadata["access_reason"], "paywallTeamMember")

        let noPlan = IOSMainRPCTelemetryProjection.project(
            method: "reportAccessBlocked",
            report: [
                "edge": "entered",
                "cause": "no_plan",
                "accessReason": "notOffered",
            ]
        )
        XCTAssertEqual(noPlan?.level, .info)

        XCTAssertNil(IOSMainRPCTelemetryProjection.project(
            method: "reportAccessBlocked",
            report: [
                "edge": "entered",
                "cause": "seat_revoked",
                "accessReason": "invented",
            ]
        ))
    }

    func testMainRPCTelemetrySendAndReactionMatchDesktopContracts() {
        let send = IOSMainRPCTelemetryProjection.project(
            method: "reportSendLatency",
            report: [
                "durationMs": 125.5,
                "commitMs": 20,
                "attachmentCount": 2,
                "isFork": false,
                "traceId": "trace",
                "spanId": "span",
                "conversationId": "conversation",
            ]
        )
        XCTAssertEqual(send?.event, "send_latency")
        XCTAssertEqual(send?.level, .info)
        XCTAssertEqual(send?.metadata["duration_ms"], "126")
        XCTAssertEqual(send?.metadata["attachment_count"], "2")

        let ack = IOSMainRPCTelemetryProjection.project(
            method: "reportSendAck",
            report: [
                "ackMs": 2_500,
                "outcome": "timeout",
                "enterEpochMs": 1_000,
                "isFork": false,
                "attachmentCount": 1,
                "clientOnlineAtTimeout": true,
                "clientNonce": "nonce",
                "conversationId": "conversation",
            ]
        )
        XCTAssertEqual(ack?.event, "send_ack")
        XCTAssertEqual(ack?.level, .warn)
        XCTAssertEqual(ack?.metadata["client_online_at_timeout"], "true")
        XCTAssertEqual(ack?.metadata["client_nonce"], "nonce")

        let reaction = IOSMainRPCTelemetryProjection.project(
            method: "reportReactionAck",
            report: [
                "ackMs": 300,
                "outcome": "landed",
                "conversationId": "conversation",
                "emoji": "👍",
            ]
        )
        XCTAssertEqual(reaction?.event, "reaction_ack")
        XCTAssertEqual(reaction?.level, .info)
        XCTAssertEqual(reaction?.metadata["reaction_id"], "thumbs_up")

        XCTAssertNil(IOSMainRPCTelemetryProjection.project(
            method: "reportSendAck",
            report: [
                "ackMs": -1,
                "outcome": "acked",
                "enterEpochMs": 1,
                "isFork": false,
                "attachmentCount": 0,
            ]
        ))
    }

    func testMainRPCTelemetryRenderMatchesDesktopContracts() {
        let first = IOSMainRPCTelemetryProjection.project(
            method: "reportRenderTtfr",
            report: [
                "ttfrMs": 80,
                "outcome": "rendered",
                "chunkType": "assistant_text",
                "enterEpochMs": 500,
                "traceparent": "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01",
                "isFork": false,
                "conversationId": "conversation",
            ]
        )
        XCTAssertEqual(first?.event, "render_ttfr")
        XCTAssertEqual(first?.level, .info)
        XCTAssertEqual(first?.metadata["chunk_type"], "assistant_text")

        let stream = IOSMainRPCTelemetryProjection.project(
            method: "reportRenderStream",
            report: [
                "outcome": "timeout",
                "chunkCount": 4,
                "maxGapMs": 900,
                "stalledMs": 1_200,
                "stallCount": 2,
                "streamMs": 5_000,
                "isFork": true,
            ]
        )
        XCTAssertEqual(stream?.event, "render_stream")
        XCTAssertEqual(stream?.level, .warn)
        XCTAssertEqual(stream?.metadata["stream_gap_max_ms"], "900")
        XCTAssertEqual(stream?.metadata["stall_count"], "2")

        XCTAssertNil(IOSMainRPCTelemetryProjection.project(
            method: "reportRenderTtfr",
            report: [
                "ttfrMs": 80,
                "outcome": "rendered",
                "chunkType": "invented",
                "enterEpochMs": 500,
                "isFork": false,
            ]
        ))
    }

    func testVncMainRPCTelemetryMatchesDesktopContracts() {
        let session = IOSMainRPCTelemetryProjection.project(
            method: "reportVncSession",
            report: [
                "surface": "interactive",
                "phase": "rfb_disconnect",
                "clean": false,
                "durationMs": 1_200,
                "sinceOpenMs": 2_000,
                "focusOwner": "vnc",
                "focusLanded": true,
            ]
        )
        XCTAssertEqual(session?.event, "vnc_session")
        XCTAssertEqual(session?.level, .warn)
        XCTAssertEqual(session?.metadata["duration_ms"], "1200")
        XCTAssertEqual(session?.metadata["focus_owner"], "vnc")

        let liveness = IOSMainRPCTelemetryProjection.project(
            method: "reportVncLiveness",
            report: [
                "phase": "post_connect",
                "stallMs": 10_000,
                "keys": 3,
                "clicks": 1,
                "moves": 12,
                "inBytes": 512,
            ]
        )
        XCTAssertEqual(liveness?.event, "vnc_liveness")
        XCTAssertEqual(liveness?.level, .warn)
        XCTAssertEqual(liveness?.metadata["error_code"], "SAND-E0609")
        XCTAssertEqual(liveness?.metadata["in_bytes"], "512")

        let open = IOSMainRPCTelemetryProjection.project(
            method: "reportOpenComputer",
            report: [
                "trigger": "handoff",
                "hadVncUrl": true,
                "monitorFocused": false,
            ]
        )
        XCTAssertEqual(open?.event, "open_computer")
        XCTAssertEqual(open?.level, .info)
        XCTAssertEqual(open?.metadata["had_vnc_url"], "true")

        XCTAssertNil(IOSMainRPCTelemetryProjection.project(
            method: "reportVncSession",
            report: ["surface": "native", "phase": "navigate"]
        ))
        XCTAssertNil(IOSMainRPCTelemetryProjection.project(
            method: "reportVncLiveness",
            report: ["phase": "post_connect", "stallMs": -1, "keys": 0, "clicks": 0, "moves": 0, "inBytes": 0]
        ))
    }

    func testClientFailureTelemetryValidatesCanonicalKinds() {
        let resource = IOSMainRPCTelemetryProjection.project(
            method: "reportClientFailure",
            report: [
                "kind": "client_resource",
                "domain": "transcript",
                "operation": "hydrate",
                "state": "failed",
                "failureCode": "SAND-E0701",
                "boundary": "client",
                "retryOwner": "client",
                "transportKind": "network",
                "failedMs": 250,
            ]
        )
        XCTAssertEqual(resource?.event, "sand.client_resource")
        XCTAssertEqual(resource?.level, .warn)
        XCTAssertEqual(resource?.metadata["failure_code"], "SAND-E0701")
        XCTAssertEqual(resource?.metadata["failed_ms"], "250")

        let boundary = IOSMainRPCTelemetryProjection.project(
            method: "reportClientFailure",
            report: [
                "kind": "renderer_boundary_caught",
                "boundary": "chat_screen",
                "errorClass": "TypeError",
                "occurrence": 2,
            ]
        )
        XCTAssertEqual(boundary?.event, "sand.renderer.boundary_caught")
        XCTAssertEqual(boundary?.level, .warn)

        let queued = IOSMainRPCTelemetryProjection.project(
            method: "reportClientFailure",
            report: [
                "kind": "send_queued_flush",
                "queuedMs": 1_000,
                "queueDepth": 3,
                "outcome": "expired",
                "cause": "SAND-E0707",
            ]
        )
        XCTAssertEqual(queued?.event, "sand.send.queued_flush")
        XCTAssertEqual(queued?.level, .warn)

        XCTAssertNil(IOSMainRPCTelemetryProjection.project(
            method: "reportClientFailure",
            report: [
                "kind": "send_queued_flush",
                "queuedMs": 1,
                "queueDepth": 1,
                "outcome": "delivered",
                "cause": "SAND-E0707",
            ]
        ))
    }

    func testClientPersistenceAndConnectorFailureTelemetryArePrivacyBounded() {
        let persistence = IOSMainRPCTelemetryProjection.project(
            method: "reportClientFailure",
            report: [
                "kind": "client_persistence",
                "op": "load",
                "outcome": "io_error",
                "slice": "composer-drafts",
                "bytes": 42,
                "error": ["code": "SAND-E0701", "errno": "ENOSPC"],
            ]
        )
        XCTAssertEqual(persistence?.event, "sand.client_persistence")
        XCTAssertEqual(persistence?.level, .warn)
        XCTAssertEqual(persistence?.metadata["error_code"], "SAND-E0701")
        XCTAssertEqual(persistence?.metadata["error_domain"], "storage")
        XCTAssertEqual(persistence?.metadata["error_retryable"], "true")
        XCTAssertEqual(persistence?.metadata["errno"], "ENOSPC")

        let connector = IOSMainRPCTelemetryProjection.project(
            method: "reportClientFailure",
            report: [
                "kind": "connector_auth_clicked",
                "serverName": "GitHub",
                "serverId": "server-1",
            ]
        )
        XCTAssertEqual(connector?.event, "sand.connector_auth")
        XCTAssertEqual(connector?.metadata["connector"], "github")
        XCTAssertEqual(connector?.metadata["surface"], "ios")

        XCTAssertNil(IOSMainRPCTelemetryProjection.project(
            method: "reportClientFailure",
            report: [
                "kind": "client_persistence",
                "op": "load",
                "outcome": "io_error",
                "slice": "unknown-slice",
                "error": ["code": "SAND-E0701"],
            ]
        ))
    }

    func testShippingMainRuntimeInterceptsNativeMainRPCTelemetryBeforeHost() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("IOSMainRPCTelemetryProjection.routedMethods.contains(method)"))
        XCTAssertTrue(source.contains(".rendererLifecycle"))
        XCTAssertTrue(source.contains("requires a valid telemetry report"))
    }

    func testShippingMainRuntimeInterceptsBroadTelemetryBeforeHost() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("IOSCoordinatorBroadTelemetryProjection.routedMethods.contains(method)"))
        XCTAssertTrue(source.contains("lifecycleReporter.report("))
        XCTAssertTrue(source.contains("lifecycleRecovery.markResyncCompleted()"))
        XCTAssertTrue(source.contains("return .ok(.object([:]))"))
    }
}

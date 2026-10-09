import XCTest
@testable import Fabushi

private struct ObservabilityRateLimitError: ConnectErrorLike {
    let connectCode: Int
    let connectMetadata: [String: String]
}

private actor StructuredLogSubmitGate {
    private var continuation: CheckedContinuation<StructuredLogReceipt, Never>?
    private var suspended = false

    func submit(_ logs: [StructuredLogEntry]) async -> StructuredLogReceipt {
        await withCheckedContinuation { continuation in
            self.continuation = continuation
            self.suspended = true
        }
    }

    func waitUntilSuspended() async {
        while !suspended {
            await Task.yield()
        }
    }

    func resume(processed: Int, dropped: Int = 0) {
        let continuation = self.continuation
        self.continuation = nil
        self.suspended = false
        continuation?.resume(returning: .init(logsProcessed: processed, logsDropped: dropped))
    }
}

private actor StructuredLogRecorder {
    private var batches: [[StructuredLogEntry]] = []

    func submit(_ logs: [StructuredLogEntry]) -> StructuredLogReceipt {
        batches.append(logs)
        return .init(logsProcessed: logs.count, logsDropped: 0)
    }

    func firstBatch() -> [StructuredLogEntry] {
        batches.first ?? []
    }
}

final class SharedObservabilityParityTests: XCTestCase {
    func testHeapMetricsRejectInvalidCoreValuesAndKeepsValidCounts() {
        XCTAssertNil(parseHeapMetricsReport(["usedBytes": -1, "limitBytes": 100]))
        XCTAssertNil(parseHeapMetricsReport(["usedBytes": 1, "limitBytes": 0]))
        let parsed = parseHeapMetricsReport([
            "usedBytes": 10,
            "limitBytes": 100,
            "loadedAgents": 2,
            "loadedTranscriptEntries": 9,
            "idleMinutesLast15m": -1,
        ])
        XCTAssertEqual(parsed?.usedBytes, 10)
        XCTAssertEqual(parsed?.loadedAgents, 2)
        XCTAssertNil(parsed?.idleMinutesLast15m)
    }

    func testLogShipDelayBackoffRetryAfterAndBatchLimits() {
        XCTAssertEqual(nextLogShipDelayMs(outcome: "shipped", streak: 0, random: { 0 }), 15_000)
        XCTAssertEqual(nextLogShipDelayMs(outcome: "failed", streak: 1, random: { 0 }), 30_000)
        XCTAssertEqual(
            nextLogShipDelayMs(outcome: "rate_limited", streak: 1, retryAfterMs: 90_000, random: { 0 }),
            90_000
        )
        let buffer = (0..<130).map { LogShipBufferedEntry(message: "m\($0)", metadata: [:]) }
        let split = takeLogShipBatch(buffer)
        XCTAssertEqual(split.batch.count, 128)
        XCTAssertEqual(split.remaining.count, 2)
    }

    func testLogShipBatchMatchesJavaScriptUTF16StringLength() {
        let prefixCount = LOG_SHIP_MAX_BATCH_BYTES - 3
        let buffer = [
            LogShipBufferedEntry(message: String(repeating: "a", count: prefixCount), metadata: [:]),
            LogShipBufferedEntry(message: "😀", metadata: [:]),
            LogShipBufferedEntry(message: "佛", metadata: [:]),
        ]
        let split = takeLogShipBatch(buffer)
        XCTAssertEqual("😀".utf16.count, 2)
        XCTAssertEqual("佛".utf16.count, 1)
        XCTAssertEqual(split.batch.count, 2)
        XCTAssertEqual(split.remaining.count, 1)
    }

    func testLogShipScheduleUsesConnectRetryAfter() {
        let schedule = LogShipSchedule()
        let error = ObservabilityRateLimitError(
            connectCode: CONNECT_CODE_RESOURCE_EXHAUSTED,
            connectMetadata: ["retry-after": "2"]
        )
        schedule.record(.failed(error), nowMs: 1_000, random: { 0 })
        XCTAssertTrue(schedule.isBackingOff())
        XCTAssertFalse(schedule.isDue(eager: true, nowMs: 2_999))
    }

    func testRequestLineageStripsHeaderNewlines() {
        let headers = buildSandRequestLineageHeaders(.init(
            parentRequestId: "p\r\n1",
            rootParentRequestId: "root\n2",
            parentAgentToolCallId: "tool\r3"
        ))
        XCTAssertEqual(headers["x-parent-request-id"], "p1")
        XCTAssertEqual(headers["x-root-parent-request-id"], "root2")
        XCTAssertEqual(headers["x-parent-agent-tool-call-id"], "tool3")
    }

    func testTraceparentParsingSamplingAndChildDerivation() {
        let parent = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01"
        let parsed = parseTraceparent(parent)
        XCTAssertEqual(parsed?.traceId, "0123456789abcdef0123456789abcdef")
        XCTAssertEqual(parsed?.traceFlags, 1)
        XCTAssertNil(parseTraceparent("00-00000000000000000000000000000000-0123456789abcdef-01"))
        XCTAssertFalse(shouldSampleSend(ratio: 0, random: { 0 }))
        XCTAssertTrue(shouldSampleSend(ratio: 0.5, random: { 0.4 }))
        XCTAssertTrue(deriveChildTraceparent(parent)?.traceparent.hasSuffix("-01") == true)
    }

    func testSentryPrivacyTierAndTelemetryVocabulary() {
        XCTAssertEqual(sandSentryPrivacyTierForMode(.usageDataTrainingAllowed), .full)
        XCTAssertEqual(sandSentryPrivacyTierForMode(.noTraining), .scrubbed)
        XCTAssertEqual(sandSentryPrivacyTierForMode(.noStorage), .fatalMetadata)
        XCTAssertEqual(TELEMETRY_DROP_UNIT_BY_REASON["ship_failed"], "attempts")
        XCTAssertEqual(TURN_USAGE_SCHEMA_VERSION, "2")
        XCTAssertTrue(WEBAUTHN_SIGN_ERROR_CLASSES.contains("platform_api"))
        XCTAssertTrue(isSandClientPersistenceSlice("send-journal"))
        XCTAssertFalse(isSandClientPersistenceSlice("unknown-slice"))
        XCTAssertEqual(SAND_QUEUED_FLUSH_CAUSE_CODES["ackExpired"], "SAND-E0707")
    }

    func testSentryGateScrubsUserContentAndRejectsUnknownProcess() {
        let envelope = SandSentryEnvelope(
            header: ["event_id": "0123456789abcdef0123456789abcdef"],
            items: [
                .init(
                    header: ["type": "event"],
                    payload: [
                        "event_id": "0123456789abcdef0123456789abcdef",
                        "user": ["id": "account-1", "email": "secret@example.com"],
                        "tags": [
                            "event.process": "host",
                            "safe": "bounded",
                            "token": "secret-value",
                        ],
                        "request": [
                            "method": "POST",
                            "url": "https://example.com/private?q=1",
                        ],
                        "exception": [
                            "values": [[
                                "type": "RuntimeError",
                                "value": "user private exception text",
                                "stacktrace": [
                                    "frames": [[
                                        "filename": "/Users/person/private/file.swift",
                                        "function": "unsafe user@example.com",
                                    ]]
                                ],
                            ]]
                        ],
                    ]
                )
            ]
        )

        let gate = SandSentryPrivacyGate()
        gate.setTier(.scrubbed)
        gate.setUserID("account-1")
        let projected = gate.handle(envelope)
        let event = projected?.items.first?.payload as? [String: Any]
        let user = event?["user"] as? [String: Any]
        let request = event?["request"] as? [String: Any]
        let exception = ((event?["exception"] as? [String: Any])?["values"] as? [[String: Any]])?.first
        let frame = (((exception?["stacktrace"] as? [String: Any])?["frames"] as? [[String: Any]])?.first)

        XCTAssertEqual(user?["id"] as? String, "account-1")
        XCTAssertNil(user?["email"])
        XCTAssertEqual(request?["url"] as? String, "<REDACTED: url>")
        XCTAssertEqual(exception?["value"] as? String, "<REDACTED: exception-message>")
        XCTAssertEqual(frame?["filename"] as? String, "<REDACTED: user-file-path>")

        let unknown = SandSentryEnvelope(
            header: [:],
            items: [.init(header: ["type": "event"], payload: ["tags": ["event.process": "unknown"]])]
        )
        XCTAssertNil(gate.handle(unknown))
    }

    func testFatalMetadataDropsNonFatalEventsAndRetainsOnlyFatalSafeContexts() {
        let nonFatal = SandSentryEnvelope(
            header: [:],
            items: [.init(
                header: ["type": "event"],
                payload: [
                    "level": "error",
                    "event_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "contexts": ["app": ["app_version": "1.2.3"]],
                ]
            )]
        )
        XCTAssertNil(projectSandSentryEnvelope(nonFatal, tier: .fatalMetadata))

        let fatal = SandSentryEnvelope(
            header: [:],
            items: [.init(
                header: ["type": "event"],
                payload: [
                    "level": "fatal",
                    "event_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "user": ["id": "account-1"],
                    "request": ["method": "POST", "url": "https://secret.example/path"],
                    "contexts": [
                        "app": ["app_version": "1.2.3", "app_memory": 42],
                        "os": ["name": "iOS", "version": "18.0"],
                        "device": ["family": "iPhone"],
                    ],
                    "tags": ["event.process": "host", "private.tag": "drop-me"],
                ]
            )]
        )
        let projected = projectSandSentryEnvelope(fatal, tier: .fatalMetadata)
        let event = projected?.items.first?.payload as? [String: Any]
        let contexts = event?["contexts"] as? [String: Any]
        XCTAssertEqual(event?["level"] as? String, "fatal")
        XCTAssertNil(event?["user"])
        XCTAssertNil(event?["request"])
        XCTAssertNotNil(contexts?["app"])
        XCTAssertNotNil(contexts?["os"])
        XCTAssertNil(contexts?["device"])
        XCTAssertEqual((event?["tags"] as? [String: Any])?["event.process"] as? String, "host")
        XCTAssertNil((event?["tags"] as? [String: Any])?["private.tag"])
    }

    func testScrubbedSentryProjectsThreadsSessionsAndClientReportsWithinBounds() {
        let envelope = SandSentryEnvelope(
            header: ["event_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "sent_at": "2026-10-09T00:00:00Z"],
            items: [
                .init(
                    header: ["type": "event"],
                    payload: [
                        "level": "error",
                        "threads": [
                            "values": [[
                                "id": "thread-1",
                                "main": true,
                                "stacktrace": ["frames": [["filename": "node:runtime/main"]]],
                            ]]
                        ],
                        "sdk": [
                            "name": "sentry.swift",
                            "version": "1.0.0",
                            "integrations": ["Crash", "Network"],
                        ],
                        "contexts": [
                            "runtime": ["name": "swift", "version": "6.0"],
                            "culture": ["locale": "en_US", "timezone": "America/Phoenix"],
                        ],
                    ]
                ),
                .init(
                    header: ["type": "session"],
                    payload: [
                        "init": true,
                        "sid": "session-1",
                        "started": "2026-10-09T00:00:00Z",
                        "status": "crashed",
                        "errors": 1,
                        "did": "must-not-project",
                        "attrs": ["release": "1.0.0", "environment": "production"],
                    ]
                ),
                .init(
                    header: ["type": "sessions"],
                    payload: [
                        "attrs": ["release": "1.0.0"],
                        "aggregates": [["started": "2026-10-09", "exited": 1, "errored": 2, "crashed": 3]],
                    ]
                ),
                .init(
                    header: ["type": "client_report"],
                    payload: [
                        "timestamp": 42,
                        "discarded_events": [["reason": "queue_overflow", "category": "error", "quantity": 2]],
                    ]
                ),
            ]
        )
        let projected = projectSandSentryEnvelope(envelope, tier: .scrubbed)
        XCTAssertEqual(projected?.items.count, 4)

        let event = projected?.items[0].payload as? [String: Any]
        let threadValues = (event?["threads"] as? [String: Any])?["values"] as? [[String: Any]]
        XCTAssertEqual(threadValues?.first?["id"] as? String, "thread-1")
        XCTAssertNotNil(event?["sdk"])
        XCTAssertNotNil(event?["contexts"])

        let session = projected?.items[1].payload as? [String: Any]
        XCTAssertEqual(session?["status"] as? String, "crashed")
        XCTAssertNil(session?["did"])

        let sessions = projected?.items[2].payload as? [String: Any]
        XCTAssertEqual((sessions?["aggregates"] as? [[String: Any]])?.count, 1)

        let report = projected?.items[3].payload as? [String: Any]
        XCTAssertEqual((report?["discarded_events"] as? [[String: Any]])?.count, 1)
    }

    func testSentryAccountMismatchFallsBackToFatalMetadataAndEventRingIsBounded() {
        let envelope = SandSentryEnvelope(
            header: [:],
            items: [.init(
                header: ["type": "event"],
                payload: [
                    "event_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "user": ["id": "other-account"],
                    "tags": [
                        "event.process": "host",
                        "private.tag": "should-drop",
                    ],
                    "request": ["method": "GET", "url": "app:///safe"],
                ]
            )]
        )

        let gate = SandSentryPrivacyGate()
        gate.setTier(.full)
        gate.setUserID("account-1")
        let projected = gate.handle(envelope)
        let event = projected?.items.first?.payload as? [String: Any]
        let tags = event?["tags"] as? [String: Any]
        XCTAssertEqual(tags?["event.process"] as? String, "host")
        XCTAssertNil(tags?["private.tag"])
        XCTAssertNil(event?["request"])

        let ring = SandSentryEventIDRing(capacity: 2)
        ring.record(envelope)
        ring.record(.init(
            header: [:],
            items: [
                .init(header: ["type": "event"], payload: ["event_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]),
                .init(header: ["type": "event"], payload: ["event_id": "cccccccccccccccccccccccccccccccc"]),
            ]
        ))
        XCTAssertEqual(ring.ids, [
            "cccccccccccccccccccccccccccccccc",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ])
    }

    func testStructuredLogHelpersValidateReceiptsAndMetadata() {
        XCTAssertTrue(isValidLogShipReceipt(.init(logsProcessed: 2, logsDropped: 1), requestSize: 3))
        XCTAssertFalse(isValidLogShipReceipt(.init(logsProcessed: 2, logsDropped: 2), requestSize: 3))
        XCTAssertEqual(
            cleanStructuredLogMetadata(["keep": "value", "empty": "", "missing": nil]),
            ["keep": "value"]
        )
        XCTAssertEqual(truncateStructuredLogValue("abcdef", max: 3), "abc")
        XCTAssertEqual(toClientLogLevel(.warn), .warn)
        XCTAssertEqual(DESKTOP_STRUCTURED_LOG_SPILL_MAX_ENTRIES, 1_000)
    }

    func testStructuredLogTransportHoldsForIdentityThenShipsTypedBatch() async {
        let recorder = StructuredLogRecorder()
        let transport = StructuredLogTransport(
            key: "sand",
            platformTags: ["platform": "ios"],
            holdForIdentity: true,
            submit: { logs in
                await recorder.submit(logs)
            }
        )

        await transport.enqueue(.error, message: "boom", metadata: ["feature": "chat"], timestampMs: 1_000)
        let heldFlush = await transport.flushNow(nowMs: 1_000)
        XCTAssertFalse(heldFlush)
        await transport.setIdentityTags(["account": "account-1"])
        let deliveredFlush = await transport.flushNow(nowMs: 1_000)
        XCTAssertTrue(deliveredFlush)

        let batch = await recorder.firstBatch()
        XCTAssertEqual(batch.count, 1)
        XCTAssertEqual(batch.first?.level, .error)
        XCTAssertEqual(batch.first?.metadata["platform"], "ios")
        XCTAssertEqual(batch.first?.metadata["feature"], "chat")
        XCTAssertEqual(batch.first?.metadata["account"], "account-1")
        XCTAssertEqual(batch.first?.key, "sand")
    }

    func testStructuredLogClearPendingFencesStaleInFlightCompletion() async {
        let gate = StructuredLogSubmitGate()
        let transport = StructuredLogTransport(
            key: "sand",
            platformTags: [:],
            submit: { logs in
                await gate.submit(logs)
            }
        )

        await transport.enqueue(.info, message: "old", timestampMs: 1_000)
        let oldFlush = Task { await transport.flushNow(nowMs: 1_000) }
        await gate.waitUntilSuspended()

        await transport.clearPending()
        await transport.enqueue(.info, message: "new", timestampMs: 2_000)
        await gate.resume(processed: 1)
        let oldResult = await oldFlush.value
        XCTAssertTrue(oldResult)

        let checkpoint = await transport.captureCheckpoint()
        XCTAssertEqual(checkpoint.records.map(\.message), ["new"])
        XCTAssertTrue(checkpoint.counters.values.allSatisfy { $0.observed == 0 && $0.acknowledgedThrough == 0 })
    }

    func testStructuredLogOverflowPrefersHostAndBoxDiagnosticEntries() async {
        let transport = StructuredLogTransport(
            key: "sand",
            platformTags: [:],
            submit: { logs in
                .init(logsProcessed: logs.count, logsDropped: 0)
            }
        )

        await transport.enqueue(.info, message: "keep-oldest", timestampMs: 1)
        await transport.enqueue(.info, message: HOST_LOG_EVENT, timestampMs: 2)
        for index in 0..<(MAX_BUFFER_SIZE - 1) {
            await transport.enqueue(.info, message: "regular-\(index)", timestampMs: Int64(index + 3))
        }

        let checkpoint = await transport.captureCheckpoint()
        XCTAssertEqual(checkpoint.records.count, MAX_BUFFER_SIZE)
        XCTAssertTrue(checkpoint.records.contains { $0.message == "keep-oldest" })
        XCTAssertFalse(checkpoint.records.contains { $0.message == HOST_LOG_EVENT })
        XCTAssertEqual(checkpoint.counters["overflow_evicted"]?.observed, 1)
    }

    func testStructuredLogTransportExpiresReplayAndCapsOverflow() async {
        let transport = StructuredLogTransport(
            key: "sand",
            platformTags: [:],
            submit: { logs in
                .init(logsProcessed: logs.count, logsDropped: 0)
            }
        )

        await transport.enqueue(.info, message: "expired", timestampMs: 1)
        let expiredFlush = await transport.flushNow(nowMs: Int64(STRUCTURED_LOG_REPLAY_MAX_AGE_MS) + 2)
        XCTAssertTrue(expiredFlush)
        var checkpoint = await transport.captureCheckpoint()
        XCTAssertEqual(checkpoint.records.count, 0)
        XCTAssertEqual(checkpoint.counters["replay_expired"]?.observed, 1)

        for index in 0..<(MAX_BUFFER_SIZE + 2) {
            await transport.enqueue(.debug, message: "m\(index)", timestampMs: 10_000)
        }
        checkpoint = await transport.captureCheckpoint()
        XCTAssertEqual(checkpoint.records.count, MAX_BUFFER_SIZE)
        XCTAssertEqual(checkpoint.counters["overflow_evicted"]?.observed, 2)
    }
}

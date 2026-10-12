import XCTest
@testable import Fabushi

final class AttachmentMainRPCParityTests: XCTestCase {
    @MainActor
    func testStageContainmentAccountFenceCommitAndDiscard() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-attachment-rpc-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        var scope: String? = "account-a"
        var hostCalls: [(String, [String: Any])] = []
        let adapter = try IOSAttachmentMainRPCAdapter(
            appDataDirectory: root,
            currentAccountScope: { scope },
            hostRequest: { method, params in
                hostCalls.append((method, params))
                return [
                    "path": "/canonical/agents/agent-a/attachments/hash.txt",
                    "totalSize": 5,
                    "bytesBase64": Data("hello".utf8).base64EncodedString(),
                    "kind": "text",
                    "text": "hello",
                    "dataUrl": "data:image/png;base64,aGVsbG8=",
                ]
            }
        )

        let staged = try await adapter.route(
            method: "stageAttachmentBytes",
            args: try CoordinatorPayload.fromFoundation([
                "agentId": "agent-a",
                "filename": "note.txt",
                "bytesBase64": Data("hello".utf8).base64EncodedString(),
            ])
        )
        guard case .ok(let stagedPayload)? = staged,
              let row = stagedPayload.foundationValue as? [String: Any],
              row["ok"] as? Bool == true,
              let path = row["path"] as? String
        else { return XCTFail("expected staged path") }
        XCTAssertTrue(path.contains("attachment-staging"))
        XCTAssertTrue(FileManager.default.fileExists(atPath: path))

        let committed = try await adapter.route(
            method: "commitStagedAttachments",
            args: try CoordinatorPayload.fromFoundation([
                "paths": [path],
                "filenames": ["note.txt"],
            ])
        )
        guard case .ok(let committedPayload)? = committed else {
            return XCTFail("expected committed attachment")
        }
        XCTAssertEqual(
            committedPayload.foundationValue as? [String],
            ["/canonical/agents/agent-a/attachments/hash.txt"]
        )
        XCTAssertEqual(hostCalls.first?.0, "feature.attachment.uploadDirect")
        XCTAssertEqual(hostCalls.first?.1["agentId"] as? String, "agent-a")

        scope = "account-b"
        do {
            _ = try await adapter.route(
                method: "commitStagedAttachments",
                args: try CoordinatorPayload.fromFoundation([
                    "paths": [path],
                    "filenames": ["note.txt"],
                ])
            )
            XCTFail("retired account scope must not commit staged bytes")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("retired account"))
        }

        _ = try await adapter.route(
            method: "discardStagedAttachment",
            args: try CoordinatorPayload.fromFoundation(["path": path])
        )
        XCTAssertFalse(FileManager.default.fileExists(atPath: path))
    }

    @MainActor
    func testRestartPurgesStaleNativeStaging() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-attachment-restart-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let first = try IOSAttachmentMainRPCAdapter(
            appDataDirectory: root,
            currentAccountScope: { "account-a" },
            hostRequest: { _, _ in [String: Any]() }
        )
        let staged = try await first.route(
            method: "stageAttachmentBytes",
            args: try CoordinatorPayload.fromFoundation([
                "agentId": "agent-a",
                "filename": "restart.txt",
                "bytesBase64": Data("stale".utf8).base64EncodedString(),
            ])
        )
        guard case .ok(let stagedPayload)? = staged,
              let row = stagedPayload.foundationValue as? [String: Any],
              let path = row["path"] as? String
        else { return XCTFail("expected staged path") }
        XCTAssertTrue(FileManager.default.fileExists(atPath: path))

        _ = try IOSAttachmentMainRPCAdapter(
            appDataDirectory: root,
            currentAccountScope: { "account-a" },
            hostRequest: { _, _ in [String: Any]() }
        )
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: path),
            "native-main recreation must not resurrect half-staged transfers"
        )
    }

    @MainActor
    func testPreviewCapBoundedReadsAndRemoteImageFallback() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-attachment-read-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        var methods: [String] = []
        let bytes = Data("hello".utf8)
        let adapter = try IOSAttachmentMainRPCAdapter(
            appDataDirectory: root,
            currentAccountScope: { "account-a" },
            hostRequest: { method, _ in
                methods.append(method)
                switch method {
                case "feature.attachment.readChunkDirect":
                    return [
                        "path": "/canonical/file.txt",
                        "bytesBase64": bytes.base64EncodedString(),
                        "totalSize": bytes.count,
                        "mime": "text/plain",
                    ]
                case "feature.attachment.readImageDirect":
                    return [
                        "path": "/canonical/image.png",
                        "dataUrl": "data:image/png;base64,aGVsbG8=",
                        "width": 1,
                        "height": 1,
                    ]
                default:
                    return [:]
                }
            }
        )

        let read = try await adapter.route(
            method: "readAttachmentBytes",
            args: try CoordinatorPayload.fromFoundation([
                "agentId": "agent-a",
                "path": "/canonical/file.txt",
                "maxBytes": IOSAttachmentMainRPCAdapter.previewByteCap,
            ])
        )
        guard case .ok(let readPayload)? = read,
              let readRow = readPayload.foundationValue as? [String: Any]
        else { return XCTFail("expected bytes result") }
        XCTAssertEqual(readRow["kind"] as? String, "bytes")
        XCTAssertEqual(
            Data(base64Encoded: readRow["bytesBase64"] as? String ?? ""),
            bytes
        )

        let media = try await adapter.route(
            method: "resolveAttachmentMedia",
            args: try CoordinatorPayload.fromFoundation([
                "agentId": "agent-a",
                "source": "/canonical/image.png",
            ])
        )
        guard case .ok(let mediaPayload)? = media,
              let mediaRow = mediaPayload.foundationValue as? [String: Any]
        else { return XCTFail("expected image projection") }
        XCTAssertEqual(mediaRow["kind"] as? String, "image")
        XCTAssertTrue(methods.contains("feature.attachment.readImageDirect"))
    }

    @MainActor
    func testPreviewRejectsOversizedPayloadBeforeStreaming() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-attachment-cap-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        var chunkCalls = 0
        let adapter = try IOSAttachmentMainRPCAdapter(
            appDataDirectory: root,
            currentAccountScope: { "account-a" },
            hostRequest: { method, _ in
                guard method == "feature.attachment.readChunkDirect" else {
                    return [String: Any]()
                }
                chunkCalls += 1
                return [
                    "bytesBase64": Data([0x61]).base64EncodedString(),
                    "totalSize": IOSAttachmentMainRPCAdapter.previewByteCap + 1,
                ]
            }
        )

        let result = try await adapter.route(
            method: "readAttachmentBytes",
            args: try CoordinatorPayload.fromFoundation([
                "agentId": "agent-a",
                "path": "/canonical/too-large.bin",
                "maxBytes": IOSAttachmentMainRPCAdapter.previewByteCap,
            ])
        )
        guard case .ok(let payload)? = result,
              let row = payload.foundationValue as? [String: Any]
        else { return XCTFail("expected too-large result") }
        XCTAssertEqual(row["kind"] as? String, "too-large")
        XCTAssertEqual(row["size"] as? Int, IOSAttachmentMainRPCAdapter.previewByteCap + 1)
        XCTAssertEqual(chunkCalls, 1, "oversized preview must stop after the size probe")
    }

    @MainActor
    func testGatewayReadsNeverExceedFourMiBChunks() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-attachment-chunks-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let totalSize = IOSAttachmentMainRPCAdapter.gatewayReadChunkBytes + 1
        var requestedLengths: [Int] = []
        let adapter = try IOSAttachmentMainRPCAdapter(
            appDataDirectory: root,
            currentAccountScope: { "account-a" },
            hostRequest: { method, params in
                guard method == "feature.attachment.readChunkDirect" else {
                    return [String: Any]()
                }
                let offset = params["offset"] as? Int ?? 0
                let length = params["length"] as? Int ?? 0
                requestedLengths.append(length)
                let emitted = max(0, min(length, totalSize - offset))
                return [
                    "bytesBase64": Data(repeating: 0x61, count: emitted).base64EncodedString(),
                    "totalSize": totalSize,
                ]
            }
        )

        let result = try await adapter.route(
            method: "readAttachmentBytes",
            args: try CoordinatorPayload.fromFoundation([
                "agentId": "agent-a",
                "path": "/canonical/chunked.bin",
                "maxBytes": IOSAttachmentMainRPCAdapter.previewByteCap,
            ])
        )
        guard case .ok(let payload)? = result,
              let row = payload.foundationValue as? [String: Any]
        else { return XCTFail("expected bytes result") }
        XCTAssertEqual(row["kind"] as? String, "bytes")
        XCTAssertEqual(
            requestedLengths,
            [1, IOSAttachmentMainRPCAdapter.gatewayReadChunkBytes, 1]
        )
        XCTAssertLessThanOrEqual(
            requestedLengths.max() ?? 0,
            IOSAttachmentMainRPCAdapter.gatewayReadChunkBytes
        )
    }

    @MainActor
    func testDownloadCancellationAndIncompleteTransferDeletePartialFiles() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-attachment-download-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let downloadRoot = root.appendingPathComponent("attachment-downloads", isDirectory: true)

        var cancelledCalls = 0
        let cancelled = try IOSAttachmentMainRPCAdapter(
            appDataDirectory: root,
            currentAccountScope: { "account-a" },
            hostRequest: { method, _ in
                guard method == "feature.attachment.readChunkDirect" else {
                    return [String: Any]()
                }
                cancelledCalls += 1
                if cancelledCalls == 1 {
                    return [
                        "bytesBase64": Data([0x61]).base64EncodedString(),
                        "totalSize": 5,
                    ]
                }
                throw CancellationError()
            }
        )
        do {
            _ = try await cancelled.route(
                method: "downloadAttachment",
                args: try CoordinatorPayload.fromFoundation([
                    "agentId": "agent-a",
                    "path": "/canonical/cancelled.txt",
                    "suggestedName": "cancelled.txt",
                ])
            )
            XCTFail("cancelled Host transfer must propagate cancellation")
        } catch is CancellationError {
            // Expected: IOSMainRuntime translates this to coordinator-cancelled.
        }
        XCTAssertTrue(
            try FileManager.default.contentsOfDirectory(
                at: downloadRoot,
                includingPropertiesForKeys: nil
            ).isEmpty,
            "cancelled download must not leave a .partial file"
        )

        var incompleteCalls = 0
        let incomplete = try IOSAttachmentMainRPCAdapter(
            appDataDirectory: root,
            currentAccountScope: { "account-a" },
            hostRequest: { method, _ in
                guard method == "feature.attachment.readChunkDirect" else {
                    return [String: Any]()
                }
                incompleteCalls += 1
                if incompleteCalls == 1 {
                    return [
                        "bytesBase64": Data([0x61]).base64EncodedString(),
                        "totalSize": 5,
                    ]
                }
                return [
                    "bytesBase64": "",
                    "totalSize": 5,
                ]
            }
        )
        let result = try await incomplete.route(
            method: "downloadAttachment",
            args: try CoordinatorPayload.fromFoundation([
                "agentId": "agent-a",
                "path": "/canonical/incomplete.txt",
                "suggestedName": "incomplete.txt",
            ])
        )
        guard case .ok(let payload)? = result else {
            return XCTFail("expected failed download result")
        }
        XCTAssertEqual(payload.foundationValue as? Bool, false)
        XCTAssertTrue(
            try FileManager.default.contentsOfDirectory(
                at: downloadRoot,
                includingPropertiesForKeys: nil
            ).isEmpty,
            "incomplete download must remove temporary output"
        )
    }

    func testShippingMainRuntimeOwnsAttachmentAdapterBeforeCoordinatorFallback() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let main = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(main.contains("attachmentMainRPC.route(method: method, args: args)"))
        XCTAssertTrue(main.contains("feature.attachment.*Direct") == false)
        XCTAssertTrue(main.contains("return try await coordinator.request(method: method, params: params).value"))
    }
}

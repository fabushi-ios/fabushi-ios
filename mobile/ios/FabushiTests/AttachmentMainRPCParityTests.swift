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

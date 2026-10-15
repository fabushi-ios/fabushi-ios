import Foundation
import XCTest
@testable import Fabushi

private func cloudAgentAppendVarint(_ value: UInt64, to data: inout Data) {
    var value = value
    repeat {
        var byte = UInt8(value & 0x7f)
        value >>= 7
        if value != 0 { byte |= 0x80 }
        data.append(byte)
    } while value != 0
}

private func cloudAgentVarint(_ field: Int, _ value: UInt64) -> Data {
    var data = Data()
    cloudAgentAppendVarint(UInt64(field << 3), to: &data)
    cloudAgentAppendVarint(value, to: &data)
    return data
}

private func cloudAgentBytes(_ field: Int, _ body: Data) -> Data {
    var data = Data()
    cloudAgentAppendVarint(UInt64((field << 3) | 2), to: &data)
    cloudAgentAppendVarint(UInt64(body.count), to: &data)
    data.append(body)
    return data
}

private func cloudAgentString(_ field: Int, _ value: String) -> Data {
    cloudAgentBytes(field, Data(value.utf8))
}


private enum CloudAgentParsedWireValue {
    case varint(UInt64)
    case bytes(Data)
}

private func cloudAgentReadVarint(_ data: Data, offset: inout Int) throws -> UInt64 {
    var value: UInt64 = 0
    var shift: UInt64 = 0
    while offset < data.count, shift < 64 {
        let byte = data[offset]
        offset += 1
        value |= UInt64(byte & 0x7f) << shift
        if byte & 0x80 == 0 { return value }
        shift += 7
    }
    throw NSError(domain: "CursorCloudAgentInfoParityTests", code: 1)
}

private func cloudAgentParsedFields(_ data: Data) throws -> [(Int, CloudAgentParsedWireValue)] {
    var offset = 0
    var fields: [(Int, CloudAgentParsedWireValue)] = []
    while offset < data.count {
        let key = try cloudAgentReadVarint(data, offset: &offset)
        let field = Int(key >> 3)
        switch key & 7 {
        case 0:
            fields.append((field, .varint(try cloudAgentReadVarint(data, offset: &offset))))
        case 2:
            let length = try cloudAgentReadVarint(data, offset: &offset)
            guard length <= UInt64(Int.max) else {
                throw NSError(domain: "CursorCloudAgentInfoParityTests", code: 2)
            }
            let count = Int(length)
            guard offset <= data.count, count <= data.count - offset else {
                throw NSError(domain: "CursorCloudAgentInfoParityTests", code: 3)
            }
            fields.append((field, .bytes(data.subdata(in: offset..<(offset + count)))))
            offset += count
        default:
            throw NSError(domain: "CursorCloudAgentInfoParityTests", code: 4)
        }
    }
    return fields
}

private func cloudAgentParsedBytes(
    _ fields: [(Int, CloudAgentParsedWireValue)],
    _ field: Int
) -> Data? {
    fields.first { $0.0 == field }.flatMap {
        if case .bytes(let value) = $0.1 { return value }
        return nil
    }
}

private func cloudAgentParsedVarint(
    _ fields: [(Int, CloudAgentParsedWireValue)],
    _ field: Int
) -> UInt64? {
    fields.first { $0.0 == field }.flatMap {
        if case .varint(let value) = $0.1 { return value }
        return nil
    }
}

private func cloudAgentParsedString(
    _ fields: [(Int, CloudAgentParsedWireValue)],
    _ field: Int
) -> String? {
    cloudAgentParsedBytes(fields, field).flatMap { String(data: $0, encoding: .utf8) }
}

final class CursorCloudAgentInfoParityTests: XCTestCase {
    func testDecodesAuthoritativeComposerBranchPRAndDiffStats() throws {
        var composer = Data()
        composer.append(cloudAgentString(5, "Refactor auth"))
        composer.append(cloudAgentString(6, "cursor/auth-refactor"))
        composer.append(cloudAgentVarint(12, 2))
        composer.append(cloudAgentString(22, "https://github.com/acme/repo/pull/42"))
        composer.append(cloudAgentVarint(25, 123))
        composer.append(cloudAgentVarint(26, 17))
        composer.append(cloudAgentVarint(27, 9))
        composer.append(cloudAgentVarint(43, 1))

        var prompt = Data()
        prompt.append(cloudAgentString(1, "Refactor the auth flow"))

        var pr = Data()
        pr.append(cloudAgentString(1, "cursor/auth-refactor"))
        pr.append(cloudAgentVarint(4, 42))
        pr.append(cloudAgentVarint(5, 2))
        pr.append(cloudAgentString(6, "https://github.com/acme/repo/pull/42"))

        var detailed = Data()
        detailed.append(cloudAgentBytes(1, composer))
        detailed.append(cloudAgentBytes(4, prompt))
        detailed.append(cloudAgentVarint(5, 2))
        detailed.append(cloudAgentString(10, "Implemented the requested changes"))
        detailed.append(cloudAgentBytes(20, pr))

        let info = try IOSCursorDashboardClient.decodeBackgroundComposerInfoForTests(
            cloudAgentBytes(1, detailed)
        )
        XCTAssertEqual(info.status, 2)
        XCTAssertEqual(info.name, "Refactor auth")
        XCTAssertEqual(info.prompt, "Refactor the auth flow")
        XCTAssertEqual(info.summary, "Implemented the requested changes")
        XCTAssertEqual(info.branchName, "cursor/auth-refactor")
        XCTAssertEqual(info.filesChanged, 9)
        XCTAssertEqual(info.linesAdded, 123)
        XCTAssertEqual(info.linesRemoved, 17)
        XCTAssertEqual(info.prURL, "https://github.com/acme/repo/pull/42")
        XCTAssertEqual(info.prState, "draft")
        XCTAssertEqual(info.prNumber, 42)
    }

    func testFallsBackToPRBranchAndPRURLWhenComposerFieldsAreEmpty() throws {
        var composer = Data()
        composer.append(cloudAgentVarint(12, 4))

        var pr = Data()
        pr.append(cloudAgentString(1, "cursor/fallback"))
        pr.append(cloudAgentVarint(4, 7))
        pr.append(cloudAgentVarint(5, 3))
        pr.append(cloudAgentString(6, "https://github.com/acme/repo/pull/7"))

        var detailed = Data()
        detailed.append(cloudAgentBytes(1, composer))
        detailed.append(cloudAgentVarint(5, 4))
        detailed.append(cloudAgentBytes(20, pr))

        let info = try IOSCursorDashboardClient.decodeBackgroundComposerInfoForTests(
            cloudAgentBytes(1, detailed)
        )
        XCTAssertEqual(info.branchName, "cursor/fallback")
        XCTAssertEqual(info.prURL, "https://github.com/acme/repo/pull/7")
        XCTAssertEqual(info.prState, "merged")
        XCTAssertEqual(info.prNumber, 7)
    }

    func testCloudAgentBoxImagePolicyAcceptsWorkspaceFileAndDecodesBinaryRead() throws {
        let resolved = try IOSCloudAgentBoxImagePolicy.resolve("file:///workspace/screens/shot.png")
        XCTAssertEqual(resolved.path, "/workspace/screens/shot.png")
        XCTAssertEqual(resolved.mimeType, "image/png")

        let bytes = Data([0x89, 0x50, 0x4e, 0x47])
        let image = try IOSCloudAgentBoxImagePolicy.decode(
            .object([
                "success": .bool(true),
                "dataBase64": .string(bytes.base64EncodedString()),
            ]),
            path: resolved.path,
            mimeType: resolved.mimeType
        )
        XCTAssertEqual(image.data, bytes)
        XCTAssertEqual(image.path, "/workspace/screens/shot.png")
        XCTAssertEqual(image.mimeType, "image/png")
    }

    func testCloudAgentBoxImagePolicyRejectsNonFileOutsideWorkspaceAndNonImage() {
        XCTAssertThrowsError(try IOSCloudAgentBoxImagePolicy.resolve("https://example.test/a.png"))
        XCTAssertThrowsError(try IOSCloudAgentBoxImagePolicy.resolve("file:///tmp/a.png"))
        XCTAssertThrowsError(try IOSCloudAgentBoxImagePolicy.resolve("file:///workspace/readme.txt"))
    }

    func testCloudAgentBoxImagePolicyFailsClosedWithoutBinaryReadPayload() throws {
        let resolved = try IOSCloudAgentBoxImagePolicy.resolve("file:///workspace/shot.webp")
        XCTAssertThrowsError(
            try IOSCloudAgentBoxImagePolicy.decode(
                .object([
                    "success": .bool(true),
                    "content": .string("not binary"),
                ]),
                path: resolved.path,
                mimeType: resolved.mimeType
            )
        )
    }

    func testCloudAgentTranscriptBridgeProducesCanonicalJSONLRows() throws {
        var human = Data()
        human.append(cloudAgentString(1, "hello"))
        human.append(cloudAgentVarint(2, 1))
        var tool = Data()
        tool.append(cloudAgentString(1, "call-shell"))
        tool.append(cloudAgentString(2, "Shell"))
        tool.append(cloudAgentString(5, "{\"cmd\":\"pwd\"}"))
        tool.append(cloudAgentString(7, "{\"stdout\":\"/workspace\"}"))
        tool.append(cloudAgentVarint(13, 100))
        tool.append(cloudAgentVarint(14, 160))
        var thinking = Data()
        thinking.append(cloudAgentString(1, "reasoning"))
        var assistant = Data()
        assistant.append(cloudAgentString(1, "done"))
        assistant.append(cloudAgentVarint(2, 2))
        assistant.append(cloudAgentBytes(18, tool))
        assistant.append(cloudAgentBytes(45, thinking))

        let dump = try MahayanaCloudAgentTrace.jsonLines(messages: [human, assistant])
        XCTAssertEqual(dump.rowCount, 3)
        let text = try XCTUnwrap(String(data: dump.jsonl, encoding: .utf8))
        let rows = try text.split(separator: "\n").map {
            try XCTUnwrap(JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any])
        }
        XCTAssertEqual(rows[0]["role"] as? String, "user")
        XCTAssertEqual(rows[1]["role"] as? String, "assistant")
        XCTAssertEqual(rows[1]["thinking"] as? String, "reasoning")
        let calls = try XCTUnwrap(rows[1]["tool_calls"] as? [[String: Any]])
        XCTAssertEqual(calls[0]["tool_call_id"] as? String, "call-shell")
        XCTAssertEqual((calls[0]["tool_args"] as? [String: Any])?["cmd"] as? String, "pwd")
        XCTAssertEqual((calls[0]["duration_ms"] as? NSNumber)?.intValue, 60)
        XCTAssertEqual(rows[2]["role"] as? String, "tool")
        XCTAssertEqual((rows[2]["tool_result"] as? [String: Any])?["stdout"] as? String, "/workspace")
    }

    func testLaunchRequestPreservesCanonicalWireAndCompositionFields() throws {
        let input = IOSCloudAgentLaunchInput(
            bcId: "bc-123",
            snapshotNameOrId: "snapshot-123",
            repoURL: "https://github.com/acme/repo",
            startingRef: "main",
            prompt: "Ship the change",
            images: [
                IOSCloudAgentImageInput(
                    data: Data([0x89, 0x50, 0x4e, 0x47]),
                    path: "/workspace/shot.png",
                    mimeType: "image/png"
                ),
            ],
            title: "Ship auth",
            usePrivateWorker: true,
            labels: [
                IOSCloudAgentWorkerLabel(key: "repo", value: "github.com/acme/repo"),
            ],
            teamId: 7,
            environmentName: "Saved env",
            environmentPublicId: "env-public",
            environmentRepos: [
                IOSCloudAgentEnvironmentRepo(
                    repoURL: "https://github.com/acme/repo",
                    scmRepoNodeId: "node-1",
                    gitEnterpriseUUID: "enterprise-1"
                ),
            ],
            modelId: "claude-sonnet",
            modelParams: ["effort": "high"]
        )

        let payload = try IOSCursorDashboardClient.startBackgroundComposerRequestForTests(input)
        let fields = try cloudAgentParsedFields(payload)

        XCTAssertEqual(cloudAgentParsedString(fields, 1), "snapshot-123")
        XCTAssertEqual(cloudAgentParsedString(fields, 6), "/workspace")
        XCTAssertEqual(cloudAgentParsedVarint(fields, 9), 1)
        XCTAssertEqual(cloudAgentParsedVarint(fields, 10), 1)
        XCTAssertEqual(cloudAgentParsedString(fields, 14), "https://github.com/acme/repo")
        XCTAssertEqual(cloudAgentParsedString(fields, 22), "bc-123")
        XCTAssertEqual(cloudAgentParsedVarint(fields, 23), 33)
        XCTAssertEqual(cloudAgentParsedVarint(fields, 26), 1)
        XCTAssertEqual(cloudAgentParsedString(fields, 31), "main")
        XCTAssertEqual(cloudAgentParsedVarint(fields, 37), 1)
        XCTAssertEqual(cloudAgentParsedVarint(fields, 41), 1)
        XCTAssertEqual(cloudAgentParsedString(fields, 50), "Ship auth")
        XCTAssertEqual(cloudAgentParsedVarint(fields, 55), 1)
        XCTAssertEqual(cloudAgentParsedVarint(fields, 98), 7)

        let startingPoint = try XCTUnwrap(cloudAgentParsedBytes(fields, 11))
        let startingFields = try cloudAgentParsedFields(startingPoint)
        XCTAssertEqual(cloudAgentParsedString(startingFields, 1), "https://github.com/acme/repo")
        XCTAssertEqual(cloudAgentParsedString(startingFields, 2), "main")
        XCTAssertEqual(cloudAgentParsedString(startingFields, 12), "Saved env")
        XCTAssertEqual(cloudAgentParsedString(startingFields, 15), "env-public")

        let repoConfig = try XCTUnwrap(cloudAgentParsedBytes(startingFields, 11))
        let repoConfigFields = try cloudAgentParsedFields(repoConfig)
        let repoEntry = try XCTUnwrap(cloudAgentParsedBytes(repoConfigFields, 1))
        let repoFields = try cloudAgentParsedFields(repoEntry)
        XCTAssertEqual(cloudAgentParsedString(repoFields, 1), "https://github.com/acme/repo")
        XCTAssertEqual(cloudAgentParsedString(repoFields, 2), "node-1")
        XCTAssertEqual(cloudAgentParsedString(repoFields, 3), "enterprise-1")

        let conversation = try XCTUnwrap(cloudAgentParsedBytes(fields, 44))
        let conversationFields = try cloudAgentParsedFields(conversation)
        let userAction = try XCTUnwrap(cloudAgentParsedBytes(conversationFields, 1))
        let userActionFields = try cloudAgentParsedFields(userAction)
        XCTAssertEqual(cloudAgentParsedVarint(userActionFields, 3), 1)
        let userMessage = try XCTUnwrap(cloudAgentParsedBytes(userActionFields, 1))
        let messageFields = try cloudAgentParsedFields(userMessage)
        XCTAssertEqual(cloudAgentParsedString(messageFields, 1), "Ship the change")
        XCTAssertEqual(cloudAgentParsedVarint(messageFields, 4), 1)
        let selectedContext = try XCTUnwrap(cloudAgentParsedBytes(messageFields, 3))
        let selectedContextFields = try cloudAgentParsedFields(selectedContext)
        let image = try XCTUnwrap(cloudAgentParsedBytes(selectedContextFields, 1))
        let imageFields = try cloudAgentParsedFields(image)
        XCTAssertEqual(cloudAgentParsedBytes(imageFields, 8), Data([0x89, 0x50, 0x4e, 0x47]))
        XCTAssertEqual(cloudAgentParsedString(imageFields, 3), "/workspace/shot.png")
        XCTAssertEqual(cloudAgentParsedString(imageFields, 7), "image/png")

        let model = try XCTUnwrap(cloudAgentParsedBytes(fields, 58))
        let modelFields = try cloudAgentParsedFields(model)
        XCTAssertEqual(cloudAgentParsedString(modelFields, 1), "claude-sonnet")
        XCTAssertEqual(cloudAgentParsedVarint(modelFields, 2), 1)
        let parameter = try XCTUnwrap(cloudAgentParsedBytes(modelFields, 3))
        let parameterFields = try cloudAgentParsedFields(parameter)
        XCTAssertEqual(cloudAgentParsedString(parameterFields, 1), "effort")
        XCTAssertEqual(cloudAgentParsedString(parameterFields, 2), "high")

        let label = try XCTUnwrap(cloudAgentParsedBytes(fields, 59))
        let labelFields = try cloudAgentParsedFields(label)
        XCTAssertEqual(cloudAgentParsedString(labelFields, 1), "repo")
        XCTAssertEqual(cloudAgentParsedString(labelFields, 2), "github.com/acme/repo")
    }

    func testLaunchRequestRejectsModelParametersWithoutModel() {
        let input = IOSCloudAgentLaunchInput(
            bcId: "bc-123",
            snapshotNameOrId: "snapshot-123",
            repoURL: "https://github.com/acme/repo",
            startingRef: nil,
            prompt: "Ship the change",
            images: [],
            title: nil,
            usePrivateWorker: nil,
            labels: [],
            teamId: nil,
            environmentName: nil,
            environmentPublicId: nil,
            environmentRepos: [],
            modelId: nil,
            modelParams: ["effort": "high"]
        )
        XCTAssertThrowsError(
            try IOSCursorDashboardClient.startBackgroundComposerRequestForTests(input)
        )
    }


}

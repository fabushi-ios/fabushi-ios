import Foundation

@MainActor
final class IOSAttachmentMainRPCAdapter {
    typealias HostRequest = @MainActor (_ method: String, _ params: [String: Any]) async throws -> Any
    typealias AccountScope = @MainActor () -> String?

    private struct StagedMetadata: Codable, Equatable {
        let accountScope: String
        let agentID: String
        let filename: String
    }

    private enum AdapterError: LocalizedError {
        case invalid(String)
        case unavailable(String)

        var errorDescription: String? {
            switch self {
            case .invalid(let message), .unavailable(let message):
                return message
            }
        }
    }

    static let previewByteCap = 25 * 1024 * 1024
    static let gatewayReadChunkBytes = 4 * 1024 * 1024
    static let videoByteCap = 200 * 1024 * 1024

    private let stagingRoot: URL
    private let downloadRoot: URL
    private let hostRequest: HostRequest
    private let currentAccountScope: AccountScope
    private let fileManager: FileManager

    init(
        appDataDirectory: URL,
        fileManager: FileManager = .default,
        currentAccountScope: @escaping AccountScope,
        hostRequest: @escaping HostRequest
    ) throws {
        self.fileManager = fileManager
        self.currentAccountScope = currentAccountScope
        self.hostRequest = hostRequest
        stagingRoot = appDataDirectory
            .appendingPathComponent("attachment-staging", isDirectory: true)
            .standardizedFileURL
        downloadRoot = appDataDirectory
            .appendingPathComponent("attachment-downloads", isDirectory: true)
            .standardizedFileURL

        // Desktop staging is ephemeral. Recreate the root at native-main startup
        // so process termination never resurrects a half-staged transfer.
        try? fileManager.removeItem(at: stagingRoot)
        try fileManager.createDirectory(
            at: stagingRoot,
            withIntermediateDirectories: true
        )
        try fileManager.createDirectory(
            at: downloadRoot,
            withIntermediateDirectories: true
        )
    }

    func route(
        method: String,
        args: CoordinatorPayload
    ) async throws -> CoordinatorReplyOutcome? {
        guard IOSMainRPCRuntime.isMethod(method),
              let params = args.foundationValue as? [String: Any]
        else { return nil }

        switch method {
        case "stageAttachmentBytes":
            return try success(stageAttachmentBytes(params))
        case "commitStagedAttachments":
            return try success(await commitStagedAttachments(params))
        case "discardStagedAttachment":
            discardStagedAttachment(params)
            return try success(NSNull())
        case "readAttachmentText":
            return try success(await readAttachmentText(params))
        case "readAttachmentBytes":
            return try success(await readAttachmentBytes(params))
        case "resolveAttachmentMedia":
            return try success(await resolveAttachmentMedia(params))
        case "downloadAttachment":
            return try success(await downloadAttachment(params))
        default:
            return nil
        }
    }

    private func stageAttachmentBytes(_ params: [String: Any]) throws -> [String: Any] {
        let scope = try requiredAccountScope()
        let agentID = try requiredString(params["agentId"], "agentId")
        let filename = try safeFilename(params["filename"], field: "filename")
        guard let bytes = attachmentBytes(params) else {
            throw AdapterError.invalid("bytes or bytesBase64 is required")
        }
        guard !bytes.isEmpty else {
            return ["ok": false, "reason": "empty"]
        }
        guard bytes.count <= Self.byteLimit(for: filename) else {
            return ["ok": false, "reason": "too-large"]
        }

        let token = UUID().uuidString.lowercased()
        let ext = URL(fileURLWithPath: filename).pathExtension
        let stagedName = ext.isEmpty ? token : "\(token).\(ext.lowercased())"
        let staged = stagingRoot.appendingPathComponent(stagedName, isDirectory: false)
        guard isWithinStagingRoot(staged) else {
            throw AdapterError.invalid("staging path escaped native staging root")
        }

        let metadata = StagedMetadata(
            accountScope: scope,
            agentID: agentID,
            filename: filename
        )
        try bytes.write(to: staged, options: [.atomic])
        do {
            let encoded = try JSONEncoder().encode(metadata)
            try encoded.write(to: metadataURL(for: staged), options: [.atomic])
        } catch {
            try? fileManager.removeItem(at: staged)
            throw error
        }
        return ["ok": true, "path": staged.path]
    }

    private func commitStagedAttachments(_ params: [String: Any]) async throws -> [String]? {
        let scope = try requiredAccountScope()
        let rawPaths = stringArray(params["paths"])
        let rawNames = stringArray(params["filenames"])
        guard !rawPaths.isEmpty, rawPaths.count == rawNames.count else { return nil }

        var committed: [String] = []
        committed.reserveCapacity(rawPaths.count)
        for index in rawPaths.indices {
            try Task.checkCancellation()
            let staged = URL(fileURLWithPath: rawPaths[index]).standardizedFileURL
            guard isWithinStagingRoot(staged) else { return nil }
            let metadata = try loadMetadata(for: staged)
            guard metadata.accountScope == scope else {
                throw AdapterError.unavailable("staged attachment belongs to a retired account scope")
            }
            let filename = try safeFilename(rawNames[index], field: "filename")
            guard filename == metadata.filename else {
                throw AdapterError.invalid("staged filename does not match its native staging token")
            }
            let bytes = try Data(contentsOf: staged, options: [.mappedIfSafe])
            guard !bytes.isEmpty, bytes.count <= Self.byteLimit(for: filename) else { return nil }

            let result = try await hostRequest(
                "feature.attachment.uploadDirect",
                [
                    "agentId": metadata.agentID,
                    "filename": filename,
                    "bytesBase64": bytes.base64EncodedString(),
                ]
            )
            guard let row = result as? [String: Any],
                  let path = row["path"] as? String,
                  !path.isEmpty
            else {
                throw AdapterError.unavailable("canonical Host did not return an attachment path")
            }
            committed.append(path)
        }
        return committed
    }

    private func discardStagedAttachment(_ params: [String: Any]) {
        guard let raw = params["path"] as? String else { return }
        let staged = URL(fileURLWithPath: raw).standardizedFileURL
        guard isWithinStagingRoot(staged) else { return }
        try? fileManager.removeItem(at: staged)
        try? fileManager.removeItem(at: metadataURL(for: staged))
    }

    private func readAttachmentText(_ params: [String: Any]) async throws -> String? {
        let result = try await hostRequest(
            "feature.attachment.readTextDirect",
            [
                "agentId": try requiredString(params["agentId"], "agentId"),
                "path": try requiredString(params["path"], "path"),
            ]
        )
        guard let row = result as? [String: Any],
              row["kind"] as? String == "text"
        else { return nil }
        return row["text"] as? String
    }

    private func readAttachmentBytes(_ params: [String: Any]) async throws -> Any {
        let agentID = try requiredString(params["agentId"], "agentId")
        let path = try requiredString(params["path"], "path")
        let requested = integer(params["maxBytes"]) ?? Self.previewByteCap
        let cap = max(1, min(requested, Self.previewByteCap))
        let probe = try await readChunk(agentID: agentID, path: path, offset: 0, length: 1)
        let totalSize = integer(probe["totalSize"]) ?? 0
        if totalSize > cap {
            return ["kind": "too-large", "size": totalSize]
        }
        var buffer = Data()
        buffer.reserveCapacity(totalSize)
        var offset = 0
        while offset < totalSize {
            try Task.checkCancellation()
            let row = try await readChunk(
                agentID: agentID,
                path: path,
                offset: offset,
                length: min(Self.gatewayReadChunkBytes, totalSize - offset)
            )
            guard let encoded = row["bytesBase64"] as? String,
                  let bytes = Data(base64Encoded: encoded),
                  !bytes.isEmpty
            else { throw AdapterError.unavailable("canonical Host returned an incomplete attachment chunk") }
            buffer.append(bytes)
            offset += bytes.count
        }
        guard buffer.count == totalSize else {
            throw AdapterError.unavailable("canonical Host attachment transfer was incomplete")
        }
        return ["kind": "bytes", "bytesBase64": buffer.base64EncodedString()]
    }

    private func resolveAttachmentMedia(_ params: [String: Any]) async throws -> Any {
        let agentID = try requiredString(params["agentId"], "agentId")
        let source = try requiredString(params["source"], "source")
        let ext = URL(fileURLWithPath: source).pathExtension.lowercased()
        if ["mp4", "mov", "m4v", "webm"].contains(ext) {
            return ["kind": "video", "src": source, "width": NSNull(), "height": NSNull()]
        }
        if ["mp3", "m4a", "aac", "wav", "ogg", "flac"].contains(ext) {
            return ["kind": "audio", "src": source]
        }
        let result = try await hostRequest(
            "feature.attachment.readImageDirect",
            ["agentId": agentID, "path": source]
        )
        guard let row = result as? [String: Any],
              let dataURL = row["dataUrl"] as? String
        else { return NSNull() }
        return [
            "kind": "image",
            "src": dataURL,
            "width": row["width"] ?? NSNull(),
            "height": row["height"] ?? NSNull(),
        ]
    }

    private func downloadAttachment(_ params: [String: Any]) async throws -> Bool {
        let agentID = try requiredString(params["agentId"], "agentId")
        let source = try requiredString(params["path"], "path")
        let suggested = try safeFilename(
            params["suggestedName"] ?? URL(fileURLWithPath: source).lastPathComponent,
            field: "suggestedName"
        )
        let probe = try await readChunk(agentID: agentID, path: source, offset: 0, length: 1)
        let totalSize = integer(probe["totalSize"]) ?? 0
        guard totalSize > 0 else { return false }

        let final = uniqueDownloadURL(suggested)
        let partial = final.appendingPathExtension("partial")
        fileManager.createFile(atPath: partial.path, contents: nil)
        var completed = false
        defer {
            if !completed {
                try? fileManager.removeItem(at: partial)
            }
        }

        let handle = try FileHandle(forWritingTo: partial)
        defer { try? handle.close() }
        var offset = 0
        while offset < totalSize {
            try Task.checkCancellation()
            let row = try await readChunk(
                agentID: agentID,
                path: source,
                offset: offset,
                length: min(Self.gatewayReadChunkBytes, totalSize - offset)
            )
            guard let encoded = row["bytesBase64"] as? String,
                  let bytes = Data(base64Encoded: encoded),
                  !bytes.isEmpty
            else { return false }
            try handle.seek(toOffset: UInt64(offset))
            try handle.write(contentsOf: bytes)
            offset += bytes.count
        }
        guard offset == totalSize else { return false }
        try handle.synchronize()
        try handle.close()
        try? fileManager.removeItem(at: final)
        try fileManager.moveItem(at: partial, to: final)
        completed = true
        return true
    }

    private func readChunk(
        agentID: String,
        path: String,
        offset: Int,
        length: Int
    ) async throws -> [String: Any] {
        let result = try await hostRequest(
            "feature.attachment.readChunkDirect",
            [
                "agentId": agentID,
                "path": path,
                "offset": offset,
                "length": max(1, min(length, 8 * 1024 * 1024)),
            ]
        )
        guard let row = result as? [String: Any] else {
            throw AdapterError.unavailable("canonical Host returned a malformed attachment chunk")
        }
        return row
    }

    private func requiredAccountScope() throws -> String {
        guard let scope = currentAccountScope()?.trimmingCharacters(in: .whitespacesAndNewlines),
              !scope.isEmpty
        else { throw AdapterError.unavailable("attachment staging requires an authenticated account scope") }
        return scope
    }

    private func loadMetadata(for staged: URL) throws -> StagedMetadata {
        guard isWithinStagingRoot(staged) else {
            throw AdapterError.invalid("staged attachment escaped native staging root")
        }
        return try JSONDecoder().decode(
            StagedMetadata.self,
            from: Data(contentsOf: metadataURL(for: staged))
        )
    }

    private func metadataURL(for staged: URL) -> URL {
        staged.appendingPathExtension("json")
    }

    private func isWithinStagingRoot(_ url: URL) -> Bool {
        let root = stagingRoot.standardizedFileURL.path
        let candidate = url.standardizedFileURL.path
        return candidate == root || candidate.hasPrefix(root + "/")
    }

    private func attachmentBytes(_ params: [String: Any]) -> Data? {
        if let data = params["bytes"] as? Data { return data }
        if let encoded = params["bytesBase64"] as? String { return Data(base64Encoded: encoded) }
        if let values = params["bytes"] as? [NSNumber] {
            return Data(values.map { UInt8(truncating: $0) })
        }
        if let values = params["bytes"] as? [Int],
           values.allSatisfy({ (0...255).contains($0) }) {
            return Data(values.map(UInt8.init))
        }
        return nil
    }

    private func requiredString(_ raw: Any?, _ field: String) throws -> String {
        guard let value = raw as? String else {
            throw AdapterError.invalid("\(field) is required")
        }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw AdapterError.invalid("\(field) must not be empty")
        }
        return trimmed
    }

    private func safeFilename(_ raw: Any?, field: String) throws -> String {
        let value = try requiredString(raw, field)
        guard value != ".", value != "..",
              !value.contains("/"), !value.contains("\\"),
              value.unicodeScalars.allSatisfy({ !$0.properties.isControl })
        else { throw AdapterError.invalid("\(field) must be a safe file name") }
        return String(value.prefix(240))
    }

    private func stringArray(_ raw: Any?) -> [String] {
        (raw as? [Any])?.compactMap { $0 as? String } ?? []
    }

    private func integer(_ raw: Any?) -> Int? {
        if let value = raw as? Int { return value }
        if let value = raw as? NSNumber { return value.intValue }
        return nil
    }

    private func uniqueDownloadURL(_ filename: String) -> URL {
        let stem = URL(fileURLWithPath: filename).deletingPathExtension().lastPathComponent
        let ext = URL(fileURLWithPath: filename).pathExtension
        for index in 0..<10_000 {
            let suffix = index == 0 ? "" : "-\(index)"
            let name = ext.isEmpty ? "\(stem)\(suffix)" : "\(stem)\(suffix).\(ext)"
            let candidate = downloadRoot.appendingPathComponent(name, isDirectory: false)
            if !fileManager.fileExists(atPath: candidate.path),
               !fileManager.fileExists(atPath: candidate.appendingPathExtension("partial").path) {
                return candidate
            }
        }
        return downloadRoot.appendingPathComponent(UUID().uuidString.lowercased())
    }

    private func success(_ value: Any) throws -> CoordinatorReplyOutcome {
        .ok(try CoordinatorPayload.fromFoundation(value))
    }

    private static func byteLimit(for filename: String) -> Int {
        let ext = URL(fileURLWithPath: filename).pathExtension.lowercased()
        return ["mp4", "mov", "m4v", "webm"].contains(ext) ? videoByteCap : previewByteCap
    }
}

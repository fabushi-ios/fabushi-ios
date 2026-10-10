import Foundation

let ALLOWED_CLIENT_PERSISTENCE_KEY_PREFIX = "sand."
let CLIENT_PERSISTENCE_MAX_VALUE_BYTES = 8 * 1024 * 1024
let CLIENT_PERSISTENCE_MAX_TOTAL_BYTES = 256 * 1024 * 1024
let CLIENT_PERSISTENCE_MIGRATION_MARKER_FILENAME = ".migrated-from-local-storage"
private let CLIENT_PERSISTENCE_FILE_NAME_ALPHABET = Array("abcdefghijklmnopqrstuvwxyz234567")
private let CLIENT_PERSISTENCE_FILE_NAME_SUFFIX = ".blob"
private let CLIENT_PERSISTENCE_MAX_FILE_NAME_LENGTH = 240
private let CLIENT_PERSISTENCE_TEMP_FILE_SUFFIX = ".tmp"

func encodeClientPersistenceFileName(_ key: String) -> String {
    var name = ""
    var buffer = 0
    var bits = 0
    for byte in key.utf8 {
        buffer = (buffer << 8) | Int(byte)
        bits += 8
        while bits >= 5 {
            let index = (buffer >> (bits - 5)) & 31
            name.append(CLIENT_PERSISTENCE_FILE_NAME_ALPHABET[index])
            bits -= 5
        }
    }
    if bits > 0 {
        let index = (buffer << (5 - bits)) & 31
        name.append(CLIENT_PERSISTENCE_FILE_NAME_ALPHABET[index])
    }
    return name + CLIENT_PERSISTENCE_FILE_NAME_SUFFIX
}

func decodeClientPersistenceFileName(_ name: String) -> String? {
    guard name.hasSuffix(CLIENT_PERSISTENCE_FILE_NAME_SUFFIX) else { return nil }
    let body = String(name.dropLast(CLIENT_PERSISTENCE_FILE_NAME_SUFFIX.count))
    guard !body.isEmpty else { return nil }
    var bytes: [UInt8] = []
    var buffer = 0
    var bits = 0
    for character in body {
        guard let index = CLIENT_PERSISTENCE_FILE_NAME_ALPHABET.firstIndex(of: character) else { return nil }
        buffer = (buffer << 5) | index
        bits += 5
        if bits >= 8 {
            bytes.append(UInt8((buffer >> (bits - 8)) & 255))
            bits -= 8
        }
    }
    if bits > 0 && (buffer & ((1 << bits) - 1)) != 0 { return nil }
    guard let key = String(bytes: bytes, encoding: .utf8),
          key.hasPrefix(ALLOWED_CLIENT_PERSISTENCE_KEY_PREFIX) else { return nil }
    return key
}

func clientPersistenceFileNameFor(_ key: String) -> String? {
    guard key.hasPrefix(ALLOWED_CLIENT_PERSISTENCE_KEY_PREFIX) else { return nil }
    let name = encodeClientPersistenceFileName(key)
    return name.count <= CLIENT_PERSISTENCE_MAX_FILE_NAME_LENGTH ? name : nil
}

struct ClientPersistenceCapError: Error, Equatable, Sendable {
    let message: String
}

protocol ClientPersistenceFiles: Sendable {
    func joinPath(_ dir: String, _ name: String) -> String
    func ensureDir(_ dir: String) async throws
    func listFiles(_ dir: String) async throws -> [String]
    func readTextFile(_ path: String) async throws -> String
    func writeTextFile(_ path: String, data: String) async throws
    func rename(_ from: String, to: String) async throws
    func removeFile(_ path: String) async throws
    func fileSize(_ path: String) async throws -> Int?
}

private struct ClientPersistenceBlobFact: Sendable {
    let key: String
    let size: Int
}

actor SandClientPersistenceStore {
    private let dir: String
    private let files: any ClientPersistenceFiles
    private let maxValueBytes: Int
    private let maxTotalBytes: Int
    private var blobs: [String: ClientPersistenceBlobFact]?

    init(
        dir: String,
        files: any ClientPersistenceFiles,
        maxValueBytes: Int = CLIENT_PERSISTENCE_MAX_VALUE_BYTES,
        maxTotalBytes: Int = CLIENT_PERSISTENCE_MAX_TOTAL_BYTES
    ) {
        self.dir = dir
        self.files = files
        self.maxValueBytes = maxValueBytes
        self.maxTotalBytes = maxTotalBytes
    }

    func read(_ key: String) async throws -> String? {
        let name = try encodedFileName(key)
        let ledger = await loadLedger()
        guard ledger[name] != nil else { return nil }
        return try? await files.readTextFile(files.joinPath(dir, name))
    }

    func write(_ key: String, value: String) async throws {
        let name = try encodedFileName(key)
        var ledger = await loadLedger()
        try await writeBlob(blobs: &ledger, name: name, key: key, value: value)
        blobs = ledger
    }

    func remove(_ key: String) async throws {
        let name = try encodedFileName(key)
        var ledger = await loadLedger()
        guard ledger.removeValue(forKey: name) != nil else { return }
        try await files.removeFile(files.joinPath(dir, name))
        blobs = ledger
    }

    func listKeys(prefix: String) async -> [String] {
        let ledger = await loadLedger()
        return ledger.values.map(\.key).filter { $0.hasPrefix(prefix) }.sorted()
    }

    func hasCompletedOneShotMigration() async -> Bool {
        (try? await files.fileSize(files.joinPath(dir, CLIENT_PERSISTENCE_MIGRATION_MARKER_FILENAME))) != nil
    }

    func migrateFromLocalStorage(_ entries: [(key: String, value: String)]) async -> Bool {
        if await hasCompletedOneShotMigration() { return true }
        var ledger = await loadLedger()
        for entry in entries {
            guard let name = clientPersistenceFileNameFor(entry.key), ledger[name] == nil else { continue }
            do {
                try await writeBlob(blobs: &ledger, name: name, key: entry.key, value: entry.value)
            } catch is ClientPersistenceCapError {
                continue
            } catch {
                return false
            }
        }
        do {
            try await files.ensureDir(dir)
            try await files.writeTextFile(
                files.joinPath(dir, CLIENT_PERSISTENCE_MIGRATION_MARKER_FILENAME),
                data: ISO8601DateFormatter().string(from: Date())
            )
            blobs = ledger
            return true
        } catch {
            return false
        }
    }

    private func encodedFileName(_ key: String) throws -> String {
        guard let name = clientPersistenceFileNameFor(key) else {
            throw ClientPersistenceCapError(message: "client persistence key is outside the allowed namespace or too long")
        }
        return name
    }

    private func loadLedger() async -> [String: ClientPersistenceBlobFact] {
        if let blobs { return blobs }
        var loaded: [String: ClientPersistenceBlobFact] = [:]
        let names = (try? await files.listFiles(dir)) ?? []
        for name in names {
            if name.hasSuffix(CLIENT_PERSISTENCE_TEMP_FILE_SUFFIX) {
                try? await files.removeFile(files.joinPath(dir, name))
                continue
            }
            guard let key = decodeClientPersistenceFileName(name),
                  let size = try? await files.fileSize(files.joinPath(dir, name)) else { continue }
            loaded[name] = .init(key: key, size: size)
        }
        blobs = loaded
        return loaded
    }

    private func writeBlob(
        blobs ledger: inout [String: ClientPersistenceBlobFact],
        name: String,
        key: String,
        value: String
    ) async throws {
        let size = value.lengthOfBytes(using: .utf8)
        guard size <= maxValueBytes else {
            throw ClientPersistenceCapError(message: "client persistence value exceeds the per-key size cap")
        }
        let total = size + ledger.filter { $0.key != name }.reduce(0) { $0 + $1.value.size }
        guard total <= maxTotalBytes else {
            throw ClientPersistenceCapError(message: "client persistence store exceeds its total size cap")
        }
        try await files.ensureDir(dir)
        let temporary = files.joinPath(dir, name + CLIENT_PERSISTENCE_TEMP_FILE_SUFFIX)
        try await files.writeTextFile(temporary, data: value)
        try await files.rename(temporary, to: files.joinPath(dir, name))
        ledger[name] = .init(key: key, size: size)
    }
}

import CryptoKit
import Foundation

enum FabAppPackageClass: String, Equatable, Sendable {
    case catalog
    case localGenerated = "local-generated"
}

struct FabAppCatalogTrust: Sendable {
    let appId: String
    let publisherId: String
    let keyId: String
    let publicKey: Data
    let revoked: Bool
}

struct VerifiedFabAppFile: Equatable, Sendable {
    let path: String
    let sha256: String
    let bytes: Int
    let mime: String
}

struct VerifiedFabAppPackage: Equatable, Sendable {
    let appId: String
    let publisherId: String
    let version: String
    let entrypoint: String
    let minHostApi: Int
    let offlineCore: Bool
    let packageClass: FabAppPackageClass
    let requestedCapabilities: [String]
    let files: [String: VerifiedFabAppFile]
}

enum FabAppVerificationError: LocalizedError, Equatable {
    case invalidJSON
    case duplicateJSONKey(String)
    case unknownField(String)
    case unsupportedManifestVersion
    case unsupportedPackageClass
    case invalidIdentifier(String)
    case invalidVersion
    case invalidEntrypoint
    case invalidRuntime
    case incompatibleHostAPI
    case invalidNetworkPolicy
    case invalidCapability
    case invalidFileTable
    case invalidPath(String)
    case pathCollision(String)
    case missingFile(String)
    case unexpectedFile(String)
    case fileSizeMismatch(String)
    case fileDigestMismatch(String)
    case invalidMime(String)
    case packageTooLarge
    case tooManyFiles
    case trustMismatch
    case revokedTrust
    case unsupportedSignature
    case invalidSignature

    var errorDescription: String? {
        switch self {
        case .invalidJSON: "Mini App manifest is not strict JSON."
        case .duplicateJSONKey(let key): "Mini App manifest contains duplicate JSON key: \(key)"
        case .unknownField(let field): "Mini App manifest contains unsupported field: \(field)"
        case .unsupportedManifestVersion: "Unsupported .fabapp manifest version."
        case .unsupportedPackageClass: "Unsupported .fabapp package trust class."
        case .invalidIdentifier(let field): "Invalid .fabapp identifier: \(field)"
        case .invalidVersion: "Invalid .fabapp version."
        case .invalidEntrypoint: "Invalid .fabapp entrypoint."
        case .invalidRuntime: "Unsupported .fabapp runtime."
        case .incompatibleHostAPI: ".fabapp requires a newer host API."
        case .invalidNetworkPolicy: "Invalid .fabapp network policy."
        case .invalidCapability: "Invalid .fabapp capability declaration."
        case .invalidFileTable: "Invalid .fabapp file table."
        case .invalidPath(let path): "Unsafe .fabapp path: \(path)"
        case .pathCollision(let path): "Colliding .fabapp path: \(path)"
        case .missingFile(let path): "Missing .fabapp file: \(path)"
        case .unexpectedFile(let path): "Unexpected .fabapp file: \(path)"
        case .fileSizeMismatch(let path): "Size mismatch for .fabapp file: \(path)"
        case .fileDigestMismatch(let path): "Digest mismatch for .fabapp file: \(path)"
        case .invalidMime(let path): "Invalid MIME declaration for .fabapp file: \(path)"
        case .packageTooLarge: ".fabapp expanded content exceeds the iOS verification limit."
        case .tooManyFiles: ".fabapp contains too many files."
        case .trustMismatch: ".fabapp publisher/app signing authority mismatch."
        case .revokedTrust: ".fabapp publisher trust is revoked."
        case .unsupportedSignature: "Unsupported .fabapp signature."
        case .invalidSignature: ".fabapp signature verification failed."
        }
    }
}

enum FabAppPackageVerifier {
    static let maximumExpandedBytes = 250 * 1024 * 1024
    static let maximumFileCount = 10_000
    static let domainSeparator = Data("fabushi.fabapp.manifest.v1\u{0}".utf8)

    private static let manifestFields: Set<String> = [
        "manifestVersion", "packageClass", "appId", "publisherId", "version",
        "displayName", "entrypoint", "runtime", "minHostApi", "offlineCore",
        "requestedCapabilities", "networkPolicy", "files", "signature",
    ]
    private static let fileFields: Set<String> = ["sha256", "bytes", "mime"]
    private static let signatureFields: Set<String> = ["algorithm", "keyId", "value"]
    private static let networkFields: Set<String> = ["mode", "allowedOrigins"]

    static func verifyCatalogPackage(
        manifestData: Data,
        resources: [String: Data],
        trust: FabAppCatalogTrust,
        hostApi: Int
    ) throws -> VerifiedFabAppPackage {
        var parser = StrictJSONParser(data: manifestData)
        let json = try parser.parse()
        guard case .object(let manifest) = json else {
            throw FabAppVerificationError.invalidJSON
        }
        try rejectUnknown(manifest.keys, allowed: manifestFields)

        guard manifest["manifestVersion"]?.intValue == 1 else {
            throw FabAppVerificationError.unsupportedManifestVersion
        }
        let packageClassRaw = manifest["packageClass"]?.stringValue ?? "catalog"
        guard let packageClass = FabAppPackageClass(rawValue: packageClassRaw),
              packageClass == .catalog
        else {
            throw FabAppVerificationError.unsupportedPackageClass
        }

        let appId = try requiredIdentifier(manifest["appId"], field: "appId")
        let publisherId = try requiredIdentifier(manifest["publisherId"], field: "publisherId")
        guard appId == trust.appId, publisherId == trust.publisherId else {
            throw FabAppVerificationError.trustMismatch
        }
        guard !trust.revoked else { throw FabAppVerificationError.revokedTrust }

        guard let version = manifest["version"]?.stringValue,
              version.range(of: #"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$"#, options: .regularExpression) != nil
        else { throw FabAppVerificationError.invalidVersion }

        guard manifest["runtime"]?.stringValue == "web" else {
            throw FabAppVerificationError.invalidRuntime
        }
        guard let minHostApi = manifest["minHostApi"]?.intValue,
              minHostApi >= 1,
              minHostApi <= hostApi
        else { throw FabAppVerificationError.incompatibleHostAPI }
        guard let offlineCore = manifest["offlineCore"]?.boolValue else {
            throw FabAppVerificationError.invalidJSON
        }

        let requestedCapabilities = try parseCapabilities(manifest["requestedCapabilities"])
        try validateNetworkPolicy(manifest["networkPolicy"], offlineCore: offlineCore)

        let files = try parseFiles(manifest["files"])
        guard files.count <= maximumFileCount else {
            throw FabAppVerificationError.tooManyFiles
        }
        try validatePathSet(Array(files.keys))

        guard let entrypoint = manifest["entrypoint"]?.stringValue,
              isSafePath(entrypoint),
              let entry = files[entrypoint],
              entry.mime == "text/html"
        else { throw FabAppVerificationError.invalidEntrypoint }

        let expectedPaths = Set(files.keys)
        let actualPaths = Set(resources.keys)
        if let missing = expectedPaths.subtracting(actualPaths).sorted().first {
            throw FabAppVerificationError.missingFile(missing)
        }
        if let unexpected = actualPaths.subtracting(expectedPaths).sorted().first {
            throw FabAppVerificationError.unexpectedFile(unexpected)
        }

        var expandedBytes = 0
        for path in expectedPaths.sorted() {
            guard let declaration = files[path], let data = resources[path] else {
                throw FabAppVerificationError.missingFile(path)
            }
            expandedBytes = try checkedAdd(expandedBytes, data.count)
            guard expandedBytes <= maximumExpandedBytes else {
                throw FabAppVerificationError.packageTooLarge
            }
            guard declaration.bytes == data.count else {
                throw FabAppVerificationError.fileSizeMismatch(path)
            }
            let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
            guard digest == declaration.sha256 else {
                throw FabAppVerificationError.fileDigestMismatch(path)
            }
        }

        guard case .object(let signature)? = manifest["signature"] else {
            throw FabAppVerificationError.unsupportedSignature
        }
        try rejectUnknown(signature.keys, allowed: signatureFields)
        guard signature["algorithm"]?.stringValue == "Ed25519",
              signature["keyId"]?.stringValue == trust.keyId,
              let encodedSignature = signature["value"]?.stringValue,
              let signatureData = decodeBase64URL(encodedSignature)
        else { throw FabAppVerificationError.unsupportedSignature }

        var unsigned = manifest
        unsigned.removeValue(forKey: "signature")
        let canonical = try StrictJSON.object(unsigned).canonicalData()
        let signedPayload = domainSeparator + canonical
        do {
            let key = try Curve25519.Signing.PublicKey(rawRepresentation: trust.publicKey)
            guard key.isValidSignature(signatureData, for: signedPayload) else {
                throw FabAppVerificationError.invalidSignature
            }
        } catch let error as FabAppVerificationError {
            throw error
        } catch {
            throw FabAppVerificationError.invalidSignature
        }

        return VerifiedFabAppPackage(
            appId: appId,
            publisherId: publisherId,
            version: version,
            entrypoint: entrypoint,
            minHostApi: minHostApi,
            offlineCore: offlineCore,
            packageClass: packageClass,
            requestedCapabilities: requestedCapabilities,
            files: files
        )
    }

    static func canonicalSigningPayloadForTests(manifestData: Data) throws -> Data {
        var parser = StrictJSONParser(data: manifestData)
        let parsed = try parser.parse()
        guard case .object(var object) = parsed else { throw FabAppVerificationError.invalidJSON }
        object.removeValue(forKey: "signature")
        return domainSeparator + (try StrictJSON.object(object).canonicalData())
    }

    private static func parseCapabilities(_ value: StrictJSON?) throws -> [String] {
        guard case .array(let values)? = value else {
            throw FabAppVerificationError.invalidCapability
        }
        var seen = Set<String>()
        var result: [String] = []
        for item in values {
            guard let capability = item.stringValue,
                  capability.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil,
                  seen.insert(capability).inserted
            else { throw FabAppVerificationError.invalidCapability }
            result.append(capability)
        }
        return result
    }

    private static func validateNetworkPolicy(_ value: StrictJSON?, offlineCore: Bool) throws {
        guard case .object(let object)? = value else {
            throw FabAppVerificationError.invalidNetworkPolicy
        }
        try rejectUnknown(object.keys, allowed: networkFields)
        guard let mode = object["mode"]?.stringValue,
              ["deny", "allowlist"].contains(mode),
              case .array(let origins)? = object["allowedOrigins"],
              origins.allSatisfy({ $0.stringValue != nil })
        else { throw FabAppVerificationError.invalidNetworkPolicy }
        if mode == "deny" && !origins.isEmpty {
            throw FabAppVerificationError.invalidNetworkPolicy
        }
        if offlineCore && mode != "deny" {
            throw FabAppVerificationError.invalidNetworkPolicy
        }
    }

    private static func parseFiles(_ value: StrictJSON?) throws -> [String: VerifiedFabAppFile] {
        guard case .object(let object)? = value, !object.isEmpty else {
            throw FabAppVerificationError.invalidFileTable
        }
        var result: [String: VerifiedFabAppFile] = [:]
        for (path, raw) in object {
            guard case .object(let fields) = raw else {
                throw FabAppVerificationError.invalidFileTable
            }
            try rejectUnknown(fields.keys, allowed: fileFields)
            guard let digest = fields["sha256"]?.stringValue,
                  digest.range(of: #"^[0-9a-f]{64}$"#, options: .regularExpression) != nil,
                  let bytes = fields["bytes"]?.intValue, bytes >= 0,
                  let mime = fields["mime"]?.stringValue,
                  isAllowedMime(mime, path: path)
            else { throw FabAppVerificationError.invalidFileTable }
            result[path] = VerifiedFabAppFile(path: path, sha256: digest, bytes: bytes, mime: mime)
        }
        return result
    }

    private static func isAllowedMime(_ mime: String, path: String) -> Bool {
        let lower = path.lowercased()
        let expected: Set<String>
        if lower.hasSuffix(".html") || lower.hasSuffix(".htm") {
            expected = ["text/html"]
        } else if lower.hasSuffix(".js") || lower.hasSuffix(".mjs") {
            expected = ["text/javascript", "application/javascript"]
        } else if lower.hasSuffix(".css") {
            expected = ["text/css"]
        } else if lower.hasSuffix(".json") {
            expected = ["application/json"]
        } else if lower.hasSuffix(".wasm") {
            expected = ["application/wasm"]
        } else if lower.hasSuffix(".png") {
            expected = ["image/png"]
        } else if lower.hasSuffix(".jpg") || lower.hasSuffix(".jpeg") {
            expected = ["image/jpeg"]
        } else if lower.hasSuffix(".svg") {
            expected = ["image/svg+xml"]
        } else {
            return mime.range(of: #"^[a-z0-9.+-]+/[a-z0-9.+-]+$"#, options: .regularExpression) != nil
        }
        return expected.contains(mime.lowercased())
    }

    private static func validatePathSet(_ paths: [String]) throws {
        var normalized = Set<String>()
        for path in paths {
            guard isSafePath(path) else { throw FabAppVerificationError.invalidPath(path) }
            let key = path.precomposedStringWithCanonicalMapping.lowercased()
            guard normalized.insert(key).inserted else {
                throw FabAppVerificationError.pathCollision(path)
            }
        }
    }

    private static func isSafePath(_ path: String) -> Bool {
        guard !path.isEmpty,
              !path.hasPrefix("/"),
              !path.contains("\\"),
              !path.contains("\u{0}"),
              path == path.precomposedStringWithCanonicalMapping,
              path.range(of: #"^[A-Za-z]:/"#, options: .regularExpression) == nil
        else { return false }
        let components = path.split(separator: "/", omittingEmptySubsequences: false)
        return components.allSatisfy { !$0.isEmpty && $0 != "." && $0 != ".." }
    }

    private static func requiredIdentifier(_ value: StrictJSON?, field: String) throws -> String {
        guard let text = value?.stringValue,
              text.range(of: #"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$"#, options: .regularExpression) != nil
        else { throw FabAppVerificationError.invalidIdentifier(field) }
        return text
    }

    private static func rejectUnknown(_ keys: Dictionary<String, StrictJSON>.Keys, allowed: Set<String>) throws {
        if let unknown = keys.first(where: { !allowed.contains($0) }) {
            throw FabAppVerificationError.unknownField(unknown)
        }
    }

    private static func checkedAdd(_ left: Int, _ right: Int) throws -> Int {
        let (value, overflow) = left.addingReportingOverflow(right)
        if overflow { throw FabAppVerificationError.packageTooLarge }
        return value
    }

    private static func decodeBase64URL(_ value: String) -> Data? {
        var normalized = value.replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        let remainder = normalized.count % 4
        if remainder != 0 { normalized += String(repeating: "=", count: 4 - remainder) }
        return Data(base64Encoded: normalized)
    }
}

private indirect enum StrictJSON: Equatable {
    case object([String: StrictJSON])
    case array([StrictJSON])
    case string(String)
    case integer(Int)
    case bool(Bool)
    case null

    var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }
    var intValue: Int? {
        if case .integer(let value) = self { return value }
        return nil
    }
    var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }

    func canonicalData() throws -> Data {
        Data(try canonicalString().utf8)
    }

    private func canonicalString() throws -> String {
        switch self {
        case .null: return "null"
        case .bool(let value): return value ? "true" : "false"
        case .integer(let value): return String(value)
        case .string(let value): return Self.quote(value)
        case .array(let values):
            return "[" + (try values.map { try $0.canonicalString() }.joined(separator: ",")) + "]"
        case .object(let object):
            let keys = object.keys.sorted { left, right in
                Array(left.utf16).lexicographicallyPrecedes(Array(right.utf16))
            }
            return "{" + (try keys.map { key in
                Self.quote(key) + ":" + (try object[key]!.canonicalString())
            }.joined(separator: ",")) + "}"
        }
    }

    private static func quote(_ value: String) -> String {
        var out = "\""
        for scalar in value.unicodeScalars {
            switch scalar.value {
            case 0x22: out += "\\\""
            case 0x5c: out += "\\\\"
            case 0x08: out += "\\b"
            case 0x0c: out += "\\f"
            case 0x0a: out += "\\n"
            case 0x0d: out += "\\r"
            case 0x09: out += "\\t"
            case 0x00...0x1f: out += String(format: "\\u%04x", scalar.value)
            default: out.unicodeScalars.append(scalar)
            }
        }
        out += "\""
        return out
    }
}

private struct StrictJSONParser {
    private let bytes: [UInt8]
    private var index = 0

    init(data: Data) {
        bytes = Array(data)
    }

    mutating func parse() throws -> StrictJSON {
        skipWhitespace()
        let value = try parseValue()
        skipWhitespace()
        guard index == bytes.count else { throw FabAppVerificationError.invalidJSON }
        return value
    }

    private mutating func parseValue() throws -> StrictJSON {
        guard index < bytes.count else { throw FabAppVerificationError.invalidJSON }
        switch bytes[index] {
        case 0x7b: return try parseObject()
        case 0x5b: return try parseArray()
        case 0x22: return .string(try parseString())
        case 0x74: try consume("true"); return .bool(true)
        case 0x66: try consume("false"); return .bool(false)
        case 0x6e: try consume("null"); return .null
        case 0x2d, 0x30...0x39: return .integer(try parseInteger())
        default: throw FabAppVerificationError.invalidJSON
        }
    }

    private mutating func parseObject() throws -> StrictJSON {
        index += 1
        skipWhitespace()
        var result: [String: StrictJSON] = [:]
        if take(0x7d) { return .object(result) }
        while true {
            guard index < bytes.count, bytes[index] == 0x22 else {
                throw FabAppVerificationError.invalidJSON
            }
            let key = try parseString()
            guard result[key] == nil else {
                throw FabAppVerificationError.duplicateJSONKey(key)
            }
            skipWhitespace()
            guard take(0x3a) else { throw FabAppVerificationError.invalidJSON }
            skipWhitespace()
            result[key] = try parseValue()
            skipWhitespace()
            if take(0x7d) { return .object(result) }
            guard take(0x2c) else { throw FabAppVerificationError.invalidJSON }
            skipWhitespace()
        }
    }

    private mutating func parseArray() throws -> StrictJSON {
        index += 1
        skipWhitespace()
        var result: [StrictJSON] = []
        if take(0x5d) { return .array(result) }
        while true {
            result.append(try parseValue())
            skipWhitespace()
            if take(0x5d) { return .array(result) }
            guard take(0x2c) else { throw FabAppVerificationError.invalidJSON }
            skipWhitespace()
        }
    }

    private mutating func parseString() throws -> String {
        guard take(0x22) else { throw FabAppVerificationError.invalidJSON }
        var raw: [UInt8] = []
        var result = ""
        func flushRaw() throws {
            guard !raw.isEmpty else { return }
            guard let decoded = String(data: Data(raw), encoding: .utf8) else {
                throw FabAppVerificationError.invalidJSON
            }
            result += decoded
            raw.removeAll(keepingCapacity: true)
        }
        while index < bytes.count {
            let byte = bytes[index]
            index += 1
            if byte == 0x22 {
                try flushRaw()
                return result
            }
            if byte == 0x5c {
                try flushRaw()
                guard index < bytes.count else { throw FabAppVerificationError.invalidJSON }
                let escape = bytes[index]
                index += 1
                switch escape {
                case 0x22: result.append("\"")
                case 0x5c: result.append("\\")
                case 0x2f: result.append("/")
                case 0x62: result.append("\u{8}")
                case 0x66: result.append("\u{c}")
                case 0x6e: result.append("\n")
                case 0x72: result.append("\r")
                case 0x74: result.append("\t")
                case 0x75:
                    let first = try parseHexValue()
                    if (0xd800...0xdbff).contains(first) {
                        guard take(0x5c), take(0x75) else { throw FabAppVerificationError.invalidJSON }
                        let second = try parseHexValue()
                        guard (0xdc00...0xdfff).contains(second) else {
                            throw FabAppVerificationError.invalidJSON
                        }
                        let scalarValue = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                        guard let scalar = UnicodeScalar(scalarValue) else { throw FabAppVerificationError.invalidJSON }
                        result.unicodeScalars.append(scalar)
                    } else if (0xdc00...0xdfff).contains(first) {
                        throw FabAppVerificationError.invalidJSON
                    } else {
                        guard let scalar = UnicodeScalar(first) else { throw FabAppVerificationError.invalidJSON }
                        result.unicodeScalars.append(scalar)
                    }
                default: throw FabAppVerificationError.invalidJSON
                }
            } else {
                guard byte >= 0x20 else { throw FabAppVerificationError.invalidJSON }
                raw.append(byte)
            }
        }
        throw FabAppVerificationError.invalidJSON
    }

    private mutating func parseHexValue() throws -> UInt32 {
        guard index + 4 <= bytes.count else { throw FabAppVerificationError.invalidJSON }
        var value: UInt32 = 0
        for _ in 0..<4 {
            let byte = bytes[index]
            index += 1
            value <<= 4
            switch byte {
            case 0x30...0x39: value += UInt32(byte - 0x30)
            case 0x41...0x46: value += UInt32(byte - 0x41 + 10)
            case 0x61...0x66: value += UInt32(byte - 0x61 + 10)
            default: throw FabAppVerificationError.invalidJSON
            }
        }
        return value
    }

    private mutating func parseInteger() throws -> Int {
        let start = index
        _ = take(0x2d)
        guard index < bytes.count else { throw FabAppVerificationError.invalidJSON }
        if take(0x30) {
            if index < bytes.count, (0x30...0x39).contains(bytes[index]) {
                throw FabAppVerificationError.invalidJSON
            }
        } else {
            guard index < bytes.count, (0x31...0x39).contains(bytes[index]) else {
                throw FabAppVerificationError.invalidJSON
            }
            while index < bytes.count, (0x30...0x39).contains(bytes[index]) { index += 1 }
        }
        guard index == bytes.count || ![0x2e, 0x45, 0x65].contains(bytes[index]),
              let text = String(bytes: bytes[start..<index], encoding: .utf8),
              let value = Int(text)
        else { throw FabAppVerificationError.invalidJSON }
        return value
    }

    private mutating func consume(_ literal: StaticString) throws {
        let text = String(describing: literal)
        guard index + text.utf8.count <= bytes.count else {
            throw FabAppVerificationError.invalidJSON
        }
        for byte in text.utf8 {
            guard bytes[index] == byte else { throw FabAppVerificationError.invalidJSON }
            index += 1
        }
    }

    private mutating func take(_ byte: UInt8) -> Bool {
        guard index < bytes.count, bytes[index] == byte else { return false }
        index += 1
        return true
    }

    private mutating func skipWhitespace() {
        while index < bytes.count, [0x20, 0x09, 0x0a, 0x0d].contains(bytes[index]) {
            index += 1
        }
    }
}

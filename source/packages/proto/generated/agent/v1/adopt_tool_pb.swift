import Foundation

enum AdoptToolProtoError: Error, Equatable, Sendable {
    case invalidFieldNumber
    case invalidVarint
    case lengthOverflow
    case truncated
    case invalidUTF8
    case unsupportedWireType(Int)
    case wrongWireType(field: Int, expected: Int, actual: Int)
}

struct AdoptOutcome: RawRepresentable, Equatable, Hashable, Sendable {
    let rawValue: Int32

    init(rawValue: Int32) {
        self.rawValue = rawValue
    }

    static let unspecified = AdoptOutcome(rawValue: 0)
    static let alreadyParented = AdoptOutcome(rawValue: 1)
    static let edgeOnly = AdoptOutcome(rawValue: 2)
    static let storeImportCompleted = AdoptOutcome(rawValue: 3)
}

enum AdoptResultValue: Equatable, Sendable {
    case success(AdoptOutcome)
    case error(String)
}

struct AdoptArgs: Equatable, Sendable {
    static let protobufTypeName = "agent.v1.AdoptArgs"

    var sourceAgentId: String

    init(sourceAgentId: String = "") {
        self.sourceAgentId = sourceAgentId
    }

    init(serializedData: Data) throws {
        var reader = AdoptProtoReader(serializedData)
        var sourceAgentId = ""

        while let field = try reader.nextField() {
            switch field.number {
            case 1:
                try reader.requireWireType(2, for: field)
                sourceAgentId = try reader.readString()
            default:
                try reader.skip(wireType: field.wireType)
            }
        }

        self.sourceAgentId = sourceAgentId
    }

    func serializedData() -> Data {
        var data = Data()
        if !sourceAgentId.isEmpty {
            AdoptProtoWriter.string(field: 1, sourceAgentId, into: &data)
        }
        return data
    }
}

struct AdoptResult: Equatable, Sendable {
    static let protobufTypeName = "agent.v1.AdoptResult"

    var sourceAgentId: String
    var targetAgentId: String
    var projectRootId: String
    var result: AdoptResultValue?

    init(
        sourceAgentId: String = "",
        targetAgentId: String = "",
        projectRootId: String = "",
        result: AdoptResultValue? = nil
    ) {
        self.sourceAgentId = sourceAgentId
        self.targetAgentId = targetAgentId
        self.projectRootId = projectRootId
        self.result = result
    }

    init(serializedData: Data) throws {
        var reader = AdoptProtoReader(serializedData)
        var sourceAgentId = ""
        var targetAgentId = ""
        var projectRootId = ""
        var result: AdoptResultValue?

        while let field = try reader.nextField() {
            switch field.number {
            case 1:
                try reader.requireWireType(2, for: field)
                sourceAgentId = try reader.readString()
            case 2:
                try reader.requireWireType(2, for: field)
                targetAgentId = try reader.readString()
            case 3:
                try reader.requireWireType(2, for: field)
                projectRootId = try reader.readString()
            case 4:
                try reader.requireWireType(0, for: field)
                result = .success(AdoptOutcome(rawValue: Int32(truncatingIfNeeded: try reader.readVarint())))
            case 5:
                try reader.requireWireType(2, for: field)
                result = .error(try reader.readString())
            default:
                try reader.skip(wireType: field.wireType)
            }
        }

        self.sourceAgentId = sourceAgentId
        self.targetAgentId = targetAgentId
        self.projectRootId = projectRootId
        self.result = result
    }

    func serializedData() -> Data {
        var data = Data()
        if !sourceAgentId.isEmpty {
            AdoptProtoWriter.string(field: 1, sourceAgentId, into: &data)
        }
        if !targetAgentId.isEmpty {
            AdoptProtoWriter.string(field: 2, targetAgentId, into: &data)
        }
        if !projectRootId.isEmpty {
            AdoptProtoWriter.string(field: 3, projectRootId, into: &data)
        }
        switch result {
        case let .success(outcome):
            AdoptProtoWriter.varint(
                field: 4,
                UInt64(bitPattern: Int64(outcome.rawValue)),
                into: &data
            )
        case let .error(message):
            AdoptProtoWriter.string(field: 5, message, into: &data, preservePresence: true)
        case nil:
            break
        }
        return data
    }
}

struct AdoptToolCall: Equatable, Sendable {
    static let protobufTypeName = "agent.v1.AdoptToolCall"

    var args: AdoptArgs?
    var result: AdoptResult?

    init(args: AdoptArgs? = nil, result: AdoptResult? = nil) {
        self.args = args
        self.result = result
    }

    init(serializedData: Data) throws {
        var reader = AdoptProtoReader(serializedData)
        var args: AdoptArgs?
        var result: AdoptResult?

        while let field = try reader.nextField() {
            switch field.number {
            case 1:
                try reader.requireWireType(2, for: field)
                args = try AdoptArgs(serializedData: reader.readLengthDelimited())
            case 2:
                try reader.requireWireType(2, for: field)
                result = try AdoptResult(serializedData: reader.readLengthDelimited())
            default:
                try reader.skip(wireType: field.wireType)
            }
        }

        self.args = args
        self.result = result
    }

    func serializedData() -> Data {
        var data = Data()
        if let args {
            AdoptProtoWriter.bytes(field: 1, args.serializedData(), into: &data)
        }
        if let result {
            AdoptProtoWriter.bytes(field: 2, result.serializedData(), into: &data)
        }
        return data
    }
}

private struct AdoptProtoField {
    let number: Int
    let wireType: Int
}

private struct AdoptProtoReader {
    private let bytes: [UInt8]
    private var index: Int = 0

    init(_ data: Data) {
        bytes = Array(data)
    }

    mutating func nextField() throws -> AdoptProtoField? {
        guard index < bytes.count else {
            return nil
        }

        let key = try readVarint()
        let number = Int(key >> 3)
        guard number > 0, number <= 536_870_911 else {
            throw AdoptToolProtoError.invalidFieldNumber
        }
        return AdoptProtoField(number: number, wireType: Int(key & 0x07))
    }

    mutating func requireWireType(_ expected: Int, for field: AdoptProtoField) throws {
        guard field.wireType == expected else {
            throw AdoptToolProtoError.wrongWireType(
                field: field.number,
                expected: expected,
                actual: field.wireType
            )
        }
    }

    mutating func readVarint() throws -> UInt64 {
        var value: UInt64 = 0

        // A UInt64 protobuf varint is at most ten bytes. The tenth byte may
        // carry only bit 63 and must terminate the varint; accepting a larger
        // payload would silently truncate overflow through UInt64 shifts.
        for byteIndex in 0..<10 {
            guard index < bytes.count else {
                throw AdoptToolProtoError.invalidVarint
            }
            let byte = bytes[index]
            index += 1
            if byteIndex == 9, byte > 0x01 {
                throw AdoptToolProtoError.invalidVarint
            }
            value |= UInt64(byte & 0x7f) << (byteIndex * 7)
            if byte & 0x80 == 0 {
                return value
            }
        }

        throw AdoptToolProtoError.invalidVarint
    }

    mutating func readLengthDelimited() throws -> Data {
        let rawLength = try readVarint()
        guard rawLength <= UInt64(Int.max) else {
            throw AdoptToolProtoError.lengthOverflow
        }
        let length = Int(rawLength)
        guard index <= bytes.count, length <= bytes.count - index else {
            throw AdoptToolProtoError.truncated
        }
        let end = index + length
        let data = Data(bytes[index..<end])
        index = end
        return data
    }

    mutating func readString() throws -> String {
        let data = try readLengthDelimited()
        guard let value = String(data: data, encoding: .utf8) else {
            throw AdoptToolProtoError.invalidUTF8
        }
        return value
    }

    mutating func skip(wireType: Int) throws {
        switch wireType {
        case 0:
            _ = try readVarint()
        case 1:
            try advance(8)
        case 2:
            _ = try readLengthDelimited()
        case 5:
            try advance(4)
        default:
            throw AdoptToolProtoError.unsupportedWireType(wireType)
        }
    }

    private mutating func advance(_ count: Int) throws {
        guard index <= bytes.count, count <= bytes.count - index else {
            throw AdoptToolProtoError.truncated
        }
        index += count
    }
}

private enum AdoptProtoWriter {
    static func varint(field: Int, _ value: UInt64, into data: inout Data) {
        key(field: field, wireType: 0, into: &data)
        rawVarint(value, into: &data)
    }

    static func string(
        field: Int,
        _ value: String,
        into data: inout Data,
        preservePresence: Bool = false
    ) {
        if value.isEmpty && !preservePresence {
            return
        }
        bytes(field: field, Data(value.utf8), into: &data)
    }

    static func bytes(field: Int, _ value: Data, into data: inout Data) {
        key(field: field, wireType: 2, into: &data)
        rawVarint(UInt64(value.count), into: &data)
        data.append(value)
    }

    private static func key(field: Int, wireType: Int, into data: inout Data) {
        rawVarint(UInt64((field << 3) | wireType), into: &data)
    }

    private static func rawVarint(_ value: UInt64, into data: inout Data) {
        var remaining = value
        while remaining >= 0x80 {
            data.append(UInt8(remaining & 0x7f) | 0x80)
            remaining >>= 7
        }
        data.append(UInt8(remaining))
    }
}

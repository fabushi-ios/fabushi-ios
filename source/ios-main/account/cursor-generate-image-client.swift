import Foundation

struct IOSCursorGenerateImageError: Error, LocalizedError, Equatable, Sendable {
    let message: String
    var errorDescription: String? { message }
}

enum IOSCursorGenerateImageProto {
    private enum FieldValue { case varint(UInt64); case bytes(Data) }

    private struct Reader {
        let bytes: [UInt8]
        var offset = 0

        mutating func varint() throws -> UInt64 {
            var result: UInt64 = 0
            var shift: UInt64 = 0
            while offset < bytes.count, shift < 64 {
                let byte = bytes[offset]
                offset += 1
                result |= UInt64(byte & 0x7f) << shift
                if byte & 0x80 == 0 { return result }
                shift += 7
            }
            throw IOSCursorGenerateImageError(message: "Generate-image protobuf contained an invalid varint.")
        }

        mutating func next() throws -> (number: Int, value: FieldValue)? {
            guard offset < bytes.count else { return nil }
            let key = try varint()
            let number = Int(key >> 3)
            guard number > 0 else {
                throw IOSCursorGenerateImageError(message: "Generate-image protobuf contained field zero.")
            }
            switch Int(key & 0x7) {
            case 0:
                return (number, .varint(try varint()))
            case 2:
                let length = try varint()
                guard length <= UInt64(Int.max) else {
                    throw IOSCursorGenerateImageError(message: "Generate-image protobuf length overflowed this platform.")
                }
                let end = offset + Int(length)
                guard end <= bytes.count else {
                    throw IOSCursorGenerateImageError(message: "Generate-image protobuf ended unexpectedly.")
                }
                let data = Data(bytes[offset..<end])
                offset = end
                return (number, .bytes(data))
            default:
                throw IOSCursorGenerateImageError(message: "Generate-image protobuf used an unsupported wire type.")
            }
        }
    }

    private static func appendVarint(_ value: UInt64, to data: inout Data) {
        var value = value
        while value >= 0x80 {
            data.append(UInt8(value & 0x7f) | 0x80)
            value >>= 7
        }
        data.append(UInt8(value))
    }

    private static func appendBytes(_ number: Int, _ bytes: Data, to data: inout Data) {
        appendVarint(UInt64(number << 3 | 2), to: &data)
        appendVarint(UInt64(bytes.count), to: &data)
        data.append(bytes)
    }

    private static func appendString(_ number: Int, _ value: String, to data: inout Data) {
        guard !value.isEmpty else { return }
        appendBytes(number, Data(value.utf8), to: &data)
    }

    static func encodeRequest(_ request: RunGenerateImageRequest) -> Data {
        var data = Data()
        appendString(1, request.description, to: &data)
        for reference in request.referenceImages {
            var nested = Data()
            appendString(1, reference.data, to: &nested)
            appendString(2, reference.mimeType, to: &nested)
            appendBytes(2, nested, to: &data)
        }
        appendString(3, request.modelId, to: &data)
        if request.maxMode {
            appendVarint(UInt64(4 << 3), to: &data)
            appendVarint(1, to: &data)
        }
        return data
    }

    private static func string(_ data: Data) throws -> String {
        guard let value = String(data: data, encoding: .utf8) else {
            throw IOSCursorGenerateImageError(message: "Generate-image protobuf contained invalid UTF-8.")
        }
        return value
    }

    private static func success(_ data: Data) throws -> GeneratedImagePayload {
        var reader = Reader(bytes: Array(data))
        var imageData = "", mimeType = ""
        while let field = try reader.next() {
            switch (field.number, field.value) {
            case (1, .bytes(let bytes)): imageData = try string(bytes)
            case (2, .bytes(let bytes)): mimeType = try string(bytes)
            default: continue
            }
        }
        guard !imageData.isEmpty, mimeType.hasPrefix("image/") else {
            throw IOSCursorGenerateImageError(message: "Generate-image success response was incomplete.")
        }
        return .init(imageData: imageData, mimeType: mimeType)
    }

    private static func failure(_ data: Data) throws -> GenerateImageWireResult {
        var reader = Reader(bytes: Array(data))
        var message = ""
        var modelRestricted = false
        while let field = try reader.next() {
            switch (field.number, field.value) {
            case (1, .bytes(let bytes)): message = try string(bytes)
            case (2, .varint(let value)): modelRestricted = value != 0
            default: continue
            }
        }
        return .error(
            message: message.isEmpty ? "Image generation failed." : message,
            modelRestricted: modelRestricted
        )
    }

    static func decodeResponse(_ data: Data) throws -> GenerateImageWireResult {
        var reader = Reader(bytes: Array(data))
        while let field = try reader.next() {
            switch (field.number, field.value) {
            case (1, .bytes(let bytes)): return .success(try success(bytes))
            case (2, .bytes(let bytes)): return try failure(bytes)
            default: continue
            }
        }
        return .none
    }
}

final class IOSCursorGenerateImageClient: @unchecked Sendable, CursorGenerateImageClient {
    typealias RequestExecutor = @Sendable (URLRequest) async throws -> (Data, URLResponse)
    private let credentials: AccountMcpCredentials
    private let backendURL: URL
    private let session: URLSession
    private let requestExecutor: RequestExecutor?

    init(
        credentials: AccountMcpCredentials,
        backendURL: URL = URL(string: getConfiguredBackendUrl())!,
        session: URLSession = .shared,
        requestExecutor: RequestExecutor? = nil
    ) {
        self.credentials = credentials
        self.backendURL = backendURL
        self.session = session
        self.requestExecutor = requestExecutor
    }

    private func perform(_ request: URLRequest) async throws -> (Data, URLResponse) {
        if let requestExecutor { return try await requestExecutor(request) }
        return try await session.data(for: request)
    }

    func runGenerateImage(_ requestValue: RunGenerateImageRequest) async throws -> GenerateImageWireResult {
        let description = requestValue.description.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !description.isEmpty else {
            throw IOSCursorGenerateImageError(message: "Describe the avatar to generate first.")
        }
        let headers = try await createSandInferenceHeaders(
            backendUrl: backendURL.absoluteString,
            getAccessToken: { value in try await self.credentials.getAccessToken(value) },
            getMachineId: credentials.getMachineId,
            resolveGhostMode: { _ in "true" }
        )
        let url = backendURL
            .appendingPathComponent("aiserver.v1.AiService")
            .appendingPathComponent("RunGenerateImage")
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = 60
        request.setValue("application/proto", forHTTPHeaderField: "Content-Type")
        request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
        for (name, value) in headers.headers { request.setValue(value, forHTTPHeaderField: name) }
        request.httpBody = IOSCursorGenerateImageProto.encodeRequest(.init(
            description: description,
            referenceImages: requestValue.referenceImages,
            modelId: requestValue.modelId,
            maxMode: requestValue.maxMode
        ))
        let (data, response) = try await perform(request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorGenerateImageError(message: "Generate-image RPC returned no HTTP response.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let detail = String(data: data, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines).prefix(512)
            let suffix = detail.map { $0.isEmpty ? "" : ": \($0)" } ?? ""
            throw IOSCursorGenerateImageError(
                message: "Generate-image RPC failed with HTTP \(http.statusCode)\(suffix)"
            )
        }
        return try IOSCursorGenerateImageProto.decodeResponse(data)
    }
}

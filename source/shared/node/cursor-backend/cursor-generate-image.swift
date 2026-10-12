import Foundation

struct GenerateImageReferenceImage: Equatable, Sendable {
    let data: String
    let mimeType: String
}

struct RunGenerateImageRequest: Equatable, Sendable {
    let description: String
    let referenceImages: [GenerateImageReferenceImage]
    let modelId: String
    let maxMode: Bool
}

struct GeneratedImagePayload: Equatable, Sendable {
    let imageData: String
    let mimeType: String
}

enum GenerateImageWireResult: Equatable, Sendable {
    case success(GeneratedImagePayload)
    case error(message: String, modelRestricted: Bool)
    case none
}

protocol CursorGenerateImageClient: Sendable {
    func runGenerateImage(_ request: RunGenerateImageRequest) async throws -> GenerateImageWireResult
}

struct SandGenerateImageError: Error, LocalizedError, Equatable, Sendable {
    let message: String
    var errorDescription: String? { message }
}

struct SandGenerateImageModelRestrictedError: Error, LocalizedError, Equatable, Sendable {
    let message: String
    var errorDescription: String? { message }
}

struct CursorGenerateImageService: Sendable {
    let client: any CursorGenerateImageClient
    let modelId: String
    let maxMode: Bool

    func generate(
        description: String,
        referenceImages: [GenerateImageReferenceImage] = []
    ) async throws -> GeneratedImagePayload {
        let result = try await client.runGenerateImage(.init(
            description: description,
            referenceImages: referenceImages,
            modelId: modelId,
            maxMode: maxMode
        ))
        switch result {
        case .success(let payload):
            return payload
        case .error(let message, let modelRestricted):
            if modelRestricted {
                throw SandGenerateImageModelRestrictedError(message: message)
            }
            throw SandGenerateImageError(message: message)
        case .none:
            throw SandGenerateImageError(message: "Image generation returned no result.")
        }
    }
}

func createCursorGenerateImageService(
    client: any CursorGenerateImageClient,
    modelId: String,
    maxMode: Bool = false
) -> CursorGenerateImageService {
    .init(client: client, modelId: modelId, maxMode: maxMode)
}

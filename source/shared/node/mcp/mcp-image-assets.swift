import Foundation

struct SavedMcpImage: Equatable, Sendable {
    let fileUrl: String
    let width: Int?
    let height: Int?
}

func describeSavedImage(_ image: SavedMcpImage) -> String {
    let dimensions: String
    if let width = image.width, let height = image.height {
        dimensions = " (\(width)x\(height))"
    } else {
        dimensions = ""
    }
    return "Saved this image to disk at \(image.fileUrl)\(dimensions). To show it to the user, pass that path to SendMessage as {\"type\":\"attachment\",\"url\":\"\(image.fileUrl)\"}."
}

func augmentMcpResultWithSavedImages(
    _ result: SandMcpResult,
    persistImage: @Sendable (String, String) async throws -> SavedMcpImage?,
    factory: any McpResultFactory = generatedMcpResultFactory
) async -> SandMcpResult {
    guard case .success(let success) = result.result else { return result }
    guard success.content.contains(where: {
        if case .image = $0.content { return true }
        return false
    }) else {
        return result
    }

    var augmented: [SandMcpContentItem] = []
    for item in success.content {
        augmented.append(item)
        guard case .image(let image) = item.content, !image.data.isEmpty else { continue }
        let saved: SavedMcpImage?
        do {
            saved = try await persistImage(image.data, image.mimeType)
        } catch {
            saved = nil
        }
        if let saved {
            augmented.append(factory.textItem(describeSavedImage(saved)))
        }
    }
    return factory.success(result, content: augmented)
}

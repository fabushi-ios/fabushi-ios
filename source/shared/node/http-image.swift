import Foundation

func imageContentType(_ response: HTTPURLResponse) -> String? {
    guard let raw = response.value(forHTTPHeaderField: "Content-Type") else { return nil }
    let value = raw.split(separator: ";", maxSplits: 1).first?
        .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return value.lowercased().hasPrefix("image/") ? value : nil
}

func readCappedImageBytes(
    _ response: HTTPURLResponse,
    chunks: [Data],
    maxBytes: Int
) -> Data? {
    if let raw = response.value(forHTTPHeaderField: "Content-Length"),
       let declared = Int(raw),
       declared > maxBytes { return nil }
    var output = Data()
    for chunk in chunks {
        guard output.count + chunk.count <= maxBytes else { return nil }
        output.append(chunk)
    }
    return output.isEmpty ? nil : output
}

func responseToImageDataUrl(
    _ response: HTTPURLResponse,
    body: Data,
    maxBytes: Int
) -> String? {
    guard (200..<300).contains(response.statusCode),
          let contentType = imageContentType(response),
          let bytes = readCappedImageBytes(response, chunks: [body], maxBytes: maxBytes) else { return nil }
    return "data:\(contentType);base64,\(bytes.base64EncodedString())"
}

import Foundation

final class URLSessionEgressWebSocket: @unchecked Sendable, ExitWebSocket {
    private let task: URLSessionWebSocketTask

    init(url: URL, options: EgressWebSocketOptions, session: URLSession = .shared) {
        var request = URLRequest(url: url)
        for (name, value) in options.headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        task = session.webSocketTask(with: request)
        task.resume()
    }

    func send(_ data: Data) async throws {
        try await task.send(.data(data))
    }

    func receive() async throws -> Data {
        switch try await task.receive() {
        case .data(let data):
            return data
        case .string(let text):
            return Data(text.utf8)
        @unknown default:
            throw URLError(.cannotDecodeContentData)
        }
    }

    func close() {
        task.cancel(with: .normalClosure, reason: nil)
    }
}

func createWebSocket(
    _ url: String,
    options: EgressWebSocketOptions,
    session: URLSession = .shared
) -> (any ExitWebSocket)? {
    guard let parsed = URL(string: url),
          parsed.scheme == "ws" || parsed.scheme == "wss" else {
        return nil
    }
    return URLSessionEgressWebSocket(url: parsed, options: options, session: session)
}

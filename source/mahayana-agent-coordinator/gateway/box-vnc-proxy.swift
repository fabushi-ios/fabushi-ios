import Foundation

@MainActor
final class BoxVNCProxy {
    typealias Dispatch = @MainActor (_ method: RemoteComputerMethod, _ payload: CoordinatorPayload) async throws -> CoordinatorPayload

    private let dispatch: Dispatch

    init(dispatch: @escaping Dispatch) {
        self.dispatch = dispatch
    }

    func readClipboard() async throws -> String {
        let value = try await dispatch(.readClipboard, .object([:]))
        guard case .string(let text) = value else { return "" }
        return text
    }

    func writeClipboard(_ text: String) async throws {
        _ = try await dispatch(.writeClipboard, .object(["text": .string(text)]))
    }

    func reportUserPresence(active: Bool, timestampMilliseconds: Int64) async throws {
        _ = try await dispatch(.reportUserPresence, .object([
            "active": .bool(active),
            "timestampMilliseconds": .number(Double(timestampMilliseconds)),
        ]))
    }
}

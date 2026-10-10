import Foundation

@MainActor
final class PreloadRemoteComputerAPI {
    private let primary: PreloadPrimaryAPI

    init(primary: PreloadPrimaryAPI) {
        self.primary = primary
    }

    func readClipboard() async throws -> String {
        let value = try await primary.call(method: "remote-computer.readClipboard")
        guard case .string(let text) = value else { return "" }
        return text
    }

    func writeClipboard(_ text: String) async throws {
        _ = try await primary.call(
            method: "remote-computer.writeClipboard",
            payload: .object(["text": .string(text)])
        )
    }

    func reportUserPresence(active: Bool, timestampMilliseconds: Int64) async throws {
        _ = try await primary.call(
            method: "remote-computer.reportUserPresence",
            payload: .object([
                "active": .bool(active),
                "timestampMilliseconds": .number(Double(timestampMilliseconds)),
            ])
        )
    }
}

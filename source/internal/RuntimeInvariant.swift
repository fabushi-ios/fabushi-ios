import Foundation

enum RuntimeInvariant {
    static func requireNonEmpty(_ value: String, _ name: StaticString) throws -> String {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw NSError(
                domain: "com.ombhrum.fabushi.runtime-invariant",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "\(name) must not be empty"]
            )
        }
        return trimmed
    }
}

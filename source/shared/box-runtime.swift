import Foundation

enum SandBoxRuntime: String, Codable, CaseIterable, Equatable, Sendable {
    case remote
    case localDocker = "local-docker"
}

let DEFAULT_SAND_BOX_RUNTIME: SandBoxRuntime = .remote

func isSandBoxRuntime(_ value: Any) -> Bool {
    guard let value = value as? String else { return false }
    return SandBoxRuntime(rawValue: value) != nil
}

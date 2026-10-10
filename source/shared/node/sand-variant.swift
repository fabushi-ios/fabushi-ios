import Foundation

enum SandVariant: String, Equatable, Sendable {
    case dev = "sand-dev"
    case lab = "sand-lab"
    case prod = "sand"
}

func isSandPackaged(_ env: [String: String] = ProcessInfo.processInfo.environment) -> Bool {
    env["SAND_PACKAGED"] == "1"
}

func isSandLabBuild(_ env: [String: String] = ProcessInfo.processInfo.environment) -> Bool {
    env["SAND_LAB"] == "1"
}

func getSandVariant(_ env: [String: String] = ProcessInfo.processInfo.environment) -> SandVariant {
    if !isSandPackaged(env) { return .dev }
    return isSandLabBuild(env) ? .lab : .prod
}

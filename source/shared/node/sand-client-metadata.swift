import Foundation

let SAND_CLIENT_TYPE = "sand"
let SAND_CLIENT_FALLBACK_BASE_VERSION = "0.1.0"
let SAND_CLIENT_VERSION_DEV_SUFFIX = "-dev"
let SAND_CLIENT_VERSION_LAB_SUFFIX = "-lab"
let SAND_BOX_NAMESPACE_HEADER = "x-sand-box-namespace"

enum SandBoxNamespace: String, Equatable, Sendable {
    case dev, lab, prod
}

func getSandClientBaseVersion(_ env: [String: String] = ProcessInfo.processInfo.environment) -> String {
    let stamped = env["SAND_CLIENT_APP_VERSION"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    if let regex = try? NSRegularExpression(pattern: #"^(\d+\.\d+\.\d+)(?:-.+)?$"#),
       let match = regex.firstMatch(in: stamped, range: NSRange(stamped.startIndex..<stamped.endIndex, in: stamped)),
       let range = Range(match.range(at: 1), in: stamped) {
        return String(stamped[range])
    }
    return SAND_CLIENT_FALLBACK_BASE_VERSION
}

func getSandBoxNamespace(_ env: [String: String] = ProcessInfo.processInfo.environment) -> SandBoxNamespace {
    let owner = env["SAND_BOX_OWNER_NAMESPACE"]?.trimmingCharacters(in: .whitespacesAndNewlines)
    if owner == "dev" { return .dev }
    if owner == "lab" { return .lab }
    switch getSandVariant(env) {
    case .dev: return .dev
    case .lab: return .lab
    case .prod: return .prod
    }
}

func getSandClientVersion(_ env: [String: String] = ProcessInfo.processInfo.environment) -> String {
    let base = getSandClientBaseVersion(env)
    switch getSandBoxNamespace(env) {
    case .dev: return base + SAND_CLIENT_VERSION_DEV_SUFFIX
    case .lab: return base + SAND_CLIENT_VERSION_LAB_SUFFIX
    case .prod: return base
    }
}

func getSandBackendClientHeaders(_ env: [String: String] = ProcessInfo.processInfo.environment) -> [String: String] {
    [
        "x-cursor-client-type": SAND_CLIENT_TYPE,
        "x-cursor-client-version": getSandClientVersion(env),
        SAND_BOX_NAMESPACE_HEADER: getSandBoxNamespace(env).rawValue,
    ]
}

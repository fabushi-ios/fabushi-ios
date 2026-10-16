import Foundation

let BOX_SECRET_REDACTION_NAMES_ENV_VAR = "CLOUD_AGENT_INJECTED_SECRET_NAMES"
let RESERVED_EXACT_BOX_SECRET_NAMES: Set<String> = [
    "PATH", "HOME", "USER", "SHELL", "TERM", "PWD", "DISPLAY",
    BOX_SECRET_REDACTION_NAMES_ENV_VAR,
]
let RESERVED_BOX_SECRET_PREFIXES = ["SAND_", "__CURSOR", "LD_"]
let MAX_BOX_SECRET_COUNT = 100
let MAX_BOX_SECRET_VALUE_LENGTH = 32 * 1024
let MAX_BOX_SECRETS_TOTAL_LENGTH = 96 * 1024

func validateBoxSecretKey(_ key: String) -> String? {
    guard isValidEnvironmentName(key) else {
        return "\"\(key)\" is not a valid environment variable name"
    }
    if RESERVED_EXACT_BOX_SECRET_NAMES.contains(key) {
        return "\(key) is reserved by the box runtime"
    }
    for prefix in RESERVED_BOX_SECRET_PREFIXES where key.hasPrefix(prefix) {
        return "Names starting with \(prefix) are reserved by the box runtime"
    }
    if key.range(of: "CURSOR_SANDBOX", options: [.regularExpression, .caseInsensitive]) != nil {
        return "\(key) is reserved by the box runtime"
    }
    return nil
}

func validateBoxSecrets(_ secrets: [String: String]) -> String? {
    let keys = Array(secrets.keys)
    guard keys.count <= MAX_BOX_SECRET_COUNT else {
        return "Too many secrets (max \(MAX_BOX_SECRET_COUNT))"
    }
    var total = 0
    for key in keys {
        if let error = validateBoxSecretKey(key) { return error }
        let value = secrets[key] ?? ""
        guard value.count <= MAX_BOX_SECRET_VALUE_LENGTH else {
            return "The value of \(key) is too large (max \(MAX_BOX_SECRET_VALUE_LENGTH.formatted()) characters)"
        }
        total += key.count + value.count
    }
    return total > MAX_BOX_SECRETS_TOTAL_LENGTH
        ? "The combined size of all secrets is too large"
        : nil
}

func buildBoxSecretsEnv(_ secrets: [String: String]) -> [String: String] {
    let names = secrets.keys.sorted()
    guard !names.isEmpty else { return [:] }
    var env: [String: String] = [:]
    for name in names {
        env[name] = secrets[name] ?? ""
    }
    env[BOX_SECRET_REDACTION_NAMES_ENV_VAR] = names.joined(separator: ",")
    return env
}

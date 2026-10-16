import Foundation

enum KnownShellExecutor: String, Sendable, CaseIterable {
    case zsh = "zsh"
    case zshLight = "zsh-light"
    case bash = "bash"
    case powerShell = "powershell"
    case naive = "naive"
}

let SHELL_ENV_OVERRIDES: [String: String] = [
    "TERM": "dumb",
    "NO_COLOR": "1",
    "FORCE_COLOR": "0",
    "_ZO_DOCTOR": "0",
]

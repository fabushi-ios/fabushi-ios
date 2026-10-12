import Foundation

struct ShellExecNetworkPolicy: Equatable, Sendable {
    enum DefaultAction: String, Equatable, Sendable {
        case allow
        case deny
    }

    let version: Int
    let defaultAction: DefaultAction
    let allow: [String]?
    let deny: [String]?

    init(
        version: Int = 1,
        defaultAction: DefaultAction,
        allow: [String]? = nil,
        deny: [String]? = nil
    ) {
        self.version = version
        self.defaultAction = defaultAction
        self.allow = allow
        self.deny = deny
    }
}

enum ShellExecNetworkPolicyUtils {
    static func effective(_ policy: ShellExecNetworkPolicy?) -> ShellExecNetworkPolicy {
        policy ?? networkDisabledPolicy()
    }

    static func isNetworkEnabled(_ policy: ShellExecNetworkPolicy?) -> Bool {
        guard let policy else { return false }
        return policy.defaultAction == .allow || !(policy.allow?.isEmpty ?? true)
    }

    static func networkDisabledPolicy() -> ShellExecNetworkPolicy {
        ShellExecNetworkPolicy(defaultAction: .deny)
    }

    static func networkAllowAllPolicy() -> ShellExecNetworkPolicy {
        ShellExecNetworkPolicy(defaultAction: .allow)
    }
}

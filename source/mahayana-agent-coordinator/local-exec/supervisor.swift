import Foundation

actor IOSLocalExecSupervisor {
    enum Route: Equatable, Sendable {
        case local(LocalCapabilityRunner.Capability)
        case remote
        case unavailable(String)
    }

    private let localRunner: LocalCapabilityRunner

    init(localRunner: LocalCapabilityRunner = LocalCapabilityRunner()) {
        self.localRunner = localRunner
    }

    func route(capabilityName: String) async -> Route {
        if let capability = LocalCapabilityRunner.Capability(rawValue: capabilityName),
           await localRunner.supports(capability) {
            return .local(capability)
        }
        if capabilityName.hasPrefix("shell.") || capabilityName.hasPrefix("process.") || capabilityName.hasPrefix("box.") {
            return .remote
        }
        return .unavailable(capabilityName)
    }
}

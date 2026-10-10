import Foundation

struct IOSUpdateTelemetryProjection: Equatable, Sendable {
    let level: String
    let metadata: [String: String]
}

enum IOSUpdateTelemetry {
    static func storeManaged(action: String, accepted: Bool) -> IOSUpdateTelemetryProjection {
        .init(
            level: "info",
            metadata: [
                "action": action,
                "accepted": accepted ? "true" : "false",
                "mechanism": "app-store-connect",
                "self_update_supported": "false",
            ]
        )
    }

    static func direction(targetVersion: String?, currentVersion: String) -> String {
        guard let targetVersion,
              let order = IOSUpdateVersion.compare(targetVersion, currentVersion)
        else { return "unknown" }
        if order > 0 { return "upgrade" }
        if order < 0 { return "downgrade" }
        return "same"
    }
}

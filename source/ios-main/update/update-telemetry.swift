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
    static func check(
        trigger: String,
        result: String,
        currentVersion: String,
        targetVersion: String?,
        errorClass: String? = nil
    ) -> IOSUpdateTelemetryProjection {
        var metadata: [String: String] = [
            "action": "check",
            "trigger": trigger,
            "track": "stable",
            "result": result,
            "mechanism": "app-store-connect",
            "self_update_supported": "false",
            "direction": direction(
                targetVersion: targetVersion,
                currentVersion: currentVersion
            ),
        ]
        if let targetVersion, !targetVersion.isEmpty {
            metadata["target_version"] = targetVersion
        }
        if let errorClass, !errorClass.isEmpty {
            metadata["error_class"] = String(errorClass.prefix(128))
        }
        return .init(
            level: result == "error" ? "warn" : "info",
            metadata: metadata
        )
    }

}

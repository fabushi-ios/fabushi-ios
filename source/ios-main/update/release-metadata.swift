import Foundation

struct IOSReleaseMetadata: Equatable, Sendable {
    let version: String
    let buildNumber: String
    let bundleIdentifier: String
    let updateMechanism: String
}

enum IOSReleaseMetadataReader {
    static func read(infoDictionary: [String: Any]) -> IOSReleaseMetadata? {
        guard let version = infoDictionary["CFBundleShortVersionString"] as? String,
              !version.isEmpty,
              let buildNumber = infoDictionary["CFBundleVersion"] as? String,
              !buildNumber.isEmpty,
              let bundleIdentifier = infoDictionary["CFBundleIdentifier"] as? String,
              !bundleIdentifier.isEmpty
        else { return nil }

        return .init(
            version: version,
            buildNumber: buildNumber,
            bundleIdentifier: bundleIdentifier,
            updateMechanism: "app-store-connect"
        )
    }

    static func read(bundle: Bundle = .main) -> IOSReleaseMetadata? {
        read(infoDictionary: bundle.infoDictionary ?? [:])
    }
}

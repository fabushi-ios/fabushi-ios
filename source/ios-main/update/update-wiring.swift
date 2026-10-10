import Foundation

@MainActor
final class IOSUpdateServiceWiring {
    private let service: IOSAppStoreUpdateService

    init(service: IOSAppStoreUpdateService = .init()) {
        self.service = service
    }

    func route(
        method: String,
        args: CoordinatorPayload
    ) -> CoordinatorReplyOutcome? {
        do {
            switch method {
            case "getUpdateStatus", "checkForUpdates":
                return .ok(try service.statusPayload())

            case "setUpdateTrack":
                return .ok(service.unsupportedActionPayload("setUpdateTrack"))

            case "quitAndInstallUpdate":
                return .ok(service.unsupportedActionPayload("quitAndInstallUpdate"))

            case "setAutoUpdateWhenIdleOptIn":
                return .ok(service.unsupportedActionPayload("setAutoUpdateWhenIdleOptIn"))

            default:
                return nil
            }
        } catch {
            return .failed(.init(
                code: "ios-update-metadata-unavailable",
                message: error.localizedDescription
            ))
        }
    }
}

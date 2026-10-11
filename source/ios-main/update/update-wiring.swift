import Foundation

@MainActor
final class IOSUpdateServiceWiring {
    private let service: IOSAppStoreUpdateService

    init(service: IOSAppStoreUpdateService = .init()) {
        self.service = service
    }

    var policyLoadState: IOSAppVersionPolicyLoadState {
        service.policyLoadState
    }

    func refreshPolicy() async -> IOSAppVersionPolicyLoadState {
        do {
            _ = try await service.checkForUpdates()
            return service.policyLoadState
        } catch is CancellationError {
            return service.policyLoadState
        } catch {
            return .failed(
                message: error.localizedDescription,
                retained: service.policyLoadState.policy
            )
        }
    }

    func route(
        method: String,
        args: CoordinatorPayload
    ) async -> CoordinatorReplyOutcome? {
        do {
            switch method {
            case "getUpdateStatus":
                return .ok(try service.statusPayload())

            case "checkForUpdates":
                return .ok(try await service.checkForUpdates())

            case "setUpdateTrack":
                guard let object = args.foundationValue as? [String: Any],
                      let track = object["track"] as? String
                else {
                    return .failed(.init(
                        code: "invalid-params",
                        message: "setUpdateTrack requires a track."
                    ))
                }
                guard track == "stable" else {
                    return .failed(.init(
                        code: "ios-update-track-managed-by-app-store",
                        message: "App Store/TestFlight controls the iOS release track."
                    ))
                }
                return .ok(try service.statusPayload())

            case "quitAndInstallUpdate":
                return .ok(service.unsupportedActionPayload("quitAndInstallUpdate"))

            case "setAutoUpdateWhenIdleOptIn":
                guard let object = args.foundationValue as? [String: Any],
                      let enabled = object["enabled"] as? Bool
                else {
                    return .failed(.init(
                        code: "invalid-params",
                        message: "setAutoUpdateWhenIdleOptIn requires enabled."
                    ))
                }
                guard !enabled else {
                    return .failed(.init(
                        code: "ios-update-idle-apply-managed-by-app-store",
                        message: "iOS updates are installed by App Store/TestFlight, not by an in-app idle relaunch."
                    ))
                }
                return .ok(try service.statusPayload())

            default:
                return nil
            }
        } catch is CancellationError {
            return .failed(.init(
                code: CoordinatorProtocol.cancelled,
                message: "Update request was cancelled."
            ))
        } catch {
            return .failed(.init(
                code: "ios-update-metadata-unavailable",
                message: error.localizedDescription
            ))
        }
    }
}

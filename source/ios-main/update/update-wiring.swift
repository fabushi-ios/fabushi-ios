import Foundation

@MainActor
final class IOSUpdateServiceWiring {
    typealias TelemetrySink = @MainActor (IOSUpdateTelemetryProjection) -> Void

    private let service: IOSAppStoreUpdateService
    private let telemetry: TelemetrySink

    init(
        service: IOSAppStoreUpdateService = .init(),
        telemetry: @escaping TelemetrySink = { _ in }
    ) {
        self.service = service
        self.telemetry = telemetry
    }

    var policyLoadState: IOSAppVersionPolicyLoadState {
        service.policyLoadState
    }

    func refreshPolicy(trigger: String = "runtime_refresh") async -> IOSAppVersionPolicyLoadState {
        do {
            _ = try await service.checkForUpdates()
            reportCheck(trigger: trigger, state: service.policyLoadState)
            return service.policyLoadState
        } catch is CancellationError {
            reportCheck(
                trigger: trigger,
                state: service.policyLoadState,
                cancelled: true
            )
            return service.policyLoadState
        } catch {
            let failed = IOSAppVersionPolicyLoadState.failed(
                message: error.localizedDescription,
                retained: service.policyLoadState.policy
            )
            reportCheck(trigger: trigger, state: failed)
            return failed
        }
    }

    func noteBackendUpdateRequirement(_ required: Bool) {
        guard service.noteBackendUpdateRequirement(required) else { return }
        if required {
            Task { @MainActor [weak self] in
                _ = await self?.refreshPolicy(trigger: "backend_gate")
            }
        }
    }

    private func reportCheck(
        trigger: String,
        state: IOSAppVersionPolicyLoadState,
        cancelled: Bool = false
    ) {
        let result: String
        let errorClass: String?
        if cancelled {
            result = "aborted"
            errorClass = "CancellationError"
        } else {
            switch state {
            case .ready(let policy):
                result = policy.updateAvailable ? "available" : "up-to-date"
                errorClass = nil
            case .failed:
                result = "error"
                errorClass = "policy_fetch_failed"
            case .idle:
                result = "idle"
                errorClass = nil
            case .loading:
                result = "checking"
                errorClass = nil
            }
        }
        telemetry(IOSUpdateTelemetry.check(
            trigger: trigger,
            result: result,
            currentVersion: service.currentVersionForTelemetry(),
            targetVersion: state.policy?.latestVersion,
            errorClass: errorClass
        ))
    }


    private func storeManagedStatusPayload() throws -> CoordinatorPayload {
        let status = try service.statusPayload()
        guard case .object(let object) = status,
              let version = object["currentVersion"],
              let buildNumber = object["buildNumber"],
              let bundleIdentifier = object["bundleIdentifier"],
              let mechanism = object["mechanism"],
              let selfUpdateSupported = object["selfUpdateSupported"]
        else {
            return status
        }
        return .object([
            "type": .string("managed-by-app-store"),
            "version": version,
            "buildNumber": buildNumber,
            "bundleIdentifier": bundleIdentifier,
            "mechanism": mechanism,
            "selfUpdateSupported": selfUpdateSupported,
        ])
    }

    func route(
        method: String,
        args: CoordinatorPayload
    ) async -> CoordinatorReplyOutcome? {
        do {
            switch method {
            case "getUpdateStatus":
                return .ok(try storeManagedStatusPayload())

            case "checkForUpdates":
                let payload = try await service.checkForUpdates()
                reportCheck(trigger: "rpc", state: service.policyLoadState)
                return .ok(payload)

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
                    telemetry(IOSUpdateTelemetry.storeManaged(
                        action: "setUpdateTrack",
                        accepted: false
                    ))
                    return .failed(.init(
                        code: "ios-update-track-managed-by-app-store",
                        message: "App Store/TestFlight controls the iOS release track."
                    ))
                }
                telemetry(IOSUpdateTelemetry.storeManaged(
                    action: "setUpdateTrack",
                    accepted: true
                ))
                return .ok(try service.statusPayload())

            case "quitAndInstallUpdate":
                telemetry(IOSUpdateTelemetry.storeManaged(
                    action: "quitAndInstallUpdate",
                    accepted: false
                ))
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
                    telemetry(IOSUpdateTelemetry.storeManaged(
                        action: "setAutoUpdateWhenIdleOptIn",
                        accepted: false
                    ))
                    return .failed(.init(
                        code: "ios-update-idle-apply-managed-by-app-store",
                        message: "iOS updates are installed by App Store/TestFlight, not by an in-app idle relaunch."
                    ))
                }
                telemetry(IOSUpdateTelemetry.storeManaged(
                    action: "setAutoUpdateWhenIdleOptIn",
                    accepted: true
                ))
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

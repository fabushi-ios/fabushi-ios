import Foundation

@MainActor
final class ProductionCoordinatorProvider {
    let main: IOSMainRuntime
    let runtime: IOSCoordinatorRuntime
    let auxiliary: ProductionCoordinatorAuxiliaryPorts

    init(main: IOSMainRuntime, auxiliary: ProductionCoordinatorAuxiliaryPorts = .live()) {
        self.main = main
        self.auxiliary = auxiliary
        runtime = IOSCoordinatorRuntime(main: main)
    }

    func start() throws -> IOSCoordinatorLaunchHandle {
        let handle = try runtime.start()
        main.coordinator.reapplyClientPauseAfterCoordinatorLaunch()
        return handle
    }

    func restart() throws {
        try runtime.restart()
        main.coordinator.reapplyClientPauseAfterCoordinatorLaunch()
    }

    func sceneDidResume() {
        main.coordinator.hostSettingsTransportConnected()
        auxiliary.onTransportConnected(currentGeneration)
    }

    func transportDown(reason: String) {
        main.coordinator.hostSettingsTransportDown()
        auxiliary.onTransportDown(currentGeneration, reason)
    }

    var currentGeneration: UInt64 {
        switch runtime.state {
        case .running(let generation), .restarting(let generation):
            generation
        case .stopped, .disposed:
            0
        }
    }
}

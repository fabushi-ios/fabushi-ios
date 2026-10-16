import Foundation

@MainActor
final class ProductionCoordinatorProvider {
    let main: IOSMainRuntime
    let runtime: IOSCoordinatorRuntime
    let auxiliary: ProductionCoordinatorAuxiliaryPorts
    private let transportStreamTelemetry: IOSProductionTransportStreamTelemetry

    init(main: IOSMainRuntime, auxiliary: ProductionCoordinatorAuxiliaryPorts = .live()) {
        self.main = main
        self.auxiliary = auxiliary
        transportStreamTelemetry = IOSProductionTransportStreamTelemetry { projection in
            main.lifecycleReporter.report(
                .coordinatorHandoff,
                level: projection.level,
                metadata: projection.metadata
            )
        }
        runtime = IOSCoordinatorRuntime(main: main)
    }

    func start() throws -> IOSCoordinatorLaunchHandle {
        let handle = try runtime.start()
        main.coordinator.reapplyClientPauseAfterCoordinatorLaunch()
        Task { @MainActor [weak main] in
            try? await main?.pushBoxSecrets(trigger: "startup")
        }
        return handle
    }

    func restart() throws {
        try runtime.restart()
        main.coordinator.reapplyClientPauseAfterCoordinatorLaunch()
    }

    func sceneDidResume() {
        main.coordinator.hostSettingsTransportConnected()
        let generation = currentGeneration
        transportStreamTelemetry.onConnected(generation: generation)
        auxiliary.onTransportConnected(generation)
        Task { @MainActor [weak main] in
            try? await main?.pushBoxSecrets(trigger: "retry")
        }
    }

    func transportDown(reason: String) {
        main.coordinator.hostSettingsTransportDown()
        let generation = currentGeneration
        transportStreamTelemetry.onDown(generation: generation, reason: reason)
        auxiliary.onTransportDown(generation, reason)
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

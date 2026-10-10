import Foundation

#if DEBUG
@MainActor
enum IOSDevControlsPreloadEntrypoint {
    static func install(bridge: IOSPreloadBridge) -> IOSDevControlsPreload {
        IOSDevControlsPreload(bridge: bridge)
    }

    static func installIfEnabled(
        bridge: IOSPreloadBridge,
        capability: IOSDevCapability
    ) -> IOSDevControlsPreload? {
        guard capability.preloadKind == .devControls else { return nil }
        return install(bridge: bridge)
    }
}
#endif

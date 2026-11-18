import Foundation
import SwiftUI

/// Coordinates SwiftUI `ScenePhase` transitions with `HostBridgeClient` so SQLite WAL/flock
/// descriptors and WebSocket/Outbox/Tailnet workers suspend cleanly before iOS `0xdead10cc` deadlines.
@MainActor
public final class SceneLifecycleCoordinator: ObservableObject {
    @Published public private(set) var lastKnownPhase: ScenePhase = .active
    @Published public private(set) var isBackgroundSuspended: Bool = false
    @Published public private(set) var backgroundTransitionCount: Int = 0
    @Published public private(set) var foregroundTransitionCount: Int = 0

    private let hostClient: HostBridgeClient

    public init(hostClient: HostBridgeClient = .shared) {
        self.hostClient = hostClient
    }

    public func handleScenePhaseChange(_ newPhase: ScenePhase) async {
        guard newPhase != lastKnownPhase else { return }
        lastKnownPhase = newPhase

        switch newPhase {
        case .background:
            if !isBackgroundSuspended {
                isBackgroundSuspended = true
                backgroundTransitionCount += 1
                _ = try? await hostClient.enterBackground()
            }
        case .active:
            if isBackgroundSuspended {
                isBackgroundSuspended = false
                foregroundTransitionCount += 1
                _ = try? await hostClient.resumeForeground()
            }
        case .inactive:
            break
        @unknown default:
            break
        }
    }
}

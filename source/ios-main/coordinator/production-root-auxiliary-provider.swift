import Foundation

@MainActor
struct ProductionCoordinatorAuxiliaryPorts {
    let detectTimeZone: () -> String?
    let getUserTimeZoneOverride: () -> String?
    let pushBoxSecrets: () async throws -> Void
    let onTransportConnected: (UInt64) -> Void
    let onTransportDown: (UInt64, String) -> Void

    static func live() -> ProductionCoordinatorAuxiliaryPorts {
        .init(
            detectTimeZone: { TimeZone.current.identifier },
            getUserTimeZoneOverride: { nil },
            pushBoxSecrets: {},
            onTransportConnected: { _ in },
            onTransportDown: { _, _ in }
        )
    }
}

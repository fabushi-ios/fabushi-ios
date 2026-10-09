import SwiftUI

/// Native iOS production renderer root corresponding to Grok frontend/production.
///
/// During decomposition this root owns the transition from the legacy shell into
/// recursively mapped SwiftUI feature modules. Runtime access is only through
/// IOSPreloadBridge.
internal struct ProductionRenderer: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    let bridge: IOSPreloadBridge
    let appAgentSurface: FabushiAppAgentSurface
    var reconnectGeneration: Int = 0
    let onRetryConnection: @MainActor () async -> Void

    var body: some View {
        GrokMobileShell(
            model: model,
            messaging: messaging,
            bridge: bridge,
            appAgentSurface: appAgentSurface,
            reconnectGeneration: reconnectGeneration,
            onRetryConnection: onRetryConnection
        )
    }
}

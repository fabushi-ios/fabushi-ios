import SwiftUI

internal struct IOSRequiredUpdateView: View {
    let policy: IOSAppVersionPolicy
    let onUpdate: @MainActor () -> Void
    let onRetry: @MainActor () -> Void

    var body: some View {
        VStack(spacing: 18) {
            Image(systemName: "arrow.down.circle.fill")
                .font(.system(size: 46, weight: .semibold))
                .accessibilityHidden(true)
            Text(policy.title)
                .font(.title2.weight(.semibold))
                .multilineTextAlignment(.center)
            Text(policy.message)
                .multilineTextAlignment(.center)
            Text("Latest version: \(policy.latestVersion) (\(policy.latestBuildNumber))")
                .font(.subheadline)
                .foregroundStyle(.secondary)
            if !policy.releaseNotes.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(policy.releaseNotes, id: \.self) { note in
                        Text("• \(note)")
                    }
                }
                .font(.footnote)
                .frame(maxWidth: 460, alignment: .leading)
            }
            HStack(spacing: 12) {
                Button("Check Again") { onRetry() }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("required-update-retry")
                Button("Update") { onUpdate() }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("required-update-open-store")
            }
        }
        .padding(28)
        .frame(maxWidth: 560)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24))
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Update Required")
        .accessibilityAddTraits(.isModal)
        .accessibilityIdentifier("required-update-surface")
    }
}

internal struct IOSOptionalUpdatePill: View {
    let policy: IOSAppVersionPolicy
    let onUpdate: @MainActor () -> Void

    var body: some View {
        Button {
            onUpdate()
        } label: {
            Label(
                "Update \(policy.latestVersion)",
                systemImage: "arrow.down.circle"
            )
            .font(.subheadline.weight(.semibold))
        }
        .buttonStyle(.borderedProminent)
        .accessibilityLabel("Update available: \(policy.latestVersion)")
        .accessibilityIdentifier("optional-update-pill")
    }
}

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
    let appVersionPolicyState: IOSAppVersionPolicyLoadState
    let onRetryAppVersionPolicy: @MainActor () -> Void
    let onOpenUpdateURL: @MainActor (URL) -> Void

    var body: some View {
        ZStack {
            GrokMobileShell(
            model: model,
            messaging: messaging,
            bridge: bridge,
            appAgentSurface: appAgentSurface,
            reconnectGeneration: reconnectGeneration,
                onRetryConnection: onRetryConnection
            )

            if let policy = appVersionPolicyState.policy, policy.isRequired {
                IOSRequiredUpdateView(
                    policy: policy,
                    onUpdate: {
                        guard let url = policy.downloadURL else { return }
                        onOpenUpdateURL(url)
                    },
                    onRetry: onRetryAppVersionPolicy
                )
            } else if let policy = appVersionPolicyState.policy,
                      policy.updateAvailable,
                      policy.strategy == .optional {
                IOSOptionalUpdatePill(
                    policy: policy,
                    onUpdate: {
                        guard let url = policy.downloadURL else { return }
                        onOpenUpdateURL(url)
                    }
                )
                .padding(.top, 12)
                .padding(.trailing, 12)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
            }
        }
    }
}

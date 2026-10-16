import SwiftUI

let ROSTER_PRIVACY_SETTINGS_URL = URL(
    string: "https://cursor.com/dashboard/settings?openPrivacy=true"
)!

func isMobileRosterPrivacyBlocked(
    access: AccessCoverSandAccess,
    failure: AccessRosterFailure?
) -> Bool {
    if access.reason == .teamPrivacyMode { return true }
    let code = failure?.code.uppercased()
    let transport = failure?.transportKind?.lowercased()
    return code == "CLOUD_AGENT_STORAGE_DISABLED" || transport == "no_storage"
}

struct MobileRosterPrivacyBlockedView: View {
    let onSignOut: @MainActor () async -> Void
    let onOpenSettings: @MainActor () -> Void

    @State private var busy = false

    var body: some View {
        VStack(spacing: 20) {
            Image(systemName: "hand.raised.fill")
                .font(.system(size: 42, weight: .semibold))
                .accessibilityHidden(true)
            Text("Update Privacy Mode")
                .font(.title2.weight(.semibold))
            Text("Privacy Mode (Legacy) isn’t compatible with Fabushi. Switch to Privacy Mode to start using Fabushi — data still isn’t used for training.")
                .multilineTextAlignment(.center)
            Text("This setting is shared with Cursor. Leaving Legacy can’t be undone.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            HStack(spacing: 12) {
                Button("Sign out", role: .destructive) {
                    guard !busy else { return }
                    busy = true
                    Task { @MainActor in
                        await onSignOut()
                        busy = false
                    }
                }
                .buttonStyle(.bordered)
                .disabled(busy)
                .accessibilityIdentifier("roster-privacy-sign-out")

                Button("Open Privacy Settings") {
                    onOpenSettings()
                }
                .buttonStyle(.borderedProminent)
                .disabled(busy)
                .accessibilityIdentifier("roster-privacy-open-settings")
            }
        }
        .padding(28)
        .frame(maxWidth: 520)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 22))
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Update Privacy Mode")
        .accessibilityAddTraits(.isModal)
        .accessibilityIdentifier("roster-privacy-blocked")
    }
}

import SwiftUI

enum MobileRosterStatusKind: String, Equatable, Sendable {
    case loading
    case empty
    case allHidden
    case error
}

enum MobileRosterStatusProjection {
    static func project(
        roster: AccessRosterSnapshot,
        bots: [MobileBotSummary]
    ) -> MobileRosterStatusKind? {
        if roster.transport == .connecting && !roster.hasCompleteRoster {
            return .loading
        }
        if roster.transport == .down && roster.failure != nil && bots.isEmpty {
            return .error
        }
        if roster.hasCompleteRoster && bots.isEmpty {
            return .empty
        }
        if !bots.isEmpty && bots.allSatisfy(\.hidden) {
            return .allHidden
        }
        return nil
    }
}

struct MobileRosterStatusView: View {
    let kind: MobileRosterStatusKind
    let onShowHiddenBots: @MainActor () -> Void

    @ViewBuilder
    var body: some View {
        switch kind {
        case .empty:
            Text("No saved agents yet.")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
                .accessibilityIdentifier("roster-status-empty")
        case .allHidden:
            HStack {
                Text("All bots are hidden")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Show Hidden Bots") { onShowHiddenBots() }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("roster-status-show-hidden")
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 12)
            .accessibilityIdentifier("roster-status-all-hidden")
        case .loading, .error:
            // Root connection resilience owns these two visible states so the
            // app has one Retry action and one Coordinator recovery owner.
            EmptyView()
        }
    }
}

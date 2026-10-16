import SwiftUI

enum MobileCoordinatorConnectionPhase: String, Equatable, Sendable {
    case hidden
    case loading
    case connected
    case reconnecting
    case unreachable
}

struct MobileCoordinatorConnectionSnapshot: Equatable, Sendable {
    let phase: MobileCoordinatorConnectionPhase
    let isRetrying: Bool
    let failureCode: String?
}

enum MobileCoordinatorConnectionProjection {
    static func project(
        loggedIn: Bool,
        access: AccessCoverSandAccess,
        roster: AccessRosterSnapshot,
        firstBox: FirstBoxGateState,
        isRetrying: Bool
    ) -> MobileCoordinatorConnectionSnapshot {
        let isPrivacyBlocked = access.reason == .teamPrivacyMode
        let phase: MobileCoordinatorConnectionPhase
        if !loggedIn || isPrivacyBlocked {
            phase = .hidden
        } else if roster.transport == .connecting {
            phase = .loading
        } else if roster.transport == .connected && roster.failure == nil {
            phase = .connected
        } else if firstBox.hasReachedBox {
            phase = .reconnecting
        } else {
            phase = .unreachable
        }
        return .init(
            phase: phase,
            isRetrying: isRetrying,
            failureCode: roster.failure?.code
        )
    }
}

struct MobileCoordinatorConnectionNotice: View {
    let snapshot: MobileCoordinatorConnectionSnapshot
    let onRetry: @MainActor () -> Void

    @ViewBuilder
    var body: some View {
        switch snapshot.phase {
        case .hidden, .connected:
            EmptyView()
        case .loading:
            HStack(spacing: 10) {
                ProgressView()
                Text("正在连接 Fabushi…")
                    .font(.subheadline.weight(.medium))
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(.regularMaterial, in: Capsule())
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Connecting to Fabushi")
            .accessibilityIdentifier("fabushi-connection-loading")
        case .reconnecting:
            connectionCard(
                title: "正在重新连接",
                detail: "已保留当前 Agent 列表；连接恢复后会自动重新同步。",
                accessibilityIdentifier: "fabushi-connection-reconnecting"
            )
        case .unreachable:
            connectionCard(
                title: "暂时无法连接 Fabushi",
                detail: "尚未取得可用 Agent 列表。请重试连接。",
                accessibilityIdentifier: "fabushi-connection-unreachable"
            )
        }
    }

    private func connectionCard(
        title: String,
        detail: String,
        accessibilityIdentifier: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Image(systemName: "wifi.exclamationmark")
                    .accessibilityHidden(true)
                Text(title)
                    .font(.headline)
                Spacer()
                if snapshot.isRetrying {
                    ProgressView()
                        .controlSize(.small)
                        .accessibilityLabel("Retrying")
                }
            }
            Text(detail)
                .font(.subheadline)
                .foregroundStyle(.secondary)
            Button(snapshot.isRetrying ? "正在重试…" : "重试") {
                onRetry()
            }
            .buttonStyle(.borderedProminent)
            .disabled(snapshot.isRetrying)
            .accessibilityIdentifier("fabushi-connection-retry")
        }
        .padding(16)
        .frame(maxWidth: 420, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(accessibilityIdentifier)
    }
}

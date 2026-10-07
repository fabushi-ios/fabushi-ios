import Observation
import SwiftUI

enum SurfaceNoticeKind: String, CaseIterable, Equatable, Hashable, Sendable {
    case success
    case error
}

enum SettingsNoticeOperation: String, CaseIterable, Equatable, Hashable, Sendable {
    case load = "settings-load"
    case account = "settings-account"
    case autoReview = "settings-auto-review"
    case theme = "settings-theme"
    case localToolPermission = "settings-local-tool-permission"
    case securityKey = "settings-security-key"
    case timeZone = "settings-time-zone"
    case routerProvider = "settings-router-provider"
    case usageCancelTrial = "settings-usage-cancel-trial"
    case updateCheck = "settings-update-check"
    case updateInstall = "settings-update-install"
    case updateAutoUpdateWhenIdle = "settings-update-auto-update-when-idle"
    case updateTrack = "settings-update-track"
}

enum PluginsNoticeOperation: String, CaseIterable, Equatable, Hashable, Sendable {
    case load = "plugins-load"
    case privateSkillsLoad = "plugins-private-skills-load"
    case privateSkillDelete = "plugins-private-skill-delete"
    case privateSkillUpdate = "plugins-private-skill-update"
    case privateSkillToggle = "plugins-private-skill-toggle"
    case privateSkillSync = "plugins-private-skill-sync"
    case authenticate = "plugins-authenticate"
    case browserRemove = "plugins-browser-remove"
    case accountRename = "plugins-account-rename"
    case accountRemove = "plugins-account-remove"
    case install = "plugins-install"
    case editSetup = "plugins-edit-setup"
    case remove = "plugins-remove"
    case serverToolsLoad = "plugins-server-tools-load"
    case serverToolToggle = "plugins-server-tool-toggle"
}

enum RootSettingsNoticeOperation: Equatable, Hashable, Sendable {
    case settings(SettingsNoticeOperation)
    case plugins(PluginsNoticeOperation)

    var rawValue: String {
        switch self {
        case .settings(let operation): operation.rawValue
        case .plugins(let operation): operation.rawValue
        }
    }
}

struct RootSettingsNoticeEvent: Equatable, Hashable, Sendable {
    let kind: SurfaceNoticeKind
    let operation: RootSettingsNoticeOperation
    let message: String
}

enum SettingsNoticeSurface: String, Equatable, Hashable, Sendable {
    case none
    case settings
    case plugins
}

struct SettingsNoticeFence: Equatable, Hashable, Sendable {
    let scopeKey: String
    let generation: Int
}

struct SettingsNoticeSnapshot: Equatable, Hashable, Sendable {
    let event: RootSettingsNoticeEvent
    let revision: Int
}

@MainActor
@Observable
final class SettingsNoticeController {
    private(set) var snapshot: SettingsNoticeSnapshot?

    @ObservationIgnored private var scopeKey = "unbound:none"
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var revision = 0
    @ObservationIgnored private var disposed = false

    func updateScope(accountKey: String, surface: SettingsNoticeSurface) {
        guard !disposed else { return }
        let normalizedAccount = accountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let nextScope = "\(normalizedAccount.isEmpty ? "unknown" : normalizedAccount):\(surface.rawValue)"
        guard nextScope != scopeKey else { return }
        scopeKey = nextScope
        generation = generation == Int.max ? 1 : generation + 1
        snapshot = nil
    }

    func makeFence() -> SettingsNoticeFence {
        SettingsNoticeFence(scopeKey: scopeKey, generation: generation)
    }

    func publish(_ event: RootSettingsNoticeEvent, fence: SettingsNoticeFence? = nil) {
        guard !disposed else { return }
        if let fence {
            guard fence.scopeKey == scopeKey, fence.generation == generation else { return }
        }
        revision = revision == Int.max ? 1 : revision + 1
        snapshot = SettingsNoticeSnapshot(event: event, revision: revision)
    }

    func reset() {
        guard !disposed, snapshot != nil else { return }
        snapshot = nil
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        snapshot = nil
    }
}

enum SettingsNoticePresentationPolicy {
    static func dismissDelayMilliseconds(for kind: SurfaceNoticeKind) -> Int {
        switch kind {
        case .success: 3_500
        case .error: 6_000
        }
    }
}

@MainActor
struct SettingsNoticeView: View {
    let controller: SettingsNoticeController
    @State private var expired = false

    var body: some View {
        Group {
            if let snapshot = controller.snapshot, !expired {
                let notice = snapshot.event
                HStack(spacing: 10) {
                    Image(systemName: notice.kind == .error ? "xmark.circle.fill" : "checkmark.circle.fill")
                        .foregroundStyle(notice.kind == .error ? Color.red : Color.green)
                    Text(notice.message)
                        .font(.subheadline)
                        .lineLimit(3)
                    Spacer(minLength: 4)
                    Button {
                        controller.reset()
                    } label: {
                        Image(systemName: "xmark")
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dismiss")
                    .accessibilityIdentifier("settings-notice-dismiss")
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .shadow(radius: 3)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("settings-notice")
            }
        }
        .task(id: controller.snapshot?.revision) {
            expired = false
            guard let snapshot = controller.snapshot else { return }
            let delay = SettingsNoticePresentationPolicy.dismissDelayMilliseconds(
                for: snapshot.event.kind
            )
            do {
                try await Task.sleep(for: .milliseconds(delay))
            } catch {
                return
            }
            guard controller.snapshot?.revision == snapshot.revision else { return }
            expired = true
        }
    }
}

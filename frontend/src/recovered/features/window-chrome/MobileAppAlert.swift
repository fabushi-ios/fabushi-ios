import Observation
import SwiftUI

@MainActor
@Observable
final class MobileAppAlertController {
    typealias Completion = @MainActor () async throws -> String?

    struct SecondaryAction {
        let label: String
        let destructive: Bool
        let perform: Completion?
    }

    struct Request {
        let title: String
        let description: String?
        let body: String?
        let warning: String?
        let confirmLabel: String
        let pendingLabel: String?
        let cancelLabel: String?
        let destructive: Bool
        let secondary: SecondaryAction?
        let wide: Bool
        let perform: Completion?
    }

    struct State {
        let request: Request
        let isPerforming: Bool
        let failure: String?
    }

    private struct Pending {
        let request: Request
        let settle: (Bool) -> Void
    }

    private(set) var state: State?
    private var active: Pending?
    private var waiting: Pending?
    private var epoch = 0
    private var disposed = false

    func alert(_ request: Request) async -> Bool {
        guard !disposed else { return false }
        return await withCheckedContinuation { continuation in
            let pending = Pending(
                request: request,
                settle: { continuation.resume(returning: $0) }
            )
            if active == nil {
                show(pending)
                return
            }

            // Match the Desktop contract: a cancellable decision is never
            // silently queued, and only one non-cancellable follow-up exists.
            if request.cancelLabel != nil || waiting != nil {
                continuation.resume(returning: false)
                return
            }
            waiting = pending
        }
    }

    func confirm() {
        perform { $0.perform }
    }

    func confirmSecondary() {
        perform { $0.secondary?.perform }
    }

    func cancel() {
        guard state?.isPerforming != true else { return }
        settleActive(false)
    }

    func reset() {
        let queued = waiting
        waiting = nil
        queued?.settle(false)
        settleActive(false)
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        let queued = waiting
        waiting = nil
        queued?.settle(false)
        settleActive(false)
    }

    private func show(_ pending: Pending) {
        active = pending
        state = .init(
            request: pending.request,
            isPerforming: false,
            failure: nil
        )
    }

    private func promoteWaiting() {
        guard !disposed, let next = waiting else { return }
        waiting = nil
        show(next)
    }

    private func settleActive(_ accepted: Bool) {
        guard let current = active else { return }
        active = nil
        epoch &+= 1
        state = nil
        promoteWaiting()
        current.settle(accepted)
    }

    private func perform(
        _ action: (Request) -> Completion?
    ) {
        guard let active,
              let state,
              !state.isPerforming
        else { return }

        guard let perform = action(active.request) else {
            settleActive(true)
            return
        }

        let actionEpoch = epoch
        self.state = .init(
            request: state.request,
            isPerforming: true,
            failure: nil
        )
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let reportedFailure = try await perform()
                guard actionEpoch == self.epoch,
                      self.active != nil,
                      let current = self.state
                else { return }
                if let reportedFailure {
                    self.state = .init(
                        request: current.request,
                        isPerforming: false,
                        failure: reportedFailure
                    )
                } else {
                    self.settleActive(true)
                }
            } catch {
                guard actionEpoch == self.epoch,
                      self.active != nil,
                      let current = self.state
                else { return }
                self.state = .init(
                    request: current.request,
                    isPerforming: false,
                    failure: error.localizedDescription
                )
            }
        }
    }
}

@MainActor
struct MobileAppAlertHost: View {
    @Bindable var controller: MobileAppAlertController

    var body: some View {
        if let state = controller.state {
            VStack(alignment: .leading, spacing: 14) {
                Text(state.request.title)
                    .font(.title3.weight(.semibold))
                    .accessibilityIdentifier("app-alert-title")
                if let description = state.request.description {
                    Text(description)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("app-alert-description")
                }
                if let body = state.request.body {
                    Text(body)
                }
                if let warning = state.request.warning {
                    Text(warning)
                        .font(.callout.weight(.semibold))
                }
                if let failure = state.failure {
                    Text(failure)
                        .foregroundStyle(.red)
                        .accessibilityLabel("Error: \(failure)")
                        .accessibilityIdentifier("app-alert-failure")
                }
                HStack {
                    Spacer()
                    if let cancel = state.request.cancelLabel {
                        Button(cancel) { controller.cancel() }
                            .buttonStyle(.bordered)
                            .disabled(state.isPerforming)
                            .accessibilityIdentifier("app-alert-cancel")
                    }
                    if let secondary = state.request.secondary {
                        Button(
                            secondary.label,
                            role: secondary.destructive ? .destructive : nil
                        ) {
                            controller.confirmSecondary()
                        }
                        .buttonStyle(.bordered)
                        .disabled(state.isPerforming)
                        .accessibilityIdentifier("app-alert-secondary")
                    }
                    Button(
                        state.isPerforming
                            ? state.request.pendingLabel ?? state.request.confirmLabel
                            : state.request.confirmLabel,
                        role: state.request.destructive ? .destructive : nil
                    ) {
                        controller.confirm()
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(state.isPerforming)
                    .accessibilityIdentifier("app-alert-confirm")
                }
            }
            .padding(20)
            .frame(maxWidth: state.request.wide ? 500 : 380)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(.black.opacity(0.18))
            .accessibilityElement(children: .contain)
            .accessibilityAddTraits(.isModal)
            .accessibilityIdentifier("app-alert-host")
        }
    }
}

import SwiftUI

/// iOS renderer adaptation of Grok's account sign-in status surface.
///
/// The renderer owns presentation only. Authentication state and commands remain
/// on MarketplaceModel -> IOSPreloadBridge -> iOS main -> Coordinator -> Rust Host.
enum AccountSignInPhase: Equatable {
    case idle
    case starting
    case awaitingBrowser(attemptID: String)

    static func resolve(attemptID: String?, busy: Bool) -> AccountSignInPhase {
        if let attemptID, !attemptID.isEmpty {
            return .awaitingBrowser(attemptID: attemptID)
        }
        return busy ? .starting : .idle
    }
}

struct AccountSignInStatusView: View {
    @Bindable var model: MarketplaceModel

    private var phase: AccountSignInPhase {
        AccountSignInPhase.resolve(
            attemptID: model.browserLoginAttemptId,
            busy: model.loginBusy
        )
    }

    var body: some View {
        VStack(spacing: 10) {
            if model.loginError != nil {
                Text("登录暂时不可用，请稍后重试。")
                    .font(.footnote)
                    .foregroundStyle(Color.red.opacity(0.82))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24)
                    .accessibilityIdentifier("mobile-login-error")
            }

            switch phase {
            case .awaitingBrowser:
                Button("继续登录") {
                    Task { await model.reopenBrowserLogin() }
                }
                .font(.system(size: 18, weight: .bold))
                .foregroundStyle(.white)
                .frame(maxWidth: .infinity, minHeight: 58)
                .background(.black, in: Capsule())
                .accessibilityIdentifier("mobile-login-reopen")

                Button("取消登录") {
                    Task { await model.cancelBrowserLogin() }
                }
                .font(.footnote.weight(.semibold))
                .foregroundStyle(Color.black.opacity(0.46))
                .accessibilityIdentifier("mobile-login-cancel")

            case .idle, .starting:
                Button {
                    Task { await model.beginBrowserLogin() }
                } label: {
                    HStack(spacing: 10) {
                        if model.loginBusy { ProgressView().tint(.white) }
                        Text(model.loginBusy ? "正在准备…" : "登录")
                    }
                    .font(.system(size: 18, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(maxWidth: .infinity, minHeight: 58)
                    .background(.black, in: Capsule())
                }
                .disabled(model.loginBusy)
                .accessibilityIdentifier("mobile-login-browser")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-login-status")
    }
}

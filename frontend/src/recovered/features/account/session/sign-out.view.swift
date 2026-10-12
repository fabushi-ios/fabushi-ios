import SwiftUI

enum AccountSignOutResult: Equatable {
    case signedOut
    case failed(message: String)
}

func accountSignOutResult(isLoggedIn: Bool, message: String) -> AccountSignOutResult {
    if !isLoggedIn { return .signedOut }
    let trimmed = message.trimmingCharacters(in: .whitespacesAndNewlines)
    return .failed(message: trimmed.isEmpty ? "退出登录失败，请重试。" : trimmed)
}

/// iOS renderer adaptation of Grok's sign-out alert dialog.
///
/// The web overlay/focus-trap maps to a native SwiftUI confirmation surface.
/// The renderer never clears auth locally: it closes only after MarketplaceModel
/// receives the Host-projected logged-out state through the trusted bridge.
struct AccountSignOutDialogView: View {
    @Bindable var model: MarketplaceModel
    let onCancel: () -> Void
    let onSignedOut: () -> Void

    @State private var busy = false
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 18) {
                Text("退出登录？")
                    .font(.title2.bold())
                Text("你将退出当前 Fabushi 账号。此操作不会在界面层删除或伪造账号状态。")
                    .foregroundStyle(.secondary)

                if let errorMessage {
                    Text(errorMessage)
                        .font(.footnote)
                        .foregroundStyle(.red)
                        .accessibilityIdentifier("mobile-logout-error")
                }

                Spacer(minLength: 8)

                HStack(spacing: 12) {
                    Button("取消") { onCancel() }
                        .buttonStyle(.bordered)
                        .disabled(busy)
                        .accessibilityIdentifier("mobile-logout-cancel")

                    Button(role: .destructive) {
                        Task { @MainActor in
                            await confirmSignOut()
                        }
                    } label: {
                        HStack(spacing: 8) {
                            if busy { ProgressView() }
                            Text(busy ? "正在退出…" : "退出登录")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(busy)
                    .accessibilityIdentifier("mobile-logout-confirm")
                }
            }
            .padding(24)
            .navigationTitle("账号")
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium])
        .interactiveDismissDisabled(busy)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-logout-dialog")
    }

    @MainActor
    private func confirmSignOut() async {
        guard !busy else { return }
        busy = true
        errorMessage = nil
        await model.logout()
        busy = false

        switch accountSignOutResult(isLoggedIn: model.loggedIn, message: model.message) {
        case .signedOut:
            onSignedOut()
        case .failed(let message):
            errorMessage = message
        }
    }
}

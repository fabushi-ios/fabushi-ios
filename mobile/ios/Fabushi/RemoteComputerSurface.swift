import SwiftUI
import UIKit
import WebKit

private let remoteComputerOriginHost = "fabushi.ombhrum.com"
private let remoteComputerURL = URL(string: "https://fabushi.ombhrum.com/remote-computer")!

/// A deliberately narrow browser surface for human-operated remote computer sessions.
///
/// Unlike `MiniAppWebMcpSurface`, this view never registers a native script handler or
/// injects a native bridge. The hosted page can use normal browser APIs (including
/// WebRTC), but it cannot call arbitrary native application capabilities.
struct RemoteComputerSurface: View {
    let onClose: () -> Void

    @State private var status = "正在连接我的电脑…"
    @State private var errorMessage: String?
    @State private var reloadToken = 0

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Button("返回", action: onClose)
                    .accessibilityIdentifier("remote-computer-close")

                VStack(alignment: .leading, spacing: 2) {
                    Text("我的电脑")
                        .font(.headline)
                    Text(status)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("remote-computer-status")
                }

                Spacer()

                if errorMessage == nil && status != "已安全连接" {
                    ProgressView()
                        .controlSize(.small)
                        .accessibilityIdentifier("remote-computer-loading")
                }
            }
            .padding(12)

            if let errorMessage {
                VStack(alignment: .leading, spacing: 10) {
                    Text("无法打开远程电脑")
                        .font(.headline)
                    Text(errorMessage)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Button("重新加载") {
                        self.errorMessage = nil
                        status = "正在重新连接…"
                        reloadToken += 1
                    }
                    .accessibilityIdentifier("remote-computer-reload")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
                .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
                .accessibilityIdentifier("remote-computer-error")
            }

            RemoteComputerWebView(
                reloadToken: reloadToken,
                status: $status,
                errorMessage: $errorMessage
            )
        }
        .background(Color(uiColor: .systemBackground))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("remote-computer-surface")
    }
}

private struct RemoteComputerWebView: UIViewRepresentable {
    let reloadToken: Int
    @Binding var status: String
    @Binding var errorMessage: String?

    func makeCoordinator() -> Coordinator {
        Coordinator(status: $status, errorMessage: $errorMessage)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.mediaTypesRequiringUserActionForPlayback = []

        // This restricted surface deliberately has no native bridge or injected script.
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.accessibilityIdentifier = "remote-computer-webview"
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = true
        webView.isInspectable = false
        context.coordinator.webView = webView
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        guard context.coordinator.loadedReloadToken != reloadToken else { return }
        context.coordinator.loadedReloadToken = reloadToken
        webView.load(URLRequest(url: remoteComputerURL, cachePolicy: .useProtocolCachePolicy))
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.stopLoading()
        webView.navigationDelegate = nil
        coordinator.webView = nil
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        @Binding private var status: String
        @Binding private var errorMessage: String?
        weak var webView: WKWebView?
        var loadedReloadToken: Int?

        init(status: Binding<String>, errorMessage: Binding<String?>) {
            _status = status
            _errorMessage = errorMessage
        }

        func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation?) {
            errorMessage = nil
            status = "正在安全连接…"
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation?) {
            if errorMessage == nil {
                status = "已安全连接"
            }
        }

        func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation?,
            withError error: Error
        ) {
            handleNavigationError(error)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation?, withError error: Error) {
            handleNavigationError(error)
        }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            status = "连接已中断"
            errorMessage = "远程电脑页面已停止响应，请重新加载。"
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void
        ) {
            guard let url = navigationAction.request.url else {
                decisionHandler(.cancel)
                return
            }

            if isAllowedRemoteComputerURL(url) {
                if navigationAction.targetFrame == nil {
                    webView.load(URLRequest(url: url))
                    decisionHandler(.cancel)
                } else {
                    decisionHandler(.allow)
                }
                return
            }

            if navigationAction.targetFrame?.isMainFrame != false {
                status = "已阻止外部导航"
                errorMessage = "远程电脑页面只允许访问 https://fabushi.ombhrum.com。"
            }
            decisionHandler(.cancel)
        }

        private func handleNavigationError(_ error: Error) {
            let nsError = error as NSError
            guard nsError.code != NSURLErrorCancelled else { return }
            status = "连接失败"
            errorMessage = nsError.localizedDescription
        }

        private func isAllowedRemoteComputerURL(_ url: URL) -> Bool {
            url.scheme?.lowercased() == "https"
                && url.host?.lowercased() == remoteComputerOriginHost
                && url.user == nil
                && url.password == nil
                && (url.port == nil || url.port == 443)
        }


    }
}

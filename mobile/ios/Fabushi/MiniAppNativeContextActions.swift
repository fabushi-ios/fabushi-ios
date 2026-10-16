import Foundation
import UIKit
import WebKit

let miniAppNativeContextMenuHandler = "fabushiNativeContextMenu"
let miniAppNativeContextMenuJavaScript = """
(function(){
  if (window.__fabushiNativeContextMenuInstalled) return;
  Object.defineProperty(window, '__fabushiNativeContextMenuInstalled', {
    configurable: false,
    value: true
  });
  document.addEventListener('contextmenu', function(event) {
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return;
    const image = target.closest('img');
    const link = target.closest('a[href]');
    const srcURL = image ? String(image.currentSrc || image.src || '') : '';
    const linkURL = link ? String(link.href || '') : '';
    if (!srcURL && !linkURL) return;
    event.preventDefault();
    window.webkit.messageHandlers.fabushiNativeContextMenu.postMessage({
      srcURL,
      linkURL
    });
  }, true);
})();
"""

@MainActor
final class MiniAppNativeContextActions {
    private static let maxImageBytes = 25 * 1024 * 1024

    func present(
        payload: [String: Any],
        from webView: WKWebView
    ) {
        let srcURL = (payload["srcURL"] as? String ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let linkURL = (payload["linkURL"] as? String ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !srcURL.isEmpty || !linkURL.isEmpty,
              let presenter = Self.topViewController()
        else { return }

        let sheet = UIAlertController(
            title: nil,
            message: nil,
            preferredStyle: .actionSheet
        )

        if let link = Self.httpURL(linkURL) {
            sheet.addAction(UIAlertAction(title: "打开链接", style: .default) { _ in
                UIApplication.shared.open(link)
            })
            sheet.addAction(UIAlertAction(title: "复制链接地址", style: .default) { _ in
                UIPasteboard.general.string = link.absoluteString
            })
        }

        if !srcURL.isEmpty {
            sheet.addAction(UIAlertAction(title: "复制图片", style: .default) { [weak self, weak webView] _ in
                guard let self, let webView else { return }
                Task { @MainActor in
                    guard let image = await self.loadImage(srcURL, webView: webView) else { return }
                    UIPasteboard.general.image = image
                }
            })
            sheet.addAction(UIAlertAction(title: "保存/分享图片…", style: .default) { [weak self, weak webView] _ in
                guard let self, let webView else { return }
                Task { @MainActor in
                    guard let image = await self.loadImage(srcURL, webView: webView),
                          let presenter = Self.topViewController()
                    else { return }
                    let share = UIActivityViewController(
                        activityItems: [image],
                        applicationActivities: nil
                    )
                    if let popover = share.popoverPresentationController {
                        popover.sourceView = webView
                        popover.sourceRect = CGRect(
                            x: webView.bounds.midX,
                            y: webView.bounds.midY,
                            width: 1,
                            height: 1
                        )
                    }
                    presenter.present(share, animated: true)
                }
            })
            sheet.addAction(UIAlertAction(title: "复制图片地址", style: .default) { _ in
                UIPasteboard.general.string = srcURL
            })
        }

        sheet.addAction(UIAlertAction(title: "取消", style: .cancel))
        if let popover = sheet.popoverPresentationController {
            popover.sourceView = webView
            popover.sourceRect = CGRect(
                x: webView.bounds.midX,
                y: webView.bounds.midY,
                width: 1,
                height: 1
            )
        }
        presenter.present(sheet, animated: true)
    }

    private func loadImage(_ raw: String, webView: WKWebView) async -> UIImage? {
        if raw.lowercased().hasPrefix("data:image/") {
            return Self.imageFromDataURL(raw)
        }
        guard let url = Self.httpURL(raw) else { return nil }
        do {
            var request = URLRequest(url: url)
            request.timeoutInterval = 20
            request.setValue("image/*", forHTTPHeaderField: "Accept")
            let (data, response) = try await URLSession.shared.data(for: request)
            guard data.count > 0,
                  data.count <= Self.maxImageBytes,
                  let http = response as? HTTPURLResponse,
                  (200..<300).contains(http.statusCode),
                  let mime = http.value(forHTTPHeaderField: "Content-Type")?.lowercased(),
                  mime.hasPrefix("image/"),
                  let image = UIImage(data: data)
            else { return nil }
            return image
        } catch {
            return nil
        }
    }

    static func httpURL(_ raw: String) -> URL? {
        guard let url = URL(string: raw),
              let scheme = url.scheme?.lowercased(),
              (scheme == "http" || scheme == "https"),
              url.host != nil
        else { return nil }
        return url
    }

    static func imageFromDataURL(_ raw: String) -> UIImage? {
        guard raw.lowercased().hasPrefix("data:image/"),
              let comma = raw.firstIndex(of: ",")
        else { return nil }
        let header = String(raw[raw.startIndex..<comma]).lowercased()
        let payload = String(raw[raw.index(after: comma)...])
        let data: Data?
        if header.hasSuffix(";base64") {
            data = Data(base64Encoded: payload, options: [.ignoreUnknownCharacters])
        } else {
            data = payload.removingPercentEncoding?.data(using: .utf8)
        }
        guard let data,
              !data.isEmpty,
              data.count <= maxImageBytes
        else { return nil }
        return UIImage(data: data)
    }

    private static func topViewController() -> UIViewController? {
        let scene = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .first { $0.activationState == .foregroundActive }
        var controller = scene?.windows.first(where: \.isKeyWindow)?.rootViewController
        while let presented = controller?.presentedViewController {
            controller = presented
        }
        return controller
    }
}

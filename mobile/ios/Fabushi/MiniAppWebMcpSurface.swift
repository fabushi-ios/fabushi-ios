import SwiftUI
import UIKit
import WebKit

private let webMcpOriginHost = "fabushi.ombhrum.com"
private let localWebMcpOriginHost = "miniapp.local.fabushi.invalid"
private let webMcpMessageHandler = "fabushiWebMcp"

func isTrustedWebMcpBridgeHost(_ host: String?) -> Bool {
    guard let host else { return false }
    return host == webMcpOriginHost || host == localWebMcpOriginHost
}

struct MiniAppWebMcpBridgeSession: Equatable {
    let pluginInstanceId: String
    let nonce: String
    let grants: Set<String>

    static func fresh(plugin: MarketplacePlugin) -> Self {
        Self(
            pluginInstanceId: "\(plugin.pluginId):\(UUID().uuidString)",
            nonce: UUID().uuidString.replacingOccurrences(of: "-", with: ""),
            grants: Set(plugin.tools.map(\.name))
        )
    }

    func allows(pluginInstanceId: String, nonce: String, toolName: String) -> Bool {
        self.pluginInstanceId == pluginInstanceId
            && self.nonce == nonce
            && grants.contains(toolName)
    }
}

struct MiniAppWebMcpSurface: View {
    let plugin: MarketplacePlugin
    let model: MarketplaceModel
    let localHtmlOverride: String?

    init(plugin: MarketplacePlugin, model: MarketplaceModel, localHtmlOverride: String? = nil) {
        self.plugin = plugin
        self.model = model
        self.localHtmlOverride = localHtmlOverride
    }

    @Environment(\.dismiss) private var dismiss
    @State private var status = "正在解析本地 WebMCP…"
    @State private var localHtml: String?
    @State private var webMcpPlugin: MarketplacePlugin?
    @State private var sourceResolved = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Button("返回") { dismiss() }
                    .accessibilityIdentifier("miniapp-webmcp-close")

                VStack(alignment: .leading, spacing: 2) {
                    Text(plugin.displayName).font(.headline)
                    Text(status).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(12)

            if plugin.pluginId == GlobalDharmaCommerceModel.miniAppId {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Image(systemName: model.globalDharmaCommerce.accessAllowed ? "checkmark.seal.fill" : "lock.fill")
                            .foregroundStyle(model.globalDharmaCommerce.accessAllowed ? .green : .secondary)
                        Text(model.globalDharmaCommerce.message)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Spacer()
                    }
                    .accessibilityIdentifier("global-dharma-entitlement-status")

                    HStack(spacing: 10) {
                        if model.globalDharmaCommerce.accessAllowed {
                            Label("本地转经轮已买断", systemImage: "infinity")
                                .font(.subheadline.weight(.semibold))
                                .accessibilityIdentifier("global-dharma-entitlement-allowed")
                        } else {
                            Button("\(model.globalDharmaCommerce.lifetimePriceLabel) 买断本地转经轮") {
                                Task { await model.globalDharmaCommerce.purchaseLifetime() }
                            }
                            .buttonStyle(.borderedProminent)
                            .disabled(!model.globalDharmaCommerce.canBuyLifetime)
                            .accessibilityIdentifier("global-dharma-buy-lifetime")
                        }

                        Button("恢复购买") {
                            Task { await model.globalDharmaCommerce.restoreLifetime() }
                        }
                        .buttonStyle(.bordered)
                        .disabled(model.globalDharmaCommerce.busy)
                        .accessibilityIdentifier("global-dharma-restore-purchase")
                    }
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 10)
            }

            if sourceResolved, let webMcpPlugin {
                MiniAppWebView(
                    plugin: webMcpPlugin,
                    model: model,
                    localHtml: localHtml,
                    sourceResolved: true,
                    status: $status
                )
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .accessibilityIdentifier("miniapp-webmcp-resolving")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("miniapp-webmcp-surface")
        .task(id: plugin.pluginId) {
            if plugin.pluginId == GlobalDharmaCommerceModel.miniAppId {
                await model.globalDharmaCommerce.refresh()
            }
            let resolvedPlugin = await model.webMcpPlugin(for: plugin)
            webMcpPlugin = resolvedPlugin
            if let localHtmlOverride {
                localHtml = hardenGeneratedMiniAppDocument(localHtmlOverride)
            } else {
                localHtml = await model.loadLocalMiniAppHtml(plugin: resolvedPlugin)
            }
            sourceResolved = true
            status = localHtml == nil ? "正在加载 Hosted WebMCP…" : "正在加载本地 WebMCP…"
        }
    }
}

private struct MiniAppWebView: UIViewRepresentable {
    let plugin: MarketplacePlugin
    let model: MarketplaceModel
    let localHtml: String?
    let sourceResolved: Bool
    @Binding var status: String

    func makeCoordinator() -> Coordinator {
        Coordinator(plugin: plugin, model: model, status: $status)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.userContentController.add(context.coordinator, name: webMcpMessageHandler)

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.accessibilityIdentifier = "miniapp-webmcp-webview"
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = false
        webView.isInspectable = false
        context.coordinator.webView = webView
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        guard sourceResolved else { return }
        if let localHtml {
            let key = "local:\(plugin.pluginId)"
            guard context.coordinator.loadedSourceKey != key else { return }
            context.coordinator.loadedSourceKey = key
            let bridgeSession = context.coordinator.prepareLocalBridgeSession()
            let baseURL = URL(string: "https://\(localWebMcpOriginHost)/miniapps/\(plugin.pluginId)/")
            webView.loadHTMLString(
                injectLocalWebMcp(localHtml, plugin: plugin, bridgeSession: bridgeSession),
                baseURL: baseURL
            )
            return
        }

        let key = "hosted:\(plugin.pluginId)"
        guard context.coordinator.loadedSourceKey != key else { return }
        context.coordinator.loadedSourceKey = key
        _ = context.coordinator.prepareLocalBridgeSession()
        var components = URLComponents()
        components.scheme = "https"
        components.host = webMcpOriginHost
        components.path = "/miniapps/\(plugin.pluginId)/"
        if let url = components.url {
            webView.load(URLRequest(url: url))
        }
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.stopLoading()
        webView.navigationDelegate = nil
        webView.configuration.userContentController.removeScriptMessageHandler(forName: webMcpMessageHandler)
        coordinator.disposeLocalBridgeSession()
        webView.loadHTMLString("", baseURL: nil)
        coordinator.webView = nil
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        let plugin: MarketplacePlugin
        let model: MarketplaceModel
        @Binding private var status: String
        weak var webView: WKWebView?
        var loadedSourceKey: String?
        private let toolByName: [String: MiniAppToolContract]
        private var activeBridgeSession: MiniAppWebMcpBridgeSession?
        private var pendingRequests: [String: Task<Void, Never>] = [:]

        init(plugin: MarketplacePlugin, model: MarketplaceModel, status: Binding<String>) {
            self.plugin = plugin
            self.model = model
            self.toolByName = Dictionary(uniqueKeysWithValues: plugin.tools.map { ($0.name, $0) })
            _status = status
        }

        func prepareLocalBridgeSession() -> MiniAppWebMcpBridgeSession {
            disposeLocalBridgeSession()
            let session = MiniAppWebMcpBridgeSession.fresh(plugin: plugin)
            activeBridgeSession = session
            return session
        }

        func disposeLocalBridgeSession() {
            for task in pendingRequests.values {
                task.cancel()
            }
            pendingRequests.removeAll()
            activeBridgeSession = nil
        }

        func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation?) {
            status = "正在加载 WebMCP…"
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation?) {
            let host = webView.url?.host
            guard isTrustedWebMcpBridgeHost(host) else {
                disposeLocalBridgeSession()
                status = "WebMCP 页面已打开"
                return
            }

            if host == webMcpOriginHost {
                guard let session = activeBridgeSession else {
                    status = "WebMCP 页面已打开"
                    return
                }
                let bootstrap = webMcpBootstrapJavaScript(plugin: plugin, bridgeSession: session)
                webView.evaluateJavaScript(bootstrap) { [weak self, weak webView] _, error in
                    guard let self, let webView else { return }
                    guard error == nil,
                          self.activeBridgeSession == session,
                          webView.url?.host == webMcpOriginHost
                    else {
                        self.status = "WebMCP 页面已打开"
                        return
                    }
                    self.probeBridge(in: webView, local: false)
                }
                return
            }

            probeBridge(in: webView, local: true)
        }

        private func probeBridge(in webView: WKWebView, local: Bool) {
            let probe = """
            (() => {
              const tools = window.__fabushiWebMcp?.list?.() || [];
              return JSON.stringify({ ready: tools.length > 0, tools: tools.map((tool) => tool.name) });
            })()
            """
            webView.evaluateJavaScript(probe) { [weak self, weak webView] value, _ in
                guard let self else { return }
                let result = value as? String ?? ""
                guard result.contains("\"ready\":true") else {
                    self.status = "WebMCP 页面已打开"
                    return
                }
                self.status = local ? "本地 WebMCP 已连接" : "WebMCP 已连接"
                if self.plugin.pluginId == GlobalDharmaCommerceModel.miniAppId {
                    let sharedRuntimeProbe = """
                    (() => {
                      const tools=window.__fabushiWebMcp?.list?.()||[];
                      function marker(label,text,revision){
                        let node=document.getElementById('fabushi-shared-runtime-sync');
                        if(!node){
                          node=document.createElement('div');
                          node.id='fabushi-shared-runtime-sync';
                          node.setAttribute('role','status');
                          node.style.cssText='margin:12px;padding:10px 12px;border:1px solid rgba(0,0,0,.12);border-radius:12px;font:600 13px -apple-system,BlinkMacSystemFont,sans-serif;';
                          document.body.prepend(node);
                        }
                        node.setAttribute('aria-label',label);
                        node.dataset.revision=revision===undefined?'':String(revision);
                        node.textContent=text;
                        return node;
                      }
                      if(!tools.some((tool)=>tool&&tool.name==='status')){
                        marker('共享状态恢复失败','共享状态恢复失败 · WebMCP status 未暴露');
                        return;
                      }
                      window.__fabushiWebMcp.call('status',{}).then((result)=>{
                        const canonicalRuntime=result?.structuredContent?.runtime;
                        const revision=Number(canonicalRuntime?.revision??-1);
                        if(canonicalRuntime?.protocol!=='fabushi.miniapp.runtime.v1'||canonicalRuntime?.miniAppId!=='global-dharma'||!Number.isInteger(revision)||revision<0){
                          marker('共享状态恢复失败','共享状态恢复失败 · canonical runtime 无效');
                          return;
                        }
                        const label=`Bot / Web UI 同一共享状态 · revision ${revision}`;
                        marker(label,label,revision);
                        window.dispatchEvent(new CustomEvent('fabushi:shared-runtime-restored',{detail:canonicalRuntime}));
                      }).catch((error)=>{
                        marker('共享状态恢复失败',`共享状态恢复失败 · ${String(error?.message||error)}`);
                      });
                    })()
                    """
                    webView?.evaluateJavaScript(sharedRuntimeProbe)
                }
            }
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void
        ) {
            guard let url = navigationAction.request.url,
                  url.scheme == "https",
                  [webMcpOriginHost, localWebMcpOriginHost].contains(url.host ?? "")
            else {
                decisionHandler(.cancel)
                return
            }
            decisionHandler(.allow)
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.name == webMcpMessageHandler,
                  let webView,
                  isTrustedWebMcpBridgeHost(webView.url?.host),
                  let body = message.body as? [String: Any],
                  let pluginInstanceId = body["pluginInstanceId"] as? String,
                  let nonce = body["nonce"] as? String,
                  let session = activeBridgeSession,
                  session.pluginInstanceId == pluginInstanceId,
                  session.nonce == nonce
            else { return }

            if body["kind"] as? String == "dispose" {
                disposeLocalBridgeSession()
                return
            }

            guard body["kind"] as? String == "call",
                  let requestId = body["requestId"] as? String,
                  !requestId.isEmpty,
                  pendingRequests[requestId] == nil,
                  let name = body["name"] as? String,
                  session.allows(pluginInstanceId: pluginInstanceId, nonce: nonce, toolName: name),
                  let tool = toolByName[name],
                  let input = body["input"] as? [String: Any]
            else { return }

            let task = Task { @MainActor [weak self, weak webView] in
                guard let self, let webView else { return }
                defer { self.pendingRequests.removeValue(forKey: requestId) }
                do {
                    try Task.checkCancellation()
                    guard self.activeBridgeSession == session else { throw CancellationError() }
                    if tool.approval != "none" {
                        guard await self.requestApproval(tool) else {
                            self.resolve(
                                webView: webView,
                                session: session,
                                requestId: requestId,
                                payload: ["ok": false, "error": "用户取消了 WebMCP Tool 调用"]
                            )
                            return
                        }
                    }
                    try Task.checkCancellation()
                    guard self.activeBridgeSession == session else { throw CancellationError() }

                    let result: Any
                    if self.plugin.pluginId == GlobalDharmaCommerceModel.miniAppId && name == "status" {
                        let runtime = try await self.model.globalDharmaCommerce.fetchCanonicalSharedRuntime()
                        result = [
                            "content": [["type": "text", "text": "已读取全球法布施状态。"]],
                            "structuredContent": ["runtime": runtime],
                        ] as [String: Any]
                    } else {
                        result = try await self.model.callWebMcpTool(
                            pluginId: self.plugin.pluginId,
                            name: name,
                            arguments: input
                        )
                    }

                    try Task.checkCancellation()
                    guard self.activeBridgeSession == session else { throw CancellationError() }
                    self.resolve(
                        webView: webView,
                        session: session,
                        requestId: requestId,
                        payload: ["ok": true, "result": result]
                    )
                } catch is CancellationError {
                    self.resolve(
                        webView: webView,
                        session: session,
                        requestId: requestId,
                        payload: ["ok": false, "error": "MCP App bridge disposed"]
                    )
                } catch {
                    self.resolve(
                        webView: webView,
                        session: session,
                        requestId: requestId,
                        payload: ["ok": false, "error": error.localizedDescription]
                    )
                }
            }
            pendingRequests[requestId] = task
        }

        private func requestApproval(_ tool: MiniAppToolContract) async -> Bool {
            guard let presenter = topViewController() else { return false }
            let warning = tool.approval == "destructive"
                ? "该操作可能产生破坏性修改。"
                : "该操作会修改小程序或后台状态。"
            return await withCheckedContinuation { continuation in
                let alert = UIAlertController(
                    title: "允许 WebMCP 调用 \(tool.name)？",
                    message: "\(tool.description)\n\n\(warning)",
                    preferredStyle: .alert
                )
                alert.addAction(UIAlertAction(title: "取消", style: .cancel) { _ in
                    continuation.resume(returning: false)
                })
                alert.addAction(UIAlertAction(title: "允许", style: .default) { _ in
                    continuation.resume(returning: true)
                })
                presenter.present(alert, animated: true)
            }
        }

        private func topViewController() -> UIViewController? {
            let scene = UIApplication.shared.connectedScenes
                .compactMap { $0 as? UIWindowScene }
                .first { $0.activationState == .foregroundActive }
            var controller = scene?.windows.first(where: \.isKeyWindow)?.rootViewController
            while let presented = controller?.presentedViewController {
                controller = presented
            }
            return controller
        }

        private func resolve(
            webView: WKWebView,
            session: MiniAppWebMcpBridgeSession,
            requestId: String,
            payload: [String: Any]
        ) {
            guard activeBridgeSession == session,
                  JSONSerialization.isValidJSONObject(payload),
                  let data = try? JSONSerialization.data(withJSONObject: payload),
                  let json = String(data: data, encoding: .utf8),
                  let requestData = try? JSONEncoder().encode(requestId),
                  let requestJson = String(data: requestData, encoding: .utf8)
            else { return }
            let instanceJson = jsonString(session.pluginInstanceId)
            let nonceJson = jsonString(session.nonce)
            webView.evaluateJavaScript(
                "window.__fabushiNativeResolve?.(\(requestJson),\(instanceJson),\(nonceJson),\(json));"
            )
        }
    }
}

private func hardenGeneratedMiniAppDocument(_ html: String) -> String {
    let policy = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'none'; media-src data:; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'\">"
    if let range = html.range(of: "<head>", options: .caseInsensitive) {
        var result = html
        result.insert(contentsOf: policy, at: range.upperBound)
        return result
    }
    if let range = html.range(of: "<html>", options: .caseInsensitive) {
        var result = html
        result.insert(contentsOf: "<head>\(policy)</head>", at: range.upperBound)
        return result
    }
    return "<!doctype html><html><head>\(policy)</head><body>\(html)</body></html>"
}

private func webMcpBootstrapJavaScript(
    plugin: MarketplacePlugin,
    bridgeSession: MiniAppWebMcpBridgeSession
) -> String {
    let definitions = plugin.tools.map { tool in
        [
            "name": tool.name,
            "description": tool.description,
            "readOnlyHint": tool.approval == "none",
        ] as [String: Any]
    }
    let data = (try? JSONSerialization.data(withJSONObject: definitions)) ?? Data("[]".utf8)
    let toolsJson = String(data: data, encoding: .utf8) ?? "[]"
    return """
    (function(){
      const definitions=\(toolsJson);
      const pluginInstanceId=\(jsonString(bridgeSession.pluginInstanceId));
      const nonce=\(jsonString(bridgeSession.nonce));
      const grants=new Set(\(jsonStringArray(Array(bridgeSession.grants).sorted())));
      const localTools=new Map();const controllers=[];const pending=new Map();let sequence=0;let disposed=false;
      function rejectPending(reason){for(const task of pending.values())task.reject(new Error(reason));pending.clear();}
      window.__fabushiNativeResolve=(requestId,responseInstanceId,responseNonce,payload)=>{if(disposed||responseInstanceId!==pluginInstanceId||responseNonce!==nonce)return;const task=pending.get(requestId);if(!task)return;pending.delete(requestId);if(payload&&payload.ok)task.resolve(payload.result);else task.reject(new Error(payload?.error||'WebMCP runtime call failed'));};
      function callNative(name,input){if(disposed)return Promise.reject(new Error('MCP App bridge disposed'));if(!grants.has(name))return Promise.reject(new Error('MCP App bridge capability not granted: '+name));return new Promise((resolve,reject)=>{const requestId='webmcp-'+Date.now()+'-'+(++sequence);pending.set(requestId,{resolve,reject});window.webkit.messageHandlers.\(webMcpMessageHandler).postMessage({kind:'call',pluginInstanceId,nonce,requestId,name,input:input||{}});});}
      function publicTool(tool){const copy={...tool};delete copy.execute;return copy;}
      function register(item){if(!grants.has(item.name))return;const tool={name:item.name,description:item.description||item.name,inputSchema:{type:'object',properties:{}},annotations:{readOnlyHint:item.readOnlyHint===true},execute:(input)=>callNative(item.name,input)};localTools.set(tool.name,tool);if(document.modelContext&&typeof document.modelContext.registerTool==='function'){const controller=new AbortController();controllers.push(controller);Promise.resolve(document.modelContext.registerTool(tool,{signal:controller.signal})).catch(()=>{});}}
      for(const item of definitions)register(item);
      Object.defineProperty(window,'__fabushiWebMcp',{configurable:true,value:{version:1,list:()=>Array.from(localTools.values()).map(publicTool),call:async(name,input={})=>{const tool=localTools.get(name);if(!tool)throw new Error('Unknown WebMCP tool: '+name);return tool.execute(input);}}});
      window.addEventListener('pagehide',()=>{if(disposed)return;disposed=true;for(const controller of controllers)controller.abort();window.webkit.messageHandlers.\(webMcpMessageHandler).postMessage({kind:'dispose',pluginInstanceId,nonce});rejectPending('MCP App bridge disposed');},{once:true});
      window.dispatchEvent(new CustomEvent('fabushi:webmcp-ready',{detail:{pluginId:\(jsonString(plugin.pluginId)),pluginInstanceId,grants:Array.from(grants),tools:Array.from(localTools.keys())}}));
    })();
    """
}

private func injectLocalWebMcp(
    _ html: String,
    plugin: MarketplacePlugin,
    bridgeSession: MiniAppWebMcpBridgeSession
) -> String {
    let bootstrap = "<script>\(webMcpBootstrapJavaScript(plugin: plugin, bridgeSession: bridgeSession))</script>"
    if let range = html.range(of: "</head>", options: .caseInsensitive) {
        var result = html
        result.insert(contentsOf: bootstrap, at: range.lowerBound)
        return result
    }
    return bootstrap + html
}

private func jsonString(_ value: String) -> String {
    guard let data = try? JSONEncoder().encode(value) else { return "\"\"" }
    return String(data: data, encoding: .utf8) ?? "\"\""
}

private func jsonStringArray(_ values: [String]) -> String {
    guard let data = try? JSONEncoder().encode(values) else { return "[]" }
    return String(data: data, encoding: .utf8) ?? "[]"
}

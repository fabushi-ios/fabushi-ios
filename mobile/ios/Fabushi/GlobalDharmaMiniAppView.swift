import SwiftUI
import UIKit
import WebKit

struct GlobalDharmaMiniAppView: View {
    let model: MarketplaceModel
    let host: MahayanaHost

    @Environment(\.dismiss) private var dismiss
    @State private var bridge: GlobalDharmaMiniAppBridge

    init(model: MarketplaceModel, host: MahayanaHost) {
        self.model = model
        self.host = host
        _bridge = State(initialValue: GlobalDharmaMiniAppBridge(host: host))
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Button("返回") { dismiss() }
                    .accessibilityIdentifier("global-dharma-miniapp-close")
                VStack(alignment: .leading, spacing: 2) {
                    Text("全球法布施").font(.headline)
                    Text("Telegram 式 Web UI · WebMCP")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(12)
            .background(.ultraThinMaterial)

            GlobalDharmaWebView(
                bridge: bridge,
                accountName: model.accountName,
                accountEmail: model.accountEmail,
                loggedIn: model.loggedIn
            )
        }
        .background(Color(.systemGroupedBackground))
        .accessibilityIdentifier("global-dharma-miniapp-surface")
    }
}

private struct GlobalDharmaWebView: UIViewRepresentable {
    let bridge: GlobalDharmaMiniAppBridge
    let accountName: String
    let accountEmail: String
    let loggedIn: Bool

    func makeCoordinator() -> Coordinator { Coordinator(bridge: bridge) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.userContentController.add(context.coordinator, name: "fabushiGlobalDharma")
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = false
        webView.isInspectable = false
        webView.accessibilityIdentifier = "global-dharma-miniapp-webview"
        context.coordinator.webView = webView
        webView.loadHTMLString(
            Self.html(
                accountName: accountName,
                accountEmail: accountEmail,
                loggedIn: loggedIn,
                testCommerceEnabled: bridge.testCommerceEnabled
            ),
            baseURL: URL(string: "https://miniapp.local.fabushi.invalid/global-dharma/")
        )
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.accountName = accountName
        context.coordinator.accountEmail = accountEmail
        context.coordinator.loggedIn = loggedIn
        let account = Self.json([
            "loggedIn": loggedIn,
            "name": accountName,
            "email": accountEmail,
        ])
        webView.evaluateJavaScript("window.__fabushiUpdateAccount?.(\(account));")
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.stopLoading()
        webView.navigationDelegate = nil
        webView.configuration.userContentController.removeScriptMessageHandler(forName: "fabushiGlobalDharma")
        coordinator.webView = nil
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        let bridge: GlobalDharmaMiniAppBridge
        weak var webView: WKWebView?
        var accountName = "Fabushi"
        var accountEmail = ""
        var loggedIn = false
        private var pendingPurchaseKey: String?

        init(bridge: GlobalDharmaMiniAppBridge) {
            self.bridge = bridge
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void
        ) {
            guard let url = navigationAction.request.url,
                  url.scheme == "https",
                  url.host == "miniapp.local.fabushi.invalid"
            else {
                decisionHandler(.cancel)
                return
            }
            decisionHandler(.allow)
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.name == "fabushiGlobalDharma",
                  let webView,
                  let body = message.body as? [String: Any],
                  let requestId = body["requestId"] as? String,
                  let action = body["action"] as? String
            else { return }

            Task { @MainActor in
                do {
                    switch action {
                    case "tool":
                        guard let name = body["name"] as? String else {
                            throw MahayanaHost.HostError.requestFailed("Missing WebMCP tool name")
                        }
                        let input = body["input"] as? [String: Any] ?? [:]
                        if Self.requiresApproval(name), !(await requestApproval(name: name)) {
                            resolve(webView: webView, requestId: requestId, payload: ["ok": false, "error": "用户取消了 WebMCP Tool 调用"])
                            return
                        }
                        let result = try await bridge.callOfficialMcpTool(
                            pluginId: GlobalDharmaMiniAppBridge.globalDharmaId,
                            name: name,
                            arguments: input
                        )
                        resolve(webView: webView, requestId: requestId, payload: ["ok": true, "result": result])
                    case "entitlement":
                        let response = try await bridge.entitlement()
                        let state = try bridge.validateLifetimeCatalog(response)
                        resolve(webView: webView, requestId: requestId, payload: [
                            "ok": true,
                            "result": [
                                "allowed": state.allowed,
                                "reason": state.reason,
                                "activeRails": state.activeRails,
                                "testMode": bridge.testCommerceEnabled,
                            ],
                        ])
                    case "purchaseLifetime":
                        guard bridge.testCommerceEnabled else {
                            throw MahayanaHost.HostError.requestFailed("生产支付 rail 必须走 provider checkout；当前不会绕过真实支付 provider")
                        }
                        let key = pendingPurchaseKey ?? "ios-global-dharma-lifetime-\(UUID().uuidString.lowercased())"
                        pendingPurchaseKey = key
                        _ = try await bridge.purchaseLifetimeTest(idempotencyKey: key)
                        let response = try await bridge.entitlement()
                        let state = try bridge.validateLifetimeCatalog(response)
                        if state.allowed { pendingPurchaseKey = nil }
                        resolve(webView: webView, requestId: requestId, payload: [
                            "ok": true,
                            "result": ["allowed": state.allowed, "reason": state.reason, "message": state.allowed ? "¥1080 买断权益已由服务端确认" : "订单已处理，但权益仍未生效"],
                        ])
                    case "restorePurchases":
                        _ = try await bridge.restorePurchases()
                        let response = try await bridge.entitlement()
                        let state = try bridge.validateLifetimeCatalog(response)
                        resolve(webView: webView, requestId: requestId, payload: [
                            "ok": true,
                            "result": ["allowed": state.allowed, "reason": state.reason, "message": state.allowed ? "权益恢复完成" : "恢复完成，但没有有效的本地转经轮权益"],
                        ])
                    default:
                        throw MahayanaHost.HostError.requestFailed("Unsupported Global Dharma host action")
                    }
                } catch {
                    resolve(webView: webView, requestId: requestId, payload: ["ok": false, "error": error.localizedDescription])
                }
            }
        }

        private static func requiresApproval(_ name: String) -> Bool {
            !["home", "status", "logs", "validate_config"].contains(name)
        }

        private func requestApproval(name: String) async -> Bool {
            guard let presenter = topViewController() else { return false }
            let destructive = name == "stop"
            return await withCheckedContinuation { continuation in
                let alert = UIAlertController(
                    title: "允许 WebMCP 调用 \(name)？",
                    message: destructive ? "该操作可能停止正在运行的服务。" : "该操作会修改全球法布施运行状态或访问外部能力。",
                    preferredStyle: .alert
                )
                alert.addAction(UIAlertAction(title: "取消", style: .cancel) { _ in continuation.resume(returning: false) })
                alert.addAction(UIAlertAction(title: "允许", style: destructive ? .destructive : .default) { _ in continuation.resume(returning: true) })
                presenter.present(alert, animated: true)
            }
        }

        private func topViewController() -> UIViewController? {
            let scene = UIApplication.shared.connectedScenes
                .compactMap { $0 as? UIWindowScene }
                .first { $0.activationState == .foregroundActive }
            var controller = scene?.windows.first(where: \.isKeyWindow)?.rootViewController
            while let presented = controller?.presentedViewController { controller = presented }
            return controller
        }

        private func resolve(webView: WKWebView, requestId: String, payload: [String: Any]) {
            let request = GlobalDharmaWebView.json(requestId)
            let payloadJSON = GlobalDharmaWebView.json(payload)
            webView.evaluateJavaScript("window.__fabushiNativeResolve?.(\(request),\(payloadJSON));")
        }
    }

    private static func html(accountName: String, accountEmail: String, loggedIn: Bool, testCommerceEnabled: Bool) -> String {
        let accountJSON = json(["loggedIn": loggedIn, "name": accountName, "email": accountEmail])
        let testMode = testCommerceEnabled ? "true" : "false"
        return """
        <!doctype html>
        <html lang="zh-CN">
        <head>
          <meta charset="utf-8">
          <meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
          <title>全球法布施</title>
          <style>
            :root{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;color-scheme:light dark;background:#f5f6f8;color:#15171a}*{box-sizing:border-box}
            body{margin:0;min-height:100vh;background:linear-gradient(180deg,#edf2f8,#f8f9fb 42%,#fff)}main{max-width:760px;margin:auto;padding:16px 14px 140px}
            .hero,.panel{background:rgba(255,255,255,.94);border-radius:22px;padding:17px;box-shadow:0 12px 38px rgba(30,45,70,.09)}h1{font-size:25px;margin:5px 0}.eyebrow{font-size:11px;font-weight:800;letter-spacing:.13em;color:#647ba5}.muted{color:#6c7686;font-size:13px;line-height:1.5}.account{margin-top:12px;font-size:13px;font-weight:650}.ok{color:#168357}.bad{color:#bd3e49}
            section{margin-top:14px}.title{font-size:12px;font-weight:800;color:#738093;margin:0 0 8px 3px}.tools{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:9px}.tool{border:0;border-radius:17px;background:#fff;padding:13px;text-align:left;box-shadow:0 7px 22px rgba(30,45,70,.07);min-height:68px}.tool b{display:block;font-size:14px}.tool small{color:#7a8596}.tool:disabled{opacity:.5}
            #runtimeStatus{font-weight:750;font-size:13px}#output{white-space:pre-wrap;word-break:break-word;font:12px/1.55 ui-monospace,SFMono-Regular,Menlo,monospace;max-height:230px;overflow:auto;color:#536174}.commerce-row{display:flex;gap:8px;flex-wrap:wrap;margin-top:10px}button.action{border:0;border-radius:14px;padding:11px 14px;font-weight:750}.primary{background:#111;color:#fff}.secondary{background:#e9edf3;color:#1c2532}
            @media(prefers-color-scheme:dark){:root{background:#0d1014;color:#eef2f7}body{background:linear-gradient(180deg,#121822,#0e1116)}.hero,.panel,.tool{background:#1a1f27;color:#eef2f7;box-shadow:none}.muted,.title,.tool small,#output{color:#9da8b8}.secondary{background:#252b35;color:#eef2f7}}
          </style>
        </head>
        <body><main>
          <div class="hero"><div class="eyebrow">FABUSHI MINI APP · IOS</div><h1>全球法布施</h1><div class="muted">Bot 与此 Web UI 通过同一官方 WebMCP、同一 Fabushi 账号作用域和同一服务端运行状态工作。</div><div id="account" class="account"></div></div>
          <section><div class="title">WEBMCP TOOLS</div><div id="tools" class="tools"></div></section>
          <section><div class="title">BOT / WEB UI 共享状态</div><div class="panel"><div id="runtimeStatus">正在恢复共享状态…</div><pre id="output"></pre></div></section>
          <section><div class="title">本地转经轮 · CNY 1080 买断</div><div class="panel"><div id="entitlement" class="muted">正在读取 canonical entitlement…</div><div class="commerce-row"><button id="buy" class="action primary">¥1080 买断</button><button id="restore" class="action secondary">恢复购买</button></div></div></section>
          <script>
          (()=>{
            const testMode=\(testMode);let account=\(accountJSON);let seq=0;const pending=new Map();
            const definitions=[
              ['home','加载首页',true],['status','读取运行状态',true],['logs','读取最近日志',true],['validate_config','验证配置',true],
              ['start','启动服务',false],['stop','停止服务',false],['loop','执行一次循环',false],['chat','处理对话',false],['send','发送内容',false],['deploy_latest','部署最新版本',false]
            ].map(([name,description,readOnlyHint])=>({name,description,inputSchema:{type:'object',properties:{}},annotations:{readOnlyHint}}));
            function post(action,extra={}){return new Promise((resolve,reject)=>{const requestId='ios-global-dharma-'+Date.now()+'-'+(++seq);pending.set(requestId,{resolve,reject});window.webkit.messageHandlers.fabushiGlobalDharma.postMessage({requestId,action,...extra});});}
            window.__fabushiNativeResolve=(requestId,payload)=>{const task=pending.get(requestId);if(!task)return;pending.delete(requestId);payload&&payload.ok?task.resolve(payload.result):task.reject(new Error(payload?.error||'Native action failed'));};
            const tools=new Map(definitions.map(tool=>[tool.name,{...tool,execute:(input={})=>post('tool',{name:tool.name,input})}]));
            window.__fabushiWebMcp={version:1,list:()=>Array.from(tools.values()).map(({execute,...rest})=>rest),call:(name,input={})=>{const tool=tools.get(name);if(!tool)throw new Error('Unknown WebMCP tool: '+name);return tool.execute(input);}};
            if(document.modelContext&&typeof document.modelContext.registerTool==='function'){for(const tool of tools.values()){const controller=new AbortController();Promise.resolve(document.modelContext.registerTool(tool,{signal:controller.signal})).catch(()=>{});}}
            function renderAccount(){const node=document.getElementById('account');node.textContent=account.loggedIn?'Fabushi 已登录 · '+(account.name||account.email||'当前账号'):'Fabushi 未登录';node.className='account '+(account.loggedIn?'ok':'bad');}
            window.__fabushiUpdateAccount=next=>{account=next||account;renderAccount();};
            const toolsNode=document.getElementById('tools');for(const def of definitions){const button=document.createElement('button');button.className='tool';button.innerHTML='<b></b><small></small>';button.querySelector('b').textContent=def.description;button.querySelector('small').textContent=def.name;button.onclick=async()=>{button.disabled=true;document.getElementById('runtimeStatus').textContent=def.name+' · running';try{const result=await window.__fabushiWebMcp.call(def.name,{});document.getElementById('output').textContent=JSON.stringify(result,null,2);document.getElementById('runtimeStatus').textContent=def.name+' · completed · Bot / Web UI 同一共享状态';}catch(error){document.getElementById('runtimeStatus').textContent=def.name+' · failed';document.getElementById('output').textContent=String(error?.message||error);}finally{button.disabled=false;}};toolsNode.appendChild(button);}
            async function restoreSharedRuntime(){try{const result=await window.__fabushiWebMcp.call('status',{});document.getElementById('output').textContent=JSON.stringify(result,null,2);document.getElementById('runtimeStatus').textContent='status · synced · Bot / Web UI 同一共享状态';window.dispatchEvent(new CustomEvent('fabushi:shared-runtime-restored',{detail:result}));}catch(error){document.getElementById('runtimeStatus').textContent='status · failed · 共享状态恢复失败';document.getElementById('output').textContent=String(error?.message||error);}}
            async function refreshEntitlement(){try{const state=await post('entitlement');document.getElementById('entitlement').textContent=(state.allowed?'本地转经轮：已解锁':'本地转经轮：未解锁')+' · '+state.reason+(state.activeRails?.length?' · '+state.activeRails.join(', '):'');const buy=document.getElementById('buy');buy.textContent=testMode?'¥1080 买断（测试）':'¥1080 买断';buy.disabled=state.allowed||!testMode;if(!testMode&&!state.allowed)document.getElementById('entitlement').textContent+=' · 生产 provider 未在此界面绕过，保持 fail-closed';}catch(error){document.getElementById('entitlement').textContent='权益读取失败 · '+String(error?.message||error);}}
            document.getElementById('buy').onclick=async()=>{const button=document.getElementById('buy');button.disabled=true;try{const result=await post('purchaseLifetime');document.getElementById('entitlement').textContent=result.message;await refreshEntitlement();}catch(error){document.getElementById('entitlement').textContent='购买失败 · '+String(error?.message||error);}finally{if(testMode)button.disabled=false;}};
            document.getElementById('restore').onclick=async()=>{const button=document.getElementById('restore');button.disabled=true;try{const result=await post('restorePurchases');document.getElementById('entitlement').textContent=result.message;await refreshEntitlement();}catch(error){document.getElementById('entitlement').textContent='恢复失败 · '+String(error?.message||error);}finally{button.disabled=false;}};
            renderAccount();window.dispatchEvent(new CustomEvent('fabushi:webmcp-ready',{detail:{pluginId:'global-dharma',tools:definitions.map(x=>x.name)}}));restoreSharedRuntime();refreshEntitlement();
          })();
          </script>
        </main></body></html>
        """
    }

    fileprivate static func json(_ value: Any) -> String {
        guard JSONSerialization.isValidJSONObject(value) || value is String else { return "null" }
        if let string = value as? String,
           let data = try? JSONEncoder().encode(string) {
            return String(data: data, encoding: .utf8) ?? "\"\""
        }
        guard let data = try? JSONSerialization.data(withJSONObject: value),
              let text = String(data: data, encoding: .utf8)
        else { return "null" }
        return text
    }
}

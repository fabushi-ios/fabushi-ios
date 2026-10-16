import Foundation
import Observation
import StoreKit

struct GlobalDharmaLifetimeOffer: Equatable, Sendable {
    let productId: String
    let sku: String
    let productKind: String
    let currency: String
    let amount: Int64
    let activeRails: [String]

    var appleStoreAvailable: Bool {
        activeRails.contains("apple_in_app_purchase")
    }

    var matchesCanonicalLifetime: Bool {
        productId == GlobalDharmaCommerceModel.lifetimeProductId &&
            sku == GlobalDharmaCommerceModel.lifetimeSku &&
            productKind == "digital_durable" &&
            currency == "CNY" &&
            amount == 108_000
    }
}

enum GlobalDharmaCommerceError: LocalizedError {
    case invalidResponse
    case unauthenticated
    case lifetimeOfferMissing
    case lifetimeOfferMismatch
    case appleStoreNotConfigured
    case entitlementNotGranted
    case serverRejected(Int, String)

    var errorDescription: String? {
        switch self {
        case .invalidResponse: return "支付服务返回了无效响应"
        case .unauthenticated: return "Fabushi 登录态不可用"
        case .lifetimeOfferMissing: return "服务端未返回本地转经轮买断商品"
        case .lifetimeOfferMismatch: return "服务端买断商品价格或权限契约与客户端安全基线不一致"
        case .appleStoreNotConfigured: return "App Store 买断商品尚未激活，已按安全策略停止购买"
        case .entitlementNotGranted: return "支付已处理，但服务端尚未授予本地转经轮权限"
        case .serverRejected(let status, let message): return "服务端拒绝请求（\(status)）：\(message)"
        }
    }
}

@MainActor
@Observable
final class GlobalDharmaCommerceModel {
    nonisolated static let miniAppId = "global-dharma"
    nonisolated static let capability = "local.prayer-wheel.start"
    nonisolated static let lifetimeProductId = "prod.global-dharma.local-prayer-wheel.lifetime"
    nonisolated static let lifetimeSku = "local-prayer-wheel.lifetime"

    var accessAllowed = false
    var accessReason = "not_loaded"
    var lifetimeOffer: GlobalDharmaLifetimeOffer?
    var busy = false
    var message = "正在检查本地转经轮权限…"
    var lastPaymentId: String?

    var canBuyLifetime: Bool {
        guard let lifetimeOffer else { return false }
        return !accessAllowed && lifetimeOffer.matchesCanonicalLifetime && (canonicalLedgerTestMode || lifetimeOffer.appleStoreAvailable) && !busy
    }

    var lifetimePriceLabel: String {
        guard let lifetimeOffer, lifetimeOffer.matchesCanonicalLifetime else { return "¥1080" }
        return "¥\(lifetimeOffer.amount / 100)"
    }

    private let host: MahayanaHost
    private let platformBaseURL: URL
    private let paymentBaseURL: URL
    private let session: URLSession
    private let canonicalLedgerTestMode: Bool

    init(
        host: MahayanaHost,
        platformBaseURL: URL = URL(string: "https://api.ombhrum.com")!,
        paymentBaseURL: URL = URL(string: "https://pay.ombhrum.com")!,
        session: URLSession = .shared,
        canonicalLedgerTestMode: Bool? = nil
    ) {
        self.host = host
        self.platformBaseURL = platformBaseURL
        self.paymentBaseURL = paymentBaseURL
        self.session = session
        self.canonicalLedgerTestMode = canonicalLedgerTestMode ?? Self.detectCanonicalLedgerTestMode()
    }

    func refresh() async {
        do {
            try applyEntitlement(try await fetchEntitlement())
            if accessAllowed {
                message = "本地转经轮已买断 · 权限有效"
            } else if let lifetimeOffer, lifetimeOffer.matchesCanonicalLifetime {
                if canonicalLedgerTestMode {
                    message = "\(lifetimePriceLabel) 测试买断 · canonical ledger（不真实扣款）"
                } else {
                    message = lifetimeOffer.appleStoreAvailable
                        ? "本地转经轮买断 \(lifetimePriceLabel)"
                        : "\(lifetimePriceLabel) 买断 · App Store 商品尚未激活"
                }
            } else {
                message = "本地转经轮权限未开通"
            }
        } catch {
            accessAllowed = false
            accessReason = "lookup_failed"
            lifetimeOffer = nil
            message = "权限检查失败：\(error.localizedDescription)"
        }
    }

    func purchaseLifetime() async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do {
            try applyEntitlement(try await fetchEntitlement())
            if accessAllowed {
                message = "本地转经轮已买断 · 无需重复购买"
                return
            }
            guard let offer = lifetimeOffer else { throw GlobalDharmaCommerceError.lifetimeOfferMissing }
            guard offer.matchesCanonicalLifetime else { throw GlobalDharmaCommerceError.lifetimeOfferMismatch }
            if canonicalLedgerTestMode {
                try await purchaseLifetimeThroughCanonicalLedger()
                return
            }
            guard offer.appleStoreAvailable else { throw GlobalDharmaCommerceError.appleStoreNotConfigured }

            message = "正在创建 Fabushi Pay 订单…"
            let intent = try await requestJSON(
                baseURL: paymentBaseURL,
                path: "/v1/miniapps/\(Self.miniAppId)/pay/intents",
                method: "POST",
                body: [
                    "sku": Self.lifetimeSku,
                    "rail": "appleInAppPurchase",
                    "idempotencyKey": "ios-global-dharma-lifetime-\(UUID().uuidString.lowercased())",
                ]
            )
            guard let paymentId = intent["paymentId"] as? String,
                  UUID(uuidString: paymentId) != nil,
                  intent["sku"] as? String == Self.lifetimeSku,
                  (intent["amount"] as? NSNumber)?.int64Value == 108_000,
                  intent["currency"] as? String == "CNY"
            else { throw GlobalDharmaCommerceError.invalidResponse }
            lastPaymentId = paymentId

            let checkout = try await requestJSON(
                baseURL: paymentBaseURL,
                path: "/v1/pay/intents/\(paymentId)/checkout",
                method: "POST"
            )
            guard let action = checkout["checkoutAction"] as? [String: Any],
                  action["kind"] as? String == "appleInAppPurchase",
                  let genericProductId = action["productId"] as? String,
                  !genericProductId.isEmpty,
                  let verifyPath = action["verifyPath"] as? String,
                  verifyPath == "/v1/pay/intents/\(paymentId)/apple/verify",
                  let advancedCommercePath = action["advancedCommercePath"] as? String,
                  advancedCommercePath == "/v1/pay/intents/\(paymentId)/apple/advanced-commerce"
            else { throw GlobalDharmaCommerceError.invalidResponse }

            guard let storefront = await Storefront.current?.countryCode,
                  storefront.count == 3
            else { throw GlobalDharmaCommerceError.appleStoreNotConfigured }
            message = "正在签名 Apple Advanced Commerce 请求…"
            let advanced = try await requestJSON(
                baseURL: paymentBaseURL,
                path: advancedCommercePath,
                method: "POST",
                body: ["storefront": storefront.uppercased()]
            )
            guard advanced["genericProductId"] as? String == genericProductId,
                  let advancedData = advanced["advancedCommerceData"] as? [String: Any],
                  let signatureInfo = advancedData["signatureInfo"] as? [String: Any],
                  let compactJWS = signatureInfo["token"] as? String,
                  !compactJWS.isEmpty,
                  let requestReferenceId = advanced["requestReferenceId"] as? String,
                  UUID(uuidString: requestReferenceId) != nil
            else { throw GlobalDharmaCommerceError.invalidResponse }

            message = "正在打开 App Store Advanced Commerce 购买流程…"
            let storeKit = FabushiPayStoreKit(
                serviceBaseURL: paymentBaseURL,
                session: session,
                accessTokenProvider: { @MainActor [host] in
                    try await Self.currentAccessToken(from: host)
                }
            )
            let receipt = try await storeKit.purchaseAdvancedCommerce(
                paymentId: paymentId,
                genericProductId: genericProductId,
                compactJWS: compactJWS,
                verifyPath: verifyPath
            )
            guard receipt.paymentId == paymentId,
                  receipt.sku == Self.lifetimeSku,
                  receipt.amount == 108_000,
                  receipt.currency == "CNY",
                  receipt.status == "succeeded"
            else { throw GlobalDharmaCommerceError.invalidResponse }

            try applyEntitlement(try await fetchEntitlement())
            guard accessAllowed else { throw GlobalDharmaCommerceError.entitlementNotGranted }
            message = "购买完成 · 本地转经轮永久权限已生效"
        } catch {
            message = "购买未完成：\(error.localizedDescription)"
        }
    }

    func restoreLifetime() async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do {
            try applyEntitlement(try await fetchEntitlement())
            if canonicalLedgerTestMode {
                try await restoreLifetimeThroughCanonicalLedger()
                return
            }
            if accessAllowed {
                message = "永久权限已在当前 Fabushi 账号生效"
                return
            }

            message = "正在从 App Store 恢复购买…"
            let storeKit = FabushiPayStoreKit(
                serviceBaseURL: paymentBaseURL,
                session: session,
                accessTokenProvider: { @MainActor [host] in
                    try await Self.currentAccessToken(from: host)
                }
            )
            let receipt = try await storeKit.restore(expectedSku: Self.lifetimeSku)
            lastPaymentId = receipt.paymentId
            try applyEntitlement(try await fetchEntitlement())
            guard accessAllowed else { throw GlobalDharmaCommerceError.entitlementNotGranted }
            message = "恢复完成 · 本地转经轮永久权限已生效"
        } catch {
            message = "恢复未完成：\(error.localizedDescription)"
        }
    }

    func fetchCanonicalSharedRuntime() async throws -> [String: Any] {
        let runtime = try await requestJSON(
            baseURL: platformBaseURL,
            path: "/v1/miniapps/\(Self.miniAppId)/runtime",
            method: "GET"
        )
        guard runtime["protocol"] as? String == "fabushi.miniapp.runtime.v1",
              runtime["miniAppId"] as? String == Self.miniAppId,
              let revision = (runtime["revision"] as? NSNumber)?.int64Value,
              revision >= 0,
              runtime["state"] is [String: Any]
        else { throw GlobalDharmaCommerceError.invalidResponse }
        return runtime
    }

    private func purchaseLifetimeThroughCanonicalLedger() async throws {
        message = "CI 测试模式：通过 canonical ledger 购买 ¥1080 买断权益（不真实扣款）…"
        let purchase = try await requestJSON(
            baseURL: platformBaseURL,
            path: "/v1/plugins/\(Self.miniAppId)/commerce/purchase",
            method: "POST",
            body: [
                "sku": Self.lifetimeSku,
                "idempotencyKey": "ios-ci-global-dharma-lifetime-\(UUID().uuidString.lowercased())",
            ]
        )
        if let paymentId = purchase["paymentId"] as? String, UUID(uuidString: paymentId) != nil {
            lastPaymentId = paymentId
        }
        try applyEntitlement(try await fetchEntitlement())
        guard accessAllowed else { throw GlobalDharmaCommerceError.entitlementNotGranted }
        message = "测试购买完成 · canonical server entitlement 已生效 · 未发生真实扣款"
    }

    private func restoreLifetimeThroughCanonicalLedger() async throws {
        message = "CI 测试模式：从 canonical purchase ledger 恢复权益（不访问 StoreKit）…"
        _ = try await requestJSON(
            baseURL: platformBaseURL,
            path: "/v1/purchases/restore",
            method: "POST",
            body: [:]
        )
        try applyEntitlement(try await fetchEntitlement())
        guard accessAllowed else { throw GlobalDharmaCommerceError.entitlementNotGranted }
        message = "测试恢复完成 · canonical server entitlement 已确认"
    }

    private func fetchEntitlement() async throws -> [String: Any] {
        try await requestJSON(
            baseURL: platformBaseURL,
            path: "/v1/plugins/\(Self.miniAppId)/entitlements/\(Self.capability)",
            method: "GET"
        )
    }

    private func applyEntitlement(_ object: [String: Any]) throws {
        guard let access = object["access"] as? [String: Any],
              let allowed = access["allowed"] as? Bool
        else { throw GlobalDharmaCommerceError.invalidResponse }
        accessAllowed = allowed
        accessReason = access["reason"] as? String ?? "unknown"

        let options = object["purchaseOptions"] as? [[String: Any]] ?? []
        lifetimeOffer = options.compactMap(Self.parseLifetimeOffer).first {
            $0.productId == Self.lifetimeProductId || $0.sku == Self.lifetimeSku
        }
    }

    nonisolated static func detectCanonicalLedgerTestMode(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> Bool {
        guard environment["GITHUB_ACTIONS"] == "true",
              environment["GITHUB_REPOSITORY"] == "bhrumom/fabushi",
              let sessionFile = environment["FABUSHI_CI_ACCOUNT_SESSION_FILE"],
              !sessionFile.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let sha = environment["GITHUB_SHA"],
              sha.count == 40
        else { return false }
        return true
    }

    nonisolated static func parseLifetimeOffer(_ object: [String: Any]) -> GlobalDharmaLifetimeOffer? {
        guard let productId = object["productId"] as? String,
              let sku = object["sku"] as? String,
              let productKind = object["productKind"] as? String,
              let currency = object["currency"] as? String,
              let amount = (object["amount"] as? NSNumber)?.int64Value
        else { return nil }
        return GlobalDharmaLifetimeOffer(
            productId: productId,
            sku: sku,
            productKind: productKind,
            currency: currency,
            amount: amount,
            activeRails: object["activeRails"] as? [String] ?? []
        )
    }

    private func requestJSON(
        baseURL: URL,
        path: String,
        method: String,
        body: [String: Any]? = nil
    ) async throws -> [String: Any] {
        guard let url = URL(string: path, relativeTo: baseURL)?.absoluteURL else {
            throw GlobalDharmaCommerceError.invalidResponse
        }
        let token = try await Self.currentAccessToken(from: host)
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            guard JSONSerialization.isValidJSONObject(body) else { throw GlobalDharmaCommerceError.invalidResponse }
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw GlobalDharmaCommerceError.invalidResponse }
        guard (200..<300).contains(http.statusCode) else {
            let message = String(data: data, encoding: .utf8) ?? "unknown error"
            throw GlobalDharmaCommerceError.serverRejected(http.statusCode, String(message.prefix(800)))
        }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw GlobalDharmaCommerceError.invalidResponse
        }
        return object
    }

    private static func currentAccessToken(from host: MahayanaHost) async throws -> String {
        let result = try await host.request(method: "feature.auth.deviceAgentSession")
        guard let object = result.value as? [String: Any],
              let token = object["accessToken"] as? String,
              token.count >= 24,
              token.count <= 16 * 1024,
              token.rangeOfCharacter(from: .whitespacesAndNewlines) == nil
        else { throw GlobalDharmaCommerceError.unauthenticated }
        return token
    }
}

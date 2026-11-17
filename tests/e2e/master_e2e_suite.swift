//
//  master_e2e_suite.swift
//  fabushi-ios
//
//  Legacy host-side C-ABI probe harness (cleaned of @main, exit(), duplicate symbols,
//  SearchIndexingBridge mocks, and hardcoded latency constants).
//  Canonical 4-tier Simulator XCUITest E2E verification lives in mobile/ios/FabushiUITests/Parity/.
//

import Foundation

@_silgen_name("telegram_app_host_create_test")
public func telegram_app_host_create_test(_ appDataDir: UnsafePointer<CChar>?) -> UnsafeMutableRawPointer?

@_silgen_name("telegram_app_host_dispatch")
public func telegram_app_host_dispatch(_ host: UnsafeMutableRawPointer?, _ req: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?

@_silgen_name("telegram_app_host_free_string")
public func telegram_app_host_free_string(_ ptr: UnsafeMutablePointer<CChar>?)

@_silgen_name("telegram_app_host_destroy")
public func telegram_app_host_destroy(_ host: UnsafeMutableRawPointer?)

@_silgen_name("telegram_bridge_register_event_callback")
public func telegram_bridge_register_event_callback(
    _ host: UnsafeMutableRawPointer?,
    _ cb: (@convention(c) (UnsafePointer<CChar>?, UnsafePointer<CChar>?, UnsafeMutableRawPointer?) -> Void)?,
    _ ctx: UnsafeMutableRawPointer?
)

public struct TelegramHostCAbiProbeReport: Sendable {
    public let pingSucceeded: Bool
    public let unknownCommandRejectedFailClosed: Bool
    public let measuredCAbiP95Ms: Double
}

public enum TelegramHostCAbiProbe {
    public static func runSmokeProbe(iterations: Int = 32) -> TelegramHostCAbiProbeReport {
        let storageURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("telegram_cabi_probe_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: storageURL) }

        guard let host = storageURL.path.withCString({ telegram_app_host_create_test($0) }) else {
            return TelegramHostCAbiProbeReport(
                pingSucceeded: false,
                unknownCommandRejectedFailClosed: false,
                measuredCAbiP95Ms: .nan
            )
        }
        defer {
            telegram_bridge_register_event_callback(host, nil, nil)
            telegram_app_host_destroy(host)
        }

        var latenciesMs: [Double] = []
        latenciesMs.reserveCapacity(max(1, iterations))
        var pingOk = true

        for i in 0..<max(1, iterations) {
            let payload = "{\"command\":\"telegram.ping\",\"params\":{\"nonce\":\(i)}}"
            let startNs = DispatchTime.now().uptimeNanoseconds
            let responsePtr = payload.withCString { telegram_app_host_dispatch(host, $0) }
            let elapsedMs = Double(DispatchTime.now().uptimeNanoseconds - startNs) / 1_000_000.0
            latenciesMs.append(elapsedMs)

            guard let responsePtr else {
                pingOk = false
                continue
            }
            let body = String(cString: responsePtr)
            telegram_app_host_free_string(responsePtr)
            if !body.contains("\"ok\":true") {
                pingOk = false
            }
        }

        var unknownRejected = false
        let unknownReq = "{\"command\":\"nonexistent.probe_command\",\"params\":{}}"
        if let unknownPtr = unknownReq.withCString({ telegram_app_host_dispatch(host, $0) }) {
            let raw = String(cString: unknownPtr)
            telegram_app_host_free_string(unknownPtr)
            unknownRejected = raw.contains("\"ok\":false") && raw.contains("\"unknown_command\"")
        }

        let sorted = latenciesMs.sorted()
        let p95Index = min(sorted.count - 1, Int(Double(sorted.count) * 0.95))
        let p95 = sorted[p95Index]

        return TelegramHostCAbiProbeReport(
            pingSucceeded: pingOk,
            unknownCommandRejectedFailClosed: unknownRejected,
            measuredCAbiP95Ms: p95
        )
    }
}

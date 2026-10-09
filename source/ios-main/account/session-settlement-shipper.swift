import Foundation

let IOS_CURSOR_STRUCTURED_LOG_SUBMIT_PATH = "aiserver.v1.AnalyticsService/SubmitLogs"
let IOS_DESKTOP_SESSION_EVENT = "sand.desktop.session"
let IOS_SAND_LOG_KEY = "sand"

enum IOSCursorSessionSettlement: Equatable, Sendable {
    case signedOut(
        cause: IOSCursorSessionSignoutCause,
        durable: Bool,
        accessToken: String
    )
    case keychainUnavailable(accessToken: String)

    var accessToken: String {
        switch self {
        case .signedOut(_, _, let accessToken), .keychainUnavailable(let accessToken):
            return accessToken
        }
    }

    var projection: IOSAuthTelemetryProjection {
        switch self {
        case .signedOut(let cause, let durable, _):
            return iosCursorSessionTelemetry(.signedOut(cause: cause, durable: durable))
        case .keychainUnavailable:
            return iosCursorSessionTelemetry(.keychainUnavailable)
        }
    }
}

enum IOSCursorStructuredLogSubmitError: Error, LocalizedError, Equatable, Sendable {
    case invalidResponse
    case httpStatus(Int)
    case invalidReceipt(processed: Int, dropped: Int, requested: Int)

    var errorDescription: String? {
        switch self {
        case .invalidResponse:
            return "Structured-log submit returned an invalid response."
        case .httpStatus(let status):
            return "Structured-log submit failed with HTTP \(status)."
        case .invalidReceipt(let processed, let dropped, let requested):
            return "Structured-log submit returned an invalid receipt processed=\(processed) dropped=\(dropped) requested=\(requested)."
        }
    }
}

struct IOSCursorStructuredLogBackend: Sendable {
    typealias RequestExecutor = @Sendable (URLRequest) async throws -> (Data, URLResponse)

    let backendURL: URL
    let getMachineID: @Sendable () async throws -> String
    let requestExecutor: RequestExecutor

    init(
        backendURL: URL,
        getMachineID: @escaping @Sendable () async throws -> String,
        session: URLSession = .shared,
        requestExecutor: RequestExecutor? = nil
    ) {
        self.backendURL = backendURL
        self.getMachineID = getMachineID
        self.requestExecutor = requestExecutor ?? { request in
            try await session.data(for: request)
        }
    }

    func submit(
        accessToken: String,
        logs: [StructuredLogEntry]
    ) async throws -> StructuredLogReceipt {
        guard !logs.isEmpty else {
            return .init(logsProcessed: 0, logsDropped: 0)
        }

        let headers = try await createSandInferenceHeaders(
            backendUrl: backendURL.absoluteString,
            getAccessToken: { _ in accessToken },
            getMachineId: getMachineID,
            // Settlement must remain privacy-safe even after the live auth
            // owner has revoked the account. A missing privacy lookup therefore
            // fails closed instead of consulting a second auth state.
            resolveGhostMode: { _ in "true" }
        )
        let url = backendURL
            .appendingPathComponent("aiserver.v1.AnalyticsService")
            .appendingPathComponent("SubmitLogs")
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = TimeInterval(STRUCTURED_LOG_SUBMIT_DEADLINE_MS) / 1_000
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
        for (name, value) in headers.headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "logs": logs.map { entry in
                [
                    "level": entry.level.rawValue,
                    "message": entry.message,
                    "metadata": entry.metadata,
                    "timestamp": String(entry.timestamp),
                    "key": entry.key,
                ] as [String: Any]
            },
        ])

        let (data, response) = try await requestExecutor(request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorStructuredLogSubmitError.invalidResponse
        }
        guard (200..<300).contains(http.statusCode) else {
            throw IOSCursorStructuredLogSubmitError.httpStatus(http.statusCode)
        }
        let object: [String: Any]
        if data.isEmpty {
            object = [:]
        } else {
            guard let decoded = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                throw IOSCursorStructuredLogSubmitError.invalidResponse
            }
            object = decoded
        }
        let processed = (object["logsProcessed"] as? NSNumber)?.intValue
            ?? (object["logs_processed"] as? NSNumber)?.intValue
            ?? logs.count
        let dropped = (object["logsDropped"] as? NSNumber)?.intValue
            ?? (object["logs_dropped"] as? NSNumber)?.intValue
            ?? 0
        let receipt = StructuredLogReceipt(
            logsProcessed: processed,
            logsDropped: dropped
        )
        guard isValidLogShipReceipt(receipt, requestSize: logs.count) else {
            throw IOSCursorStructuredLogSubmitError.invalidReceipt(
                processed: processed,
                dropped: dropped,
                requested: logs.count
            )
        }
        return receipt
    }
}

@MainActor
final class IOSCursorSessionSettlementShipper {
    typealias ReportFailure = @MainActor (_ operation: String, _ error: Error) -> Void

    private let backend: IOSCursorStructuredLogBackend
    private let clientVersion: String
    private let appVersion: String
    private let arch: String
    private let platform: String
    private let nowMs: () -> Int64
    private let reportFailure: ReportFailure

    init(
        backendURL: URL,
        getMachineID: @escaping @Sendable () async throws -> String,
        session: URLSession = .shared,
        requestExecutor: IOSCursorStructuredLogBackend.RequestExecutor? = nil,
        clientVersion: String = getSandClientVersion(),
        appVersion: String = (Bundle.main.object(
            forInfoDictionaryKey: "CFBundleShortVersionString"
        ) as? String) ?? "0.18.0",
        arch: String = IOSCursorSessionSettlementShipper.currentArch,
        platform: String = "ios",
        nowMs: @escaping () -> Int64 = {
            Int64(Date().timeIntervalSince1970 * 1_000)
        },
        reportFailure: @escaping ReportFailure
    ) {
        backend = .init(
            backendURL: backendURL,
            getMachineID: getMachineID,
            session: session,
            requestExecutor: requestExecutor
        )
        self.clientVersion = clientVersion
        self.appVersion = appVersion
        self.arch = arch
        self.platform = platform
        self.nowMs = nowMs
        self.reportFailure = reportFailure
    }

    func ship(_ settlement: IOSCursorSessionSettlement) async {
        let projection = settlement.projection
        do {
            let machineID = try await backend.getMachineID()
            var metadata = [
                "client": "sand",
                "client.type": "sand",
                "client.machine_id": machineID,
                "client_version": clientVersion,
                "app_version": appVersion,
                "arch": arch,
                "platform": platform,
            ]
            for (key, value) in projection.metadata where !value.isEmpty {
                metadata[key] = value
            }
            let entry = StructuredLogEntry(
                level: projection.level == .warn ? .warn : .info,
                message: IOS_DESKTOP_SESSION_EVENT,
                metadata: metadata,
                timestamp: nowMs(),
                key: IOS_SAND_LOG_KEY
            )
            let receipt = try await backend.submit(
                accessToken: settlement.accessToken,
                logs: [entry]
            )
            guard receipt.logsProcessed + receipt.logsDropped == 1 else {
                throw IOSCursorStructuredLogSubmitError.invalidReceipt(
                    processed: receipt.logsProcessed,
                    dropped: receipt.logsDropped,
                    requested: 1
                )
            }
        } catch {
            reportFailure("session-settlement", error)
        }
    }

    private static var currentArch: String {
        #if arch(arm64)
        return "arm64"
        #elseif arch(x86_64)
        return "x86_64"
        #elseif arch(arm)
        return "arm"
        #elseif arch(i386)
        return "i386"
        #else
        return "unknown"
        #endif
    }
}

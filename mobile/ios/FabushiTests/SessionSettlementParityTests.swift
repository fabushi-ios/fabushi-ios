import Security
import XCTest
@testable import Fabushi

private actor IOSSettlementRequestRecorder {
    struct Capture: Sendable {
        let url: String
        let timeout: TimeInterval
        let headers: [String: String]
        let body: Data
    }

    private var values: [Capture] = []

    func record(_ request: URLRequest) {
        values.append(.init(
            url: request.url?.absoluteString ?? "",
            timeout: request.timeoutInterval,
            headers: request.allHTTPHeaderFields ?? [:],
            body: request.httpBody ?? Data()
        ))
    }

    func captures() -> [Capture] {
        values
    }
}

@MainActor
private final class IOSSettlementCredentialStore: IOSCursorCredentialStoring {
    var values: [String: String]
    var deleteFailures: Set<String>
    var readFailures: Set<String>
    private(set) var deleteAttempts: [String] = []

    init(
        values: [String: String],
        deleteFailures: Set<String> = [],
        readFailures: Set<String> = []
    ) {
        self.values = values
        self.deleteFailures = deleteFailures
        self.readFailures = readFailures
    }

    func readSecret(_ key: String) async throws -> String? {
        if readFailures.contains(key) {
            throw IOSCursorAuthError.keychain(errSecInteractionNotAllowed)
        }
        return values[key]
    }

    func writeSecret(_ key: String, value: String) async throws {
        values[key] = value
    }

    func deleteSecret(_ key: String) async throws {
        deleteAttempts.append(key)
        if deleteFailures.contains(key) {
            throw IOSCursorAuthError.keychain(errSecInteractionNotAllowed)
        }
        values.removeValue(forKey: key)
    }

    func waitForEncryptedStorage() async throws {}
}

final class SessionSettlementParityTests: XCTestCase {
    private func header(
        _ name: String,
        in headers: [String: String]
    ) -> String? {
        headers.first {
            $0.key.caseInsensitiveCompare(name) == .orderedSame
        }?.value
    }

    @MainActor
    func testSignedOutSettlementUsesDepartingTokenAndDesktopMetadata() async throws {
        let recorder = IOSSettlementRequestRecorder()
        var failures: [String] = []
        let shipper = IOSCursorSessionSettlementShipper(
            backendURL: try XCTUnwrap(URL(string: "https://api2.cursor.sh")),
            getMachineID: { "machine-42" },
            requestExecutor: { request in
                await recorder.record(request)
                let response = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: nil,
                    headerFields: ["content-type": "application/json"]
                )!
                return (
                    Data(#"{"logsProcessed":1,"logsDropped":0}"#.utf8),
                    response
                )
            },
            clientVersion: "0.18.0-test",
            appVersion: "9.8.7",
            arch: "arm64",
            platform: "ios",
            nowMs: { 1_787_000_123_456 },
            reportFailure: { _, error in
                failures.append(String(reflecting: type(of: error)))
            }
        )

        await shipper.ship(.signedOut(
            cause: .userAction,
            durable: true,
            accessToken: "departing-secret"
        ))

        XCTAssertTrue(failures.isEmpty)
        let captures = await recorder.captures()
        let capture = try XCTUnwrap(captures.first)
        XCTAssertTrue(capture.url.hasSuffix("/aiserver.v1.AnalyticsService/SubmitLogs"))
        XCTAssertEqual(capture.timeout, 15, accuracy: 0.001)
        XCTAssertEqual(header("Authorization", in: capture.headers), "Bearer departing-secret")
        XCTAssertEqual(header("x-cursor-client-type", in: capture.headers), "sand")
        XCTAssertNotNil(header("x-cursor-checksum", in: capture.headers))
        XCTAssertEqual(header("Connect-Protocol-Version", in: capture.headers), "1")

        let object = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: capture.body) as? [String: Any]
        )
        let logs = try XCTUnwrap(object["logs"] as? [[String: Any]])
        let entry = try XCTUnwrap(logs.first)
        XCTAssertEqual(entry["level"] as? Int, ClientLogLevel.info.rawValue)
        XCTAssertEqual(entry["message"] as? String, "sand.desktop.session")
        XCTAssertEqual(entry["timestamp"] as? String, "1787000123456")
        XCTAssertEqual(entry["key"] as? String, "sand")
        let metadata = try XCTUnwrap(entry["metadata"] as? [String: String])
        XCTAssertEqual(metadata["client"], "sand")
        XCTAssertEqual(metadata["client.type"], "sand")
        XCTAssertEqual(metadata["client.machine_id"], "machine-42")
        XCTAssertEqual(metadata["client_version"], "0.18.0-test")
        XCTAssertEqual(metadata["app_version"], "9.8.7")
        XCTAssertEqual(metadata["arch"], "arm64")
        XCTAssertEqual(metadata["platform"], "ios")
        XCTAssertEqual(metadata["phase"], "signed_out")
        XCTAssertEqual(metadata["cause"], "user_action")
        XCTAssertEqual(metadata["durable"], "true")
        XCTAssertFalse(String(data: capture.body, encoding: .utf8)!.contains("departing-secret"))
    }

    @MainActor
    func testKeychainUnavailableIsWarnAndInvalidReceiptUsesFailureChannel() async throws {
        let recorder = IOSSettlementRequestRecorder()
        var failures: [String] = []
        let shipper = IOSCursorSessionSettlementShipper(
            backendURL: try XCTUnwrap(URL(string: "https://api2.cursor.sh")),
            getMachineID: { "machine-7" },
            requestExecutor: { request in
                await recorder.record(request)
                let response = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: nil,
                    headerFields: nil
                )!
                return (
                    Data(#"{"logsProcessed":2,"logsDropped":0}"#.utf8),
                    response
                )
            },
            reportFailure: { _, error in
                failures.append(String(reflecting: type(of: error)))
            }
        )

        await shipper.ship(.keychainUnavailable(accessToken: "secret-7"))

        XCTAssertEqual(failures.count, 1)
        XCTAssertTrue(failures[0].contains("IOSCursorStructuredLogSubmitError"))
        let captures = await recorder.captures()
        let capture = try XCTUnwrap(captures.first)
        let object = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: capture.body) as? [String: Any]
        )
        let logs = try XCTUnwrap(object["logs"] as? [[String: Any]])
        let entry = try XCTUnwrap(logs.first)
        XCTAssertEqual(entry["level"] as? Int, ClientLogLevel.warn.rawValue)
        let metadata = try XCTUnwrap(entry["metadata"] as? [String: String])
        XCTAssertEqual(metadata["phase"], "keychain_unavailable")
        XCTAssertEqual(metadata["error_code"], "SAND-E0219")
        XCTAssertFalse(String(data: capture.body, encoding: .utf8)!.contains("secret-7"))
    }

    @MainActor
    func testLogoutDeletesBothSecretsThenShipsDurableSettlementWithFrozenToken() async throws {
        let recorder = IOSSettlementRequestRecorder()
        let store = IOSSettlementCredentialStore(values: [
            IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY: "departing-token",
            IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY: "refresh-token",
            iosMachineIDSecretKey: "machine-logout",
        ])
        let service = IOSCursorAuthService(
            store: store,
            backendURL: try XCTUnwrap(URL(string: "https://api2.cursor.sh")),
            structuredLogRequestExecutor: { request in
                await recorder.record(request)
                let response = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: nil,
                    headerFields: nil
                )!
                return (
                    Data(#"{"logsProcessed":1,"logsDropped":0}"#.utf8),
                    response
                )
            }
        )

        try await service.logout()

        XCTAssertEqual(
            store.deleteAttempts,
            [IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY, IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY]
        )
        XCTAssertNil(store.values[IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY])
        XCTAssertNil(store.values[IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY])
        let captures = await recorder.captures()
        XCTAssertEqual(captures.count, 1)
        XCTAssertEqual(header("Authorization", in: captures[0].headers), "Bearer departing-token")
        let body = String(data: captures[0].body, encoding: .utf8) ?? ""
        XCTAssertTrue(body.contains(#""phase":"signed_out""#))
        XCTAssertTrue(body.contains(#""durable":"true""#))
        XCTAssertFalse(body.contains("departing-token"))

        do {
            _ = try await service.getValidAccessToken()
            XCTFail("revoked session must not reuse deleted credentials")
        } catch let error as IOSCursorAuthError {
            XCTAssertEqual(error, .signInRequired)
        }
    }

    @MainActor
    func testRefreshSecretReadFailureShipsOneKeychainUnavailableWithAlreadyReadAccessToken() async throws {
        let recorder = IOSSettlementRequestRecorder()
        let store = IOSSettlementCredentialStore(
            values: [
                IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY: "readable-access-token",
                IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY: "blocked-refresh-token",
                iosMachineIDSecretKey: "machine-keychain-read",
            ],
            readFailures: [IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY]
        )
        let service = IOSCursorAuthService(
            store: store,
            backendURL: try XCTUnwrap(URL(string: "https://api2.cursor.sh")),
            structuredLogRequestExecutor: { request in
                await recorder.record(request)
                let response = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: nil,
                    headerFields: nil
                )!
                return (
                    Data(#"{"logsProcessed":1,"logsDropped":0}"#.utf8),
                    response
                )
            }
        )

        do {
            _ = try await service.getValidAccessToken()
            XCTFail("refresh-token Keychain failure must fail auth")
        } catch {
            XCTAssertEqual(error as? IOSCursorAuthError, .keychain(errSecInteractionNotAllowed))
        }

        do {
            _ = try await service.getValidAccessToken()
            XCTFail("repeat Keychain failure must remain a real auth failure")
        } catch {
            XCTAssertEqual(error as? IOSCursorAuthError, .keychain(errSecInteractionNotAllowed))
        }

        let captures = await recorder.captures()
        XCTAssertEqual(captures.count, 1)
        XCTAssertEqual(header("Authorization", in: captures[0].headers), "Bearer readable-access-token")
        let body = String(data: captures[0].body, encoding: .utf8) ?? ""
        XCTAssertTrue(body.contains(#""phase":"keychain_unavailable""#))
        XCTAssertFalse(body.contains("readable-access-token"))
        XCTAssertFalse(body.contains("blocked-refresh-token"))
    }

    @MainActor
    func testFailedLogoutStillShipsDurableFalseBeforeReturningKeychainErrorAndDedupes() async throws {
        let recorder = IOSSettlementRequestRecorder()
        let store = IOSSettlementCredentialStore(
            values: [
                IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY: "retained-token",
                IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY: "refresh-token",
                iosMachineIDSecretKey: "machine-retained",
            ],
            deleteFailures: [IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY]
        )
        let service = IOSCursorAuthService(
            store: store,
            backendURL: try XCTUnwrap(URL(string: "https://api2.cursor.sh")),
            structuredLogRequestExecutor: { request in
                await recorder.record(request)
                let response = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: nil,
                    headerFields: nil
                )!
                return (
                    Data(#"{"logsProcessed":1,"logsDropped":0}"#.utf8),
                    response
                )
            }
        )

        do {
            try await service.logout()
            XCTFail("failed Keychain deletion must remain a real logout error")
        } catch {
            XCTAssertEqual(error as? IOSCursorAuthError, .keychain(errSecInteractionNotAllowed))
        }

        XCTAssertEqual(
            store.deleteAttempts,
            [IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY, IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY]
        )
        XCTAssertEqual(store.values[IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY], "retained-token")
        XCTAssertNil(store.values[IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY])

        let firstPass = await recorder.captures()
        XCTAssertEqual(firstPass.count, 2)
        let firstBody = String(data: firstPass[0].body, encoding: .utf8) ?? ""
        let secondBody = String(data: firstPass[1].body, encoding: .utf8) ?? ""
        XCTAssertTrue(firstBody.contains(#""phase":"signed_out""#))
        XCTAssertTrue(firstBody.contains(#""durable":"false""#))
        XCTAssertTrue(secondBody.contains(#""phase":"keychain_unavailable""#))
        XCTAssertFalse(firstBody.contains("retained-token"))
        XCTAssertFalse(secondBody.contains("retained-token"))

        do {
            _ = try await service.getValidAccessToken()
            XCTFail("retained-after-failed-logout credentials must remain revoked")
        } catch let error as IOSCursorAuthError {
            XCTAssertEqual(error, .signInRequired)
        }

        do {
            try await service.logout()
            XCTFail("the retained access-token delete still fails")
        } catch {
            XCTAssertEqual(error as? IOSCursorAuthError, .keychain(errSecInteractionNotAllowed))
        }
        let finalCaptures = await recorder.captures()
        XCTAssertEqual(finalCaptures.count, 2)
    }

    @MainActor
    func testNetworkFailureDoesNotFabricateSettlementSuccess() async throws {
        var failures: [String] = []
        let shipper = IOSCursorSessionSettlementShipper(
            backendURL: try XCTUnwrap(URL(string: "https://api2.cursor.sh")),
            getMachineID: { "machine-network" },
            requestExecutor: { _ in
                throw URLError(.notConnectedToInternet)
            },
            reportFailure: { operation, error in
                failures.append("\(operation):\(String(reflecting: type(of: error)))")
            }
        )

        await shipper.ship(.signedOut(
            cause: .sessionRevoked,
            durable: true,
            accessToken: "network-secret"
        ))

        XCTAssertEqual(failures.count, 1)
        XCTAssertTrue(failures[0].hasPrefix("session-settlement:"))
    }
}

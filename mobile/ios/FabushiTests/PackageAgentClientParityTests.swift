import XCTest
@testable import Fabushi

final class PackageAgentClientParityTests: XCTestCase {
    func testAuthUpgradePaymentAndConfigErrorsRequireTheExpectedAction() {
        let auth = classifyAgentClientError(.connect(.init(
            code: 2,
            message: "login required",
            errorCode: "AUTH_TOKEN_EXPIRED"
        )), requestId: "req-auth")
        XCTAssertEqual(auth.kind, .actionRequired("login"))
        XCTAssertEqual(auth.requestId, "req-auth")

        let upgrade = classifyAgentClientError(.connect(.init(
            code: 8,
            message: "limit",
            errorCode: "PRO_USER_USAGE_LIMIT"
        )))
        XCTAssertEqual(upgrade.kind, .actionRequired("upgrade"))

        let payment = classifyAgentClientError(.connect(.init(
            code: 9,
            message: "pricing",
            errorCode: "USAGE_PRICING_REQUIRED"
        )))
        XCTAssertEqual(payment.kind, .actionRequired("payment"))

        let config = classifyAgentClientError(.connect(.init(
            code: 3,
            message: "bad key",
            errorCode: "BAD_API_KEY"
        )))
        XCTAssertEqual(config.kind, .actionRequired("config"))
    }

    func testBackendActionOverridesBuiltInBucketsAndPreservesDisplayMetadata() {
        let result = classifyAgentClientError(.connect(.init(
            code: 13,
            message: "fallback",
            metadata: ["x-cursor-inference-request-error-type": "provider_overloaded"],
            errorCode: "UNAUTHORIZED",
            title: "Action",
            detail: "Reconnect workspace",
            isRetryable: false,
            backendActionRequired: "reconnect"
        )))

        XCTAssertEqual(result.kind, .actionRequired("reconnect"))
        XCTAssertEqual(result.message, "Action Reconnect workspace")
        XCTAssertEqual(result.displayInfo?.inferenceRequestErrorType, "provider_overloaded")
        XCTAssertEqual(result.displayInfo?.errorCode, "UNAUTHORIZED")
    }

    func testCancellationAndTransportCancellationRemainDistinct() {
        let cancelled = classifyAgentClientError(.connect(.init(
            code: AgentClientConnectCode.cancelled,
            message: "user cancelled"
        )))
        XCTAssertEqual(cancelled.kind, .cancelled)
        XCTAssertFalse(cancelled.isTransport)

        let disconnected = classifyAgentClientError(.connect(.init(
            code: AgentClientConnectCode.aborted,
            message: "connection aborted ECONNRESET"
        )))
        XCTAssertEqual(disconnected.kind, .retriable)
        XCTAssertTrue(disconnected.isTransport)

        XCTAssertEqual(
            classifyAgentClientError(.abort(message: "stopped")).kind,
            .cancelled
        )
    }

    func testNonRetryableAndMissingConversationBranchesAreTerminal() {
        let terminal = classifyAgentClientError(.connect(.init(
            code: 13,
            message: "custom",
            errorCode: "CUSTOM_MESSAGE",
            title: "Request failed",
            detail: "Try a different input",
            isRetryable: false
        )))
        XCTAssertEqual(terminal.kind, .nonRetriable)
        XCTAssertEqual(terminal.message, "Request failed Try a different input")

        let missing = classifyAgentClientError(.blobNotFound(message: "blob abc was not found"))
        XCTAssertEqual(missing.kind, .nonRetriable)
        XCTAssertEqual(missing.displayInfo?.title, "Conversation data missing")
        XCTAssertEqual(missing.displayInfo?.isRetryable, false)
    }

    func testUnknownAndNetworkFailuresDefaultToRetryable() {
        let network = classifyAgentClientError(.standard(
            message: "socket hang up",
            name: "Error",
            code: "ECONNRESET",
            causes: []
        ))
        XCTAssertEqual(network.kind, .retriable)
        XCTAssertTrue(network.isTransport)

        let opaque = classifyAgentClientError(.opaque("unexpected value"))
        XCTAssertEqual(opaque.kind, .retriable)
        XCTAssertFalse(opaque.isTransport)
    }
}

import XCTest
@testable import Fabushi

private struct TaggedTestError: LocalizedError, SandStringCodedError {
    let sandErrorCode: String?
    var errorDescription: String? { "test-message" }
}

final class SharedErrorRegistryParityTests: XCTestCase {
    func testRegistryContainsFullPinnedErrorVocabulary() {
        XCTAssertEqual(SAND_ERROR_DEFINITIONS.count, 87)
        XCTAssertEqual(SAND_ERROR_DEFINITIONS["SAND-E0109"]?.name, "logShipTimeout")
        XCTAssertEqual(SAND_ERROR_DEFINITIONS["SAND-E0109"]?.domain, "transport")
        XCTAssertEqual(SAND_ERROR_DEFINITIONS["SAND-E0109"]?.retryable, true)
        XCTAssertEqual(SAND_ERROR_DEFINITIONS["SAND-E0707"]?.name, "clientQueuedSendAckExpired")
    }

    func testWireFallbackAndDeclaredBoundedTags() {
        let known = SandError.gatewayHttp5xx([
            "httpStatus": .int(503),
            "undeclared": .string("secret"),
        ])
        let knownTags = sandErrorTags(known)
        XCTAssertEqual(knownTags["error_code"], "SAND-E0103")
        XCTAssertEqual(knownTags["http_status"], "503")
        XCTAssertNil(knownTags["undeclared"])

        let unknown = SandErrorValue(code: "NOT-REGISTERED", payload: ["secret": .string("leak")])
        let unknownTags = sandErrorTags(unknown)
        XCTAssertEqual(unknownTags["error_code"], UNREGISTERED_CODE)
        XCTAssertEqual(unknownTags["error_domain"], "registry")
        XCTAssertNil(unknownTags["secret"])
    }

    func testStringPayloadTagsAreStrictlyBounded() {
        let safe = SandError.webauthnSignFailed([
            "domError": .string("NotAllowedError"),
            "signErrorClass": .string("cancelled_or_timeout"),
        ])
        XCTAssertEqual(sandErrorTags(safe)["sign_error_class"], "cancelled_or_timeout")

        let unsafe = SandError.webauthnSignFailed([
            "domError": .string("contains whitespace and must not ship"),
        ])
        XCTAssertNil(sandErrorTags(unsafe)["dom_error"])
    }

    func testConnectorAuthTelemetryBoundsIdentityAndErrorMetadata() {
        let record = connectorAuthTelemetry(
            .init(
                phase: "callback",
                outcome: "failed",
                serverName: "Git Hub",
                serverId: "srv:123",
                reauth: true,
                error: SandError.connectorOauthCallbackFailed(["reason": .string("timeout")])
            ),
            surface: "settings"
        )
        XCTAssertEqual(record.level, "warn")
        XCTAssertEqual(record.metadata["connector"], "github")
        XCTAssertEqual(record.metadata["server_id"], "srv:123")
        XCTAssertEqual(record.metadata["error_code"], "SAND-E0205")
        XCTAssertEqual(record.metadata["reason"], "timeout")
    }

    func testNativeErrorMessageAndLogTagBridge() {
        let error = TaggedTestError(sandErrorCode: "E_TEST")
        XCTAssertEqual(errorMessage(error), "test-message")
        XCTAssertTrue(errorLogTag(error).contains("E_TEST"))
        XCTAssertEqual(errorMessage("plain"), "plain")
    }
}

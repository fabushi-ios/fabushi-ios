import XCTest
@testable import Fabushi

final class ExternalURLPolicyParityTests: XCTestCase {
    func testUntrustedExternalURLsStripForeignWebLoginCredentials() {
        XCTAssertEqual(
            ExternalURLPolicy.parseAllowed(
                "https://api.ombhrum.com/login?keep=1&tgWebAuthToken=foreign&%2561utologin_token=foreign-two#/route/?ok=2&autologin_token=foreign-three"
            ),
            "https://api.ombhrum.com/login?keep=1#/route/?ok=2"
        )
    }

    func testEncodedOrRecasedForeignLoginTokensCannotSurvive() {
        XCTAssertEqual(
            ExternalURLPolicy.parseAllowed(
                "https://api.ombhrum.com/login?TGWEBAUTHanything=secret&%253FtgWebAuthToken=secret-two&safe=1"
            ),
            "https://api.ombhrum.com/login?safe=1"
        )
    }

    func testServerAcceptedAuthURLRequiresExactFirstPartyOrigin() {
        let accepted = "https://api.ombhrum.com/loginDeepControl?autologin_token=server-issued&attempt=abc#done"
        XCTAssertEqual(
            ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
                accepted,
                expectedOrigin: "https://api.ombhrum.com"
            ),
            accepted
        )
        XCTAssertNil(
            ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
                accepted,
                expectedOrigin: "https://other.example"
            )
        )
        XCTAssertNil(
            ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
                "https://user:secret@api.ombhrum.com/login",
                expectedOrigin: "https://api.ombhrum.com"
            )
        )
        XCTAssertNil(
            ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
                "http://api.ombhrum.com/login",
                expectedOrigin: "https://api.ombhrum.com"
            )
        )
    }

    func testOnlyAllowlistedNonHTTPFamiliesAreAccepted() {
        XCTAssertEqual(ExternalURLPolicy.parseAllowed("mailto:help@example.com"), "mailto:help@example.com")
        XCTAssertNil(ExternalURLPolicy.parseAllowed("javascript:alert(1)"))
    }
}

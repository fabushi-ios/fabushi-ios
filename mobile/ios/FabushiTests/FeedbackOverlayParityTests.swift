import XCTest
@testable import Fabushi

final class FeedbackOverlayParityTests: XCTestCase {
    func testFeedbackCodesNormalizeDesktopContractAndUnknownFailsClosed() {
        XCTAssertEqual(AccountFeedbackCode.normalize("access-denied"), .accessDenied)
        XCTAssertEqual(AccountFeedbackCode.normalize("invalid-feedback"), .invalidFeedback)
        XCTAssertEqual(AccountFeedbackCode.normalize("not-signed-in"), .notSignedIn)
        XCTAssertEqual(AccountFeedbackCode.normalize("rate-limited"), .rateLimited)
        XCTAssertEqual(AccountFeedbackCode.normalize("subscription-required"), .subscriptionRequired)
        XCTAssertEqual(AccountFeedbackCode.normalize("unavailable"), .unavailable)
        XCTAssertEqual(AccountFeedbackCode.normalize("future-code"), .unavailable)
        XCTAssertEqual(AccountFeedbackCode.normalize(nil), .unavailable)
    }

    func testFeedbackErrorMessagesRemainRecoverableAndUserFacing() {
        for code in AccountFeedbackCode.allCases {
            XCTAssertFalse(code.localizedMessage.isEmpty)
        }
        XCTAssertEqual(AccountFeedbackError(code: .rateLimited).errorDescription, AccountFeedbackCode.rateLimited.localizedMessage)
    }

    func testFeedbackViewStateKeepsSendingAndSentDistinctFromRecoverableFailure() {
        XCTAssertNotEqual(AccountFeedbackViewState.sending, .idle)
        XCTAssertNotEqual(AccountFeedbackViewState.sent, .sending)
        XCTAssertEqual(AccountFeedbackViewState.failed(.unavailable), .failed(.unavailable))
        XCTAssertNotEqual(AccountFeedbackViewState.failed(.rateLimited), .sent)
    }
}

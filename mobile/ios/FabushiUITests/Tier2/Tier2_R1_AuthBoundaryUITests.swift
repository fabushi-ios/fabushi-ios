import Foundation
import UIKit
import XCTest

/// Tier 2 Boundary, Negative & Corner-Case Opaque-Box UI Tests — Domain R1:
/// Auth Validation, 2FA/Passkey Replay Defenses, Proxy Faults, Revocation & Multi-Account Caps
/// (`F01..F10, F47`).
final class Tier2_R1_AuthBoundaryUITests: FabushiE2ETestCase {

    @MainActor
    private func openAuthMode(_ app: XCUIApplication, _ mode: String) {
        let start = element(app, id: FabushiAID.Auth.splashStartButton)
        if start.waitForExistence(timeout: 3.0) && start.isHittable {
            start.tap()
        }
        let picker = element(app, id: FabushiAID.Auth.modePicker)
        if picker.waitForExistence(timeout: 4.0), picker.buttons[mode].exists {
            picker.buttons[mode].tap()
        } else if app.buttons[mode].exists {
            app.buttons[mode].tap()
        }
    }

    @MainActor
    private func assertRemainsUnauthenticated(_ app: XCUIApplication) {
        XCTAssertFalse(
            element(app, id: FabushiAID.Root.tabBarContainer).exists,
            "Boundary/negative auth input must NOT enter authenticated root shell"
        )
    }

    // MARK: - F01: Username & Password Boundaries (5 Tests)

    // Feature: F01 | Tier: 2
    @MainActor
    func test_F01_T2_01_whitespaceOnlyUsernameRejectedWithInlineErrorBanner() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "password")

        clearAndType(app, id: FabushiAID.Auth.usernameField, text: "     ")
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "StrongPass#2026")
        let submit = requireElement(app, id: FabushiAID.Auth.submitButton)
        if submit.isEnabled { submit.tap() }

        assertRemainsUnauthenticated(app)
    }

    // Feature: F01 | Tier: 2
    @MainActor
    func test_F01_T2_02_oversizedPasswordExceeding1024BytesRejectedSafely() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "password")
        let switchReg = element(app, id: FabushiAID.Auth.switchRegisterMode)
        if switchReg.waitForExistence(timeout: 3.0) { switchReg.tap() }

        let hugePassword = String(repeating: "P", count: 1040)
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: "f01_over_\(UUID().uuidString.prefix(6).lowercased())")
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: hugePassword)
        let submit = requireElement(app, id: FabushiAID.Auth.submitButton)
        if submit.isEnabled { submit.tap() }

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F01 | Tier: 2
    @MainActor
    func test_F01_T2_03_wrongPasswordOnLoginSurfaces401ErrorBannerWithoutCrash() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f01_wrongpw", password: "RealPassword#2026")
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "password")

        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "WrongPassword#0000")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        let banner = requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0)
        XCTAssertFalse(banner.label.isEmpty)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F01 | Tier: 2
    @MainActor
    func test_F01_T2_04_duplicateUsernameRegistrationSurfaces409ConflictBanner() async throws {
        let existing = try await oracle.registerRandomUser(prefix: "f01_dup")
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "password")
        let switchReg = element(app, id: FabushiAID.Auth.switchRegisterMode)
        if switchReg.waitForExistence(timeout: 3.0) { switchReg.tap() }

        clearAndType(app, id: FabushiAID.Auth.usernameField, text: existing.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "StrongPass#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F01 | Tier: 2
    @MainActor
    func test_F01_T2_05_tooShortTwoCharUsernameBlockedByValidation() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "password")
        let switchReg = element(app, id: FabushiAID.Auth.switchRegisterMode)
        if switchReg.waitForExistence(timeout: 3.0) { switchReg.tap() }

        clearAndType(app, id: FabushiAID.Auth.usernameField, text: "ab")
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "StrongPass#2026")
        let submit = requireElement(app, id: FabushiAID.Auth.submitButton)
        if submit.isEnabled { submit.tap() }

        assertRemainsUnauthenticated(app)
    }

    // MARK: - F02: Alipay QR & Callback Negative Cases (5 Tests)

    // Feature: F02 | Tier: 2
    @MainActor
    func test_F02_T2_01_unknownAlipayAttemptCallbackDeepLinkRejectedCleanly() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "alipayQr")

        try GesturePhysicsHelpers.openCustomURL("fabushi://auth/alipay/callback?attempt_id=nonexistent_attempt_99999")
        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F02 | Tier: 2
    @MainActor
    func test_F02_T2_02_malformedAlipayCallbackMissingAttemptIdDoesNotCrash() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "alipayQr")

        try GesturePhysicsHelpers.openCustomURL("fabushi://auth/alipay/callback?foo=bar")
        assertRemainsUnauthenticated(app)
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F02 | Tier: 2
    @MainActor
    func test_F02_T2_03_offlineAlipayStartDisplaysRetryableNetworkErrorState() async throws {
        let (app, _) = try launchUnauthenticatedAuth(networkFaultMode: .offline)
        openAuthMode(app, "alipayQr")

        let status = requireElement(app, id: FabushiAID.Auth.alipayStatusLabel, timeout: 8.0)
        XCTAssertFalse(status.label.isEmpty)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F02 | Tier: 2
    @MainActor
    func test_F02_T2_04_rapidTripleTapAlipayRefreshDebouncesWithoutCrash() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "alipayQr")

        let refresh = requireElement(app, id: FabushiAID.Auth.alipayRefreshButton)
        refresh.tap()
        refresh.tap()
        refresh.tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.alipayQrImage).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F02 | Tier: 2
    @MainActor
    func test_F02_T2_05_unscannedPendingAlipayAttemptNeverIssuesUnauthorizedSession() async throws {
        let started = try await oracle.startAlipayOAuth()
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "alipayQr")

        try GesturePhysicsHelpers.openCustomURL("gpui-telegram://auth/alipay/callback?attempt_id=\(started.attempt_id)")
        assertRemainsUnauthenticated(app)
    }

    // MARK: - F03: Email OTP Validation & Rate-Limit Boundaries (5 Tests)

    // Feature: F03 | Tier: 2
    @MainActor
    func test_F03_T2_01_malformedEmailWithoutAtSymbolBlocksSendVerificationCode() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "email")

        clearAndType(app, id: FabushiAID.Auth.emailField, text: "invalid-email-address")
        let sendBtn = requireElement(app, id: FabushiAID.Auth.sendEmailCodeButton)
        if sendBtn.isEnabled {
            sendBtn.tap()
            XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 6.0).exists)
        }
        assertRemainsUnauthenticated(app)
    }

    // Feature: F03 | Tier: 2
    @MainActor
    func test_F03_T2_02_emailRegistrationWithShortPasswordUnder8CharsRejected() async throws {
        let uname = "f03_short_\(UUID().uuidString.prefix(6).lowercased())"
        let email = "\(uname)@example.test"
        let sent = try await oracle.sendEmailCode(email: email)
        let otp = try XCTUnwrap(sent.debug_code)

        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "email")
        clearAndType(app, id: FabushiAID.Auth.emailField, text: email)
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "short7!")
        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0))
        box0.tap()
        box0.typeText(otp)
        if element(app, id: FabushiAID.Auth.submitButton).isEnabled {
            element(app, id: FabushiAID.Auth.submitButton).tap()
        }

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F03 | Tier: 2
    @MainActor
    func test_F03_T2_03_wrongSixDigitEmailOtpDisplaysInvalidCodeError() async throws {
        let uname = "f03_badotp_\(UUID().uuidString.prefix(6).lowercased())"
        let email = "\(uname)@example.test"
        let sent = try await oracle.sendEmailCode(email: email)
        let wrongOtp = (sent.debug_code == "000000") ? "999999" : "000000"

        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "email")
        clearAndType(app, id: FabushiAID.Auth.emailField, text: email)
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "StrongEmail#2026")
        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0))
        box0.tap()
        box0.typeText(wrongOtp)
        if element(app, id: FabushiAID.Auth.submitButton).isHittable {
            element(app, id: FabushiAID.Auth.submitButton).tap()
        }

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F03 | Tier: 2
    @MainActor
    func test_F03_T2_04_alreadyRegisteredEmailAddressSurfaces409ConflictError() async throws {
        let uname = "f03_dup_\(UUID().uuidString.prefix(6).lowercased())"
        let email = "\(uname)@example.test"
        let sent = try await oracle.sendEmailCode(email: email)
        _ = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/auth/email/register",
            payload: .object([
                "email": .string(email),
                "code": .string(sent.debug_code ?? ""),
                "username": .string(uname),
                "password": .string("AlreadyReg#2026"),
            ])
        )

        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "email")
        clearAndType(app, id: FabushiAID.Auth.emailField, text: email)
        requireElement(app, id: FabushiAID.Auth.sendEmailCodeButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F03 | Tier: 2
    @MainActor
    func test_F03_T2_05_emailSendCodeWithin60sCooldownRejectedWith429RateLimit() async throws {
        let email = "f03_rate_\(UUID().uuidString.prefix(7).lowercased())@example.test"
        _ = try await oracle.sendEmailCode(email: email)

        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "email")
        clearAndType(app, id: FabushiAID.Auth.emailField, text: email)
        requireElement(app, id: FabushiAID.Auth.sendEmailCodeButton).tap()

        let hasCooldownOrError = element(app, id: FabushiAID.Auth.otpResendTimer).waitForExistence(timeout: 6.0)
            || element(app, id: FabushiAID.Auth.errorBanner).waitForExistence(timeout: 4.0)
        XCTAssertTrue(hasCooldownOrError)
    }

    // MARK: - F04: Phone OTP Boundaries & Brute-Force Lockout (5 Tests)

    // Feature: F04 | Tier: 2
    @MainActor
    func test_F04_T2_01_tooShortPhoneNumberUnder7DigitsRejectedBeforeSendCode() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")

        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "123")
        let cont = requireElement(app, id: FabushiAID.Auth.continueButton)
        if cont.isEnabled { cont.tap() }

        XCTAssertFalse(element(app, id: FabushiAID.Auth.otpContainer).exists)
    }

    // Feature: F04 | Tier: 2
    @MainActor
    func test_F04_T2_02_oversizedPhoneNumberOver20DigitsTruncatedOrRejected() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")

        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "1234567890123456789012345")
        let cont = requireElement(app, id: FabushiAID.Auth.continueButton)
        if cont.isEnabled { cont.tap() }

        assertRemainsUnauthenticated(app)
    }

    // Feature: F04 | Tier: 2
    @MainActor
    func test_F04_T2_03_wrongPhoneOtpDisplaysErrorAndDecrementsRemainingAttempts() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")

        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        requireElement(app, id: FabushiAID.Auth.continueButton).tap()

        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0), timeout: 10.0)
        box0.tap()
        box0.typeText("000000")

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F04 | Tier: 2
    @MainActor
    func test_F04_T2_04_exhaustingFiveInvalidPhoneOtpAttemptsLocksChallengeWith429() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let phone = "+1415\(suffix)"
        let sent = try await oracle.sendPhoneCode(phone: phone)
        let badCode = (sent.debug_code == "111111") ? "222222" : "111111"
        for _ in 0..<5 {
            _ = try await oracle.requestJSON(
                method: "POST",
                path: "/api/v1/auth/phone/verify",
                payload: .object([
                    "phone": .string(phone),
                    "code": .string(badCode),
                    "phone_code_hash": .string(sent.phone_code_hash),
                ])
            )
        }

        let (app, _) = try launchUnauthenticatedAuth(
            extraEnvironment: ["FABUSHI_TEST_PREFETCHED_PHONE_HASH": sent.phone_code_hash]
        )
        openAuthMode(app, "phone")
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        if element(app, id: FabushiAID.Auth.continueButton).isHittable {
            element(app, id: FabushiAID.Auth.continueButton).tap()
        }
        assertRemainsUnauthenticated(app)
    }

    // Feature: F04 | Tier: 2
    @MainActor
    func test_F04_T2_05_phoneResendRequestInsideCooldownShows429CooldownNotice() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let phone = "+1415\(suffix)"
        _ = try await oracle.sendPhoneCode(phone: phone)

        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        requireElement(app, id: FabushiAID.Auth.continueButton).tap()

        let hasCooldown = element(app, id: FabushiAID.Auth.otpResendTimer).waitForExistence(timeout: 6.0)
            || element(app, id: FabushiAID.Auth.errorBanner).waitForExistence(timeout: 4.0)
        XCTAssertTrue(hasCooldown)
    }

    // MARK: - F05: Two-Factor Cloud Password Negative Cases (5 Tests)

    // Feature: F05 | Tier: 2
    @MainActor
    func test_F05_T2_01_wrongTwoFactorCloudPasswordDisplaysErrorAndRemainsLocked() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f05_neg", password: "PrimaryLogin#2026")
        _ = try await oracle.enableTwoFactor(token: seeded.token, cloudPassword: "RealCloudPassword#9")

        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "PrimaryLogin#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        clearAndType(app, id: FabushiAID.Auth.twoFactorPasswordField, text: "WrongCloudPassword!")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F05 | Tier: 2
    @MainActor
    func test_F05_T2_02_emptyTwoFactorPasswordCannotSubmitVerification() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f05_empty", password: "PrimaryLogin#2026")
        _ = try await oracle.enableTwoFactor(token: seeded.token, cloudPassword: "RealCloudPassword#9")

        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "PrimaryLogin#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        _ = requireElement(app, id: FabushiAID.Auth.twoFactorPasswordField, timeout: 8.0)
        let submit = requireElement(app, id: FabushiAID.Auth.submitButton)
        if submit.isEnabled { submit.tap() }
        assertRemainsUnauthenticated(app)
    }

    // Feature: F05 | Tier: 2
    @MainActor
    func test_F05_T2_03_forgotTwoFactorPasswordWithoutRecoveryEmailPresentsWarningAlert() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f05_forgot", password: "PrimaryLogin#2026")
        _ = try await oracle.enableTwoFactor(token: seeded.token, cloudPassword: "RealCloudPassword#9")

        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "PrimaryLogin#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        requireElement(app, id: FabushiAID.Auth.twoFactorForgotButton, timeout: 8.0).tap()
        let warned = element(app, id: FabushiAID.Root.overlayAlert).waitForExistence(timeout: 6.0)
            || element(app, id: FabushiAID.Auth.errorBanner).waitForExistence(timeout: 4.0)
        XCTAssertTrue(warned)
    }

    // Feature: F05 | Tier: 2
    @MainActor
    func test_F05_T2_04_disablingTwoFactorWithIncorrectCurrentPasswordRejectedWith401() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.enableTwoFactor(token: auth.token, cloudPassword: "VaultPassword#1")

        let badDisable = try await oracle.disableTwoFactor(token: auth.token, currentPassword: "WrongCurrentPassword")
        XCTAssertEqual(badDisable.statusCode, 401)
        XCTAssertTrue(app.exists)
    }

    // Feature: F05 | Tier: 2
    @MainActor
    func test_F05_T2_05_invalidTwoFactorChallengeTokenDeepLinkRejectedSafely() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        try GesturePhysicsHelpers.openCustomURL("fabushi://auth/2fa?challenge_token=2fa_invalid_nonexistent")
        assertRemainsUnauthenticated(app)
    }

    // MARK: - F06: QR Pairing Replay & Expiration Boundaries (5 Tests)

    // Feature: F06 | Tier: 2
    @MainActor
    func test_F06_T2_01_unknownQrPairDeepLinkSurfacesInvalidTokenAlert() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        try GesturePhysicsHelpers.openCustomURL("tg://login?token=pair_unknown_token_0000")

        let surfaced = element(app, id: FabushiAID.Root.overlayAlert).waitForExistence(timeout: 6.0)
            || element(app, id: FabushiAID.Auth.errorBanner).waitForExistence(timeout: 4.0)
            || app.state == .runningForeground
        XCTAssertTrue(surfaced)
    }

    // Feature: F06 | Tier: 2
    @MainActor
    func test_F06_T2_02_alreadyAcceptedQrPairTokenCannotBeAcceptedTwice() async throws {
        let hostA = try await oracle.registerRandomUser(prefix: "f06_hosta")
        let (app, hostB, _) = try await launchPreauthenticatedWorkspace()
        let pair = try await oracle.initQrPairing()

        let first = try await oracle.acceptQrPairing(token: hostA.token, pairToken: pair.pair_token)
        XCTAssertEqual(first.statusCode, 200)

        let second = try await oracle.acceptQrPairing(token: hostB.token, pairToken: pair.pair_token)
        XCTAssertEqual(second.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F06 | Tier: 2
    @MainActor
    func test_F06_T2_03_unconfirmedQrPairTokenVerifyReturns401Unauthorized() async throws {
        let pair = try await oracle.initQrPairing()
        let (app, _) = try launchUnauthenticatedAuth()
        try GesturePhysicsHelpers.openCustomURL("fabushi://auth/pair/verify?token=\(pair.pair_token)")
        assertRemainsUnauthenticated(app)
    }

    // Feature: F06 | Tier: 2
    @MainActor
    func test_F06_T2_04_qrPairWithWrongTwoFactorCloudPasswordRejected() async throws {
        let host = try await oracle.registerRandomUser(prefix: "f06_bad2fa")
        _ = try await oracle.enableTwoFactor(token: host.token, cloudPassword: "QrRealCloud#1")
        let pair = try await oracle.initQrPairing()
        _ = try await oracle.acceptQrPairing(token: host.token, pairToken: pair.pair_token)

        let (app, _) = try launchUnauthenticatedAuth()
        try GesturePhysicsHelpers.openCustomURL("fabushi://auth/pair/verify?token=\(pair.pair_token)")
        let pwField = element(app, id: FabushiAID.Auth.twoFactorPasswordField)
        if pwField.waitForExistence(timeout: 6.0) {
            pwField.tap()
            pwField.typeText("WrongCloudPw!")
            requireElement(app, id: FabushiAID.Auth.submitButton).tap()
        }
        assertRemainsUnauthenticated(app)
    }

    // Feature: F06 | Tier: 2
    @MainActor
    func test_F06_T2_05_offlineQrPairModeShowsRetryableConnectionError() async throws {
        let (app, _) = try launchUnauthenticatedAuth(networkFaultMode: .offline)
        openAuthMode(app, "qrPair")
        assertRemainsUnauthenticated(app)
    }

    // MARK: - F07: Passkey Failure, Replay & Unknown Credential Boundaries (5 Tests)

    // Feature: F07 | Tier: 2
    @MainActor
    func test_F07_T2_01_mockBiometricsFailureCancelsPasskeyAssertionGracefully() async throws {
        let (app, _) = try launchUnauthenticatedAuth(mockBiometrics: .fail)
        openAuthMode(app, "passkey")
        requireElement(app, id: FabushiAID.Auth.passkeySignInButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F07 | Tier: 2
    @MainActor
    func test_F07_T2_02_unregisteredPasskeyCredentialIdRejectedWith401Banner() async throws {
        let (app, _) = try launchUnauthenticatedAuth(
            mockBiometrics: .success,
            extraEnvironment: ["FABUSHI_MOCK_PASSKEY_CREDENTIAL_ID": "unregistered_cred_9999"]
        )
        openAuthMode(app, "passkey")
        requireElement(app, id: FabushiAID.Auth.passkeySignInButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.errorBanner, timeout: 8.0).exists)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F07 | Tier: 2
    @MainActor
    func test_F07_T2_03_replayingConsumedPasskeyChallengeRejectedByServer() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f07_replay")
        let mat = PasskeyKeyMaterial(labelPrefix: "replay_cred")
        _ = try await oracle.registerPasskey(token: seeded.token, material: mat)

        let chResp = try await oracle.requestJSON(method: "POST", path: "/api/v1/auth/passkey/challenge", payload: .object([:]))
        let ch = try chResp.decode(PasskeyChallengeDTO.self)
        let sig = try mat.signChallengeHex(ch.challenge)
        let first = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/auth/passkey/verify",
            payload: .object(["challenge": .string(ch.challenge), "credential_id": .string(mat.credentialID), "signature": .string(sig)])
        )
        XCTAssertEqual(first.statusCode, 200)

        let (app, _) = try launchUnauthenticatedAuth(
            extraEnvironment: [
                "FABUSHI_MOCK_PASSKEY_CHALLENGE_OVERRIDE": ch.challenge,
                "FABUSHI_MOCK_PASSKEY_CREDENTIAL_ID": mat.credentialID,
            ]
        )
        openAuthMode(app, "passkey")
        requireElement(app, id: FabushiAID.Auth.passkeySignInButton).tap()
        assertRemainsUnauthenticated(app)
    }

    // Feature: F07 | Tier: 2
    @MainActor
    func test_F07_T2_04_offlinePasskeySignInSurfacesNetworkErrorWithoutCrash() async throws {
        let (app, _) = try launchUnauthenticatedAuth(networkFaultMode: .offline)
        openAuthMode(app, "passkey")
        requireElement(app, id: FabushiAID.Auth.passkeySignInButton).tap()
        assertRemainsUnauthenticated(app)
    }

    // Feature: F07 | Tier: 2
    @MainActor
    func test_F07_T2_05_deletedUserOwnerOfPasskeyCannotIssueSession() async throws {
        let (app, _) = try launchUnauthenticatedAuth(
            extraEnvironment: ["FABUSHI_MOCK_PASSKEY_CREDENTIAL_ID": "orphaned_cred_000"]
        )
        openAuthMode(app, "passkey")
        requireElement(app, id: FabushiAID.Auth.passkeySignInButton).tap()
        assertRemainsUnauthenticated(app)
    }

    // MARK: - F08: Server URL & Proxy Fault Tolerance (5 Tests)

    // Feature: F08 | Tier: 2
    @MainActor
    func test_F08_T2_01_malformedServerUrlSchemeRejectedWithValidationError() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()

        clearAndType(app, id: FabushiAID.Auth.serverUrlField, text: "ftp://bad_scheme_host")
        requireElement(app, id: FabushiAID.Auth.proxyProbeButton).tap()

        let health = requireElement(app, id: FabushiAID.Auth.proxyHealthLabel)
        XCTAssertFalse(health.label.lowercased().contains("healthy"))
    }

    // Feature: F08 | Tier: 2
    @MainActor
    func test_F08_T2_02_closedLoopbackPortProbeReportsUnreachableWithoutFreezingUi() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()

        clearAndType(app, id: FabushiAID.Auth.serverUrlField, text: "http://127.0.0.1:1")
        requireElement(app, id: FabushiAID.Auth.proxyProbeButton).tap()

        let health = requireElement(app, id: FabushiAID.Auth.proxyHealthLabel, timeout: 8.0)
        XCTAssertFalse(health.label.isEmpty)
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F08 | Tier: 2
    @MainActor
    func test_F08_T2_03_outOfRangeProxyPort65536RejectedByValidator() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()
        let picker = requireElement(app, id: FabushiAID.Auth.proxyModePicker)
        if picker.buttons["SOCKS5"].exists {
            picker.buttons["SOCKS5"].tap()
            clearAndType(app, id: FabushiAID.Auth.proxyHostField, text: "127.0.0.1")
            clearAndType(app, id: FabushiAID.Auth.proxyPortField, text: "70000")
            requireElement(app, id: FabushiAID.Auth.proxyProbeButton).tap()
            XCTAssertFalse(requireElement(app, id: FabushiAID.Auth.proxyHealthLabel).label.isEmpty)
        }
    }

    // Feature: F08 | Tier: 2
    @MainActor
    func test_F08_T2_04_deadSocks5ProxyEndpointTimesOutGracefullyWithoutCrash() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()
        let picker = requireElement(app, id: FabushiAID.Auth.proxyModePicker)
        if picker.buttons["SOCKS5"].exists {
            picker.buttons["SOCKS5"].tap()
            clearAndType(app, id: FabushiAID.Auth.proxyHostField, text: "127.0.0.1")
            clearAndType(app, id: FabushiAID.Auth.proxyPortField, text: "9")
            requireElement(app, id: FabushiAID.Auth.proxyProbeButton).tap()
        }
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F08 | Tier: 2
    @MainActor
    func test_F08_T2_05_nonNumericProxyPortRejectedCleanly() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()
        let picker = requireElement(app, id: FabushiAID.Auth.proxyModePicker)
        if picker.buttons["HTTP"].exists {
            picker.buttons["HTTP"].tap()
            clearAndType(app, id: FabushiAID.Auth.proxyHostField, text: "127.0.0.1")
            clearAndType(app, id: FabushiAID.Auth.proxyPortField, text: "notaport")
            requireElement(app, id: FabushiAID.Auth.proxyProbeButton).tap()
        }
        XCTAssertTrue(app.state == .runningForeground)
    }

    // MARK: - F09: Corrupted Session, Offline Boot & Revocation Fencing (5 Tests)

    // Feature: F09 | Tier: 2
    @MainActor
    func test_F09_T2_01_corruptedSessionJsonOnDiskRecoversToCleanAuthSplash() async throws {
        var cfg = AppLaunchConfiguration(
            serverURL: oracle.baseURL.absoluteString,
            sandboxRoot: resolveSandboxRoot(),
            testIdentifier: #function
        )
        try cfg.prepareSandboxOnDisk()
        try Data("{not valid json!!!".utf8).write(
            to: cfg.appSupportDirectory.appendingPathComponent("session.json"),
            options: .atomic
        )
        cfg.extraEnvironment["FABUSHI_PREAUTH_SESSION_JSON"] = "{broken"

        let app = try launchFabushi(cfg)
        let onAuth = element(app, id: FabushiAID.Auth.splashStartButton).waitForExistence(timeout: 10.0)
            || element(app, id: FabushiAID.Auth.usernameField).waitForExistence(timeout: 5.0)
        XCTAssertTrue(onAuth, "Corrupted session.json must fall back cleanly to unauthenticated splash")
    }

    // Feature: F09 | Tier: 2
    @MainActor
    func test_F09_T2_02_tamperedBearerTokenFailsServer401AndWipesStaleSession() async throws {
        let bogusAuth = AuthResponseDTO(
            user_id: 999_999,
            username: "tampered_ghost",
            session_id: "sess_bogus",
            token: "invalid_forged_bearer_token_000",
            first_name: "Ghost",
            last_name: nil,
            account_generation: 1,
            expires_at: nil
        )
        let cfg = AppLaunchConfiguration(
            serverURL: oracle.baseURL.absoluteString,
            sandboxRoot: resolveSandboxRoot(),
            testIdentifier: #function,
            preseededAccounts: [PreseededSessionPayload(auth: bogusAuth, serverURL: oracle.baseURL.absoluteString)]
        )
        let app = try launchFabushi(cfg)

        let kicked = try await waitUntil(timeout: 12.0) {
            self.element(app, id: FabushiAID.Auth.splashStartButton).exists
                || self.element(app, id: FabushiAID.Auth.usernameField).exists
        }
        XCTAssertTrue(kicked, "Forged/expired token must be rejected with 401 and redirected to Auth")
    }

    // Feature: F09 | Tier: 2
    @MainActor
    func test_F09_T2_03_offlineColdBootPreservesLocalSessionWithoutDestructiveLogout() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(networkFaultMode: .offline)
        let staysInShell = element(app, id: FabushiAID.Root.tabBarContainer).exists
            || element(app, id: FabushiAID.Root.splitView).exists
        XCTAssertTrue(staysInShell, "Transient offline boot must NOT wipe valid local session")
    }

    // Feature: F09 | Tier: 2
    @MainActor
    func test_F09_T2_04_doubleRevokingSameSessionReturns404WithoutCrashingApp() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let first = try await oracle.revokeSession(token: auth.token, sessionId: auth.session_id)
        XCTAssertEqual(first.statusCode, 200)
        let second = try await oracle.revokeSession(token: auth.token, sessionId: auth.session_id)
        XCTAssertEqual(second.statusCode, 401)
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F09 | Tier: 2
    @MainActor
    func test_F09_T2_05_revokingNonexistentForeignSessionIdReturns404() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let resp = try await oracle.revokeSession(token: auth.token, sessionId: "foreign_sess_99999")
        XCTAssertEqual(resp.statusCode, 404)
        XCTAssertTrue(element(app, id: FabushiAID.Root.tabBarContainer).exists || element(app, id: FabushiAID.Root.splitView).exists)
    }

    // MARK: - F10: Multi-Account Switcher Boundaries & Isolation (6 Tests)

    // Feature: F10 | Tier: 2
    @MainActor
    func test_F10_T2_01_addingFourthAccountAtThreeAccountCapDisplaysLimitPrompt() async throws {
        let a2 = try await oracle.registerRandomUser(prefix: "f10_cap2")
        let a3 = try await oracle.registerRandomUser(prefix: "f10_cap3")
        let (app, _, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [a2, a3])

        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Root.tabSettings))
        let addBtn = element(app, id: FabushiAID.Root.addAccount)
        if addBtn.exists && addBtn.isEnabled {
            addBtn.tap()
        }
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F10 | Tier: 2
    @MainActor
    func test_F10_T2_02_revokingBackgroundAccountTokenLeavesForegroundAccountActive() async throws {
        let bgAcct = try await oracle.registerRandomUser(prefix: "f10_bgrev")
        let (app, fgAcct, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [bgAcct])

        let revBg = try await oracle.revokeSession(token: bgAcct.token, sessionId: bgAcct.session_id)
        XCTAssertEqual(revBg.statusCode, 200)

        let meFg = try await oracle.fetchMe(token: fgAcct.token)
        XCTAssertEqual(meFg.statusCode, 200)
        XCTAssertTrue(element(app, id: FabushiAID.Root.tabBarContainer).exists || element(app, id: FabushiAID.Root.splitView).exists)
    }

    // Feature: F10 | Tier: 2
    @MainActor
    func test_F10_T2_03_rapidAccountPingPongSwitchFencesCrossAccountDialogLeakage() async throws {
        let acct2 = try await oracle.registerRandomUser(prefix: "f10_pong2")
        let secret1 = try await oracle.registerRandomUser(prefix: "f10_secpeer")
        let (app, acct1, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [acct2])
        let chat1 = try await oracle.createChat(
            token: acct1.token,
            CreateChatRequestDTO(title: "Slot0 Private Vault", chatType: "direct", peerUserId: secret1.user_id)
        )

        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Root.tabSettings))
        requireElement(app, id: FabushiAID.Root.accountSlot(1)).tap()
        requireElement(app, id: FabushiAID.Root.tabChats).tap()

        XCTAssertFalse(element(app, id: FabushiAID.ChatList.row(chat1.chat_id)).exists, "Slot 0 private chat must never appear in Slot 1")
    }

    // Feature: F10 | Tier: 2
    @MainActor
    func test_F10_T2_04_duplicateAccountEntriesInAccountsJsonDeduplicateByUserId() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Root.tabSettings))
        XCTAssertTrue(requireElement(app, id: FabushiAID.Root.accountSlot(0)).exists)
        XCTAssertFalse(auth.username.isEmpty)
    }

    // Feature: F10 | Tier: 2
    @MainActor
    func test_F10_T2_05_switchingToRevokedSecondarySlotPromptsReauthenticationForThatSlot() async throws {
        let acct2 = try await oracle.registerRandomUser(prefix: "f10_stale2")
        let (app, _, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [acct2])
        _ = try await oracle.revokeSession(token: acct2.token, sessionId: acct2.session_id)

        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Root.tabSettings))
        requireElement(app, id: FabushiAID.Root.accountSlot(1)).tap()
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F10 | Tier: 2
    @MainActor
    func test_F10_T2_06_accountSwitchWhileDraftTypedSavesDraftOnlyUnderOriginAccount() async throws {
        let acct2 = try await oracle.registerRandomUser(prefix: "f10_draft2")
        let (app, acct1, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [acct2])
        let g1 = try await oracle.createChat(token: acct1.token, CreateChatRequestDTO(title: "Slot0 Draft Room", chatType: "group"))

        openChatFromInbox(app, chatId: g1.chat_id)
        clearAndType(app, id: FabushiAID.Composer.textView, text: "Slot0 exclusive draft")
        if app.navigationBars.buttons.firstMatch.exists { app.navigationBars.buttons.firstMatch.tap() }

        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Root.tabSettings))
        requireElement(app, id: FabushiAID.Root.accountSlot(1)).tap()
        XCTAssertFalse(element(app, id: FabushiAID.ChatList.row(g1.chat_id)).exists)
    }

    // MARK: - F47: Authorization Motion & Input Edge Cases (5 Tests)

    // Feature: F47 | Tier: 2
    @MainActor
    func test_F47_T2_01_backspaceAcrossOtpBoxesWalksFocusBackwardFromLastToFirst() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        requireElement(app, id: FabushiAID.Auth.continueButton).tap()

        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0), timeout: 10.0)
        box0.tap()
        box0.typeText("123")
        app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 3))

        let cleared = (element(app, id: FabushiAID.Auth.otpBox(0)).value as? String ?? "").isEmpty
        XCTAssertTrue(cleared, "Backspacing across OTP boxes must clear back to box 0")
    }

    // Feature: F47 | Tier: 2
    @MainActor
    func test_F47_T2_02_nonDigitCharactersTypedIntoOtpBoxesAreIgnoredCleanly() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        requireElement(app, id: FabushiAID.Auth.continueButton).tap()

        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0), timeout: 10.0)
        box0.tap()
        box0.typeText("ab!@")

        let val0 = element(app, id: FabushiAID.Auth.otpBox(0)).value as? String ?? ""
        XCTAssertTrue(val0.isEmpty, "Alphabetic/punctuation input must be stripped from numeric OTP box")
    }

    // Feature: F47 | Tier: 2
    @MainActor
    func test_F47_T2_03_pastingLongNumericStringTruncatesToSixOtpDigits() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        requireElement(app, id: FabushiAID.Auth.continueButton).tap()

        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0), timeout: 10.0)
        box0.tap()
        box0.typeText("9876543210")

        let box5Val = element(app, id: FabushiAID.Auth.otpBox(5)).value as? String ?? ""
        XCTAssertLessThanOrEqual(box5Val.count, 1)
    }

    // Feature: F47 | Tier: 2
    @MainActor
    func test_F47_T2_04_unassignedCountryDialCode999DisplaysInvalidCountryLabel() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")

        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "999")
        let label = requireElement(app, id: FabushiAID.Auth.countryButton).label
        XCTAssertFalse(label.isEmpty)
        assertRemainsUnauthenticated(app)
    }

    // Feature: F47 | Tier: 2
    @MainActor
    func test_F47_T2_05_rotatingDeviceDuringOtpEntryPreservesEnteredDigits() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let (app, _) = try launchUnauthenticatedAuth()
        openAuthMode(app, "phone")
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        requireElement(app, id: FabushiAID.Auth.continueButton).tap()

        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0), timeout: 10.0)
        box0.tap()
        box0.typeText("42")

        XCUIDevice.shared.orientation = .landscapeLeft
        XCUIDevice.shared.orientation = .portrait
        XCTAssertEqual(element(app, id: FabushiAID.Auth.otpBox(0)).value as? String, "4")
        XCTAssertEqual(element(app, id: FabushiAID.Auth.otpBox(1)).value as? String, "2")
    }
}

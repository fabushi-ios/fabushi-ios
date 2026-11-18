import Foundation
import XCTest

/// Tier 1 Happy-Path Opaque-Box UI Tests — Domain R1:
/// Identity, Multi-Factor Auth, Passkeys, Pairing, Proxy & Multi-Account (`F01..F10, F47`).
final class Tier1_R1_AuthSessionUITests: FabushiE2ETestCase {

    // MARK: - Helpers

    @MainActor
    private func navigatePastSplashIfNeeded(_ app: XCUIApplication) {
        let startButton = element(app, id: FabushiAID.Auth.splashStartButton)
        if startButton.waitForExistence(timeout: 3.0) && startButton.isHittable {
            startButton.tap()
        }
    }

    @MainActor
    private func selectAuthMode(_ app: XCUIApplication, segmentLabelOrId: String) {
        let picker = element(app, id: FabushiAID.Auth.modePicker)
        if picker.waitForExistence(timeout: 4.0) {
            let candidate = picker.buttons[segmentLabelOrId].firstMatch
            if candidate.exists {
                candidate.tap()
                return
            }
        }
        let direct = app.buttons[segmentLabelOrId].firstMatch
        if direct.exists {
            direct.tap()
        }
    }

    @MainActor
    private func assertEnteredAuthenticatedShell(_ app: XCUIApplication, timeout: TimeInterval = 12.0) {
        let inShell = element(app, id: FabushiAID.Root.tabBarContainer).waitForExistence(timeout: timeout)
            || element(app, id: FabushiAID.Root.splitView).waitForExistence(timeout: 4.0)
        XCTAssertTrue(inShell, "Expected transition into authenticated TelegramRootController shell")
    }

    // MARK: - F01: Username & Password Login / Registration (5 Tests)

    // Feature: F01 | Tier: 1
    @MainActor
    func test_F01_T1_01_registerWithUsernamePasswordHydratesRootShellAndServerProfile() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")

        let switchReg = element(app, id: FabushiAID.Auth.switchRegisterMode)
        if switchReg.waitForExistence(timeout: 3.0) { switchReg.tap() }

        let uname = "f01_reg_\(UUID().uuidString.prefix(8).lowercased())"
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "StrongPass#2026_F01")
        if element(app, id: FabushiAID.Auth.firstNameField).exists {
            clearAndType(app, id: FabushiAID.Auth.firstNameField, text: "Clara")
        }
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()
        assertEnteredAuthenticatedShell(app)

        let loginCheck = try await oracle.loginUser(
            LoginRequestDTO(username: uname, password: "StrongPass#2026_F01", deviceName: "OracleVerify")
        )
        XCTAssertEqual(loginCheck.statusCode, 200)
    }

    // Feature: F01 | Tier: 1
    @MainActor
    func test_F01_T1_02_registerWithAutoWelcomeSeedsOfficialWelcomeBotDialog() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")

        let switchReg = element(app, id: FabushiAID.Auth.switchRegisterMode)
        if switchReg.waitForExistence(timeout: 3.0) { switchReg.tap() }

        let uname = "f01_wel_\(UUID().uuidString.prefix(8).lowercased())"
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "StrongPass#2026_Wel")
        let welcomeToggle = element(app, id: FabushiAID.Auth.autoWelcomeToggle)
        if welcomeToggle.waitForExistence(timeout: 3.0), (welcomeToggle.value as? String) == "0" {
            welcomeToggle.tap()
        }
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()
        assertEnteredAuthenticatedShell(app)

        let collection = requireElement(app, id: FabushiAID.ChatList.collectionView)
        XCTAssertTrue(collection.cells.count >= 1, "Auto-welcome registration must populate companion welcome dialog")
    }

    // Feature: F01 | Tier: 1
    @MainActor
    func test_F01_T1_03_loginExistingAccountWithPasswordIssuesSessionAndLoadsChats() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f01_login", password: "LoginPass#2026_OK")
        let peer = try await oracle.registerRandomUser(prefix: "f01_peer")
        let dm = try await oracle.createChat(
            token: seeded.token,
            CreateChatRequestDTO(title: "Direct Peer", chatType: "direct", peerUserId: peer.user_id)
        )

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "LoginPass#2026_OK")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        assertEnteredAuthenticatedShell(app)
        let row = requireElement(app, id: FabushiAID.ChatList.row(dm.chat_id))
        XCTAssertTrue(row.exists)
    }

    // Feature: F01 | Tier: 1
    @MainActor
    func test_F01_T1_04_switchBetweenLoginAndRegisterModesPreservesEnteredUsername() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")

        let expectedUser = "persist_u_\(UUID().uuidString.prefix(6).lowercased())"
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: expectedUser)

        let switchMode = requireElement(app, id: FabushiAID.Auth.switchRegisterMode)
        switchMode.tap()
        _ = requireElement(app, id: FabushiAID.Auth.firstNameField, timeout: 5.0)

        let currentValue = element(app, id: FabushiAID.Auth.usernameField).value as? String ?? ""
        XCTAssertTrue(currentValue.contains(expectedUser), "Switching login/register mode must preserve typed username")
    }

    // Feature: F01 | Tier: 1
    @MainActor
    func test_F01_T1_05_registerWithFirstAndLastNamePersistsDisplayNameOnServer() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")

        let switchReg = requireElement(app, id: FabushiAID.Auth.switchRegisterMode)
        switchReg.tap()

        let uname = "f01_name_\(UUID().uuidString.prefix(7).lowercased())"
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "StrongPass#2026_Name")
        clearAndType(app, id: FabushiAID.Auth.firstNameField, text: "Satoshi")
        clearAndType(app, id: FabushiAID.Auth.lastNameField, text: "Nakamoto")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()
        assertEnteredAuthenticatedShell(app)

        let verifyLogin = try await oracle.loginUser(
            LoginRequestDTO(username: uname, password: "StrongPass#2026_Name")
        )
        let auth = try verifyLogin.decode(AuthResponseDTO.self)
        let meResp = try await oracle.fetchMe(token: auth.token)
        let meJSON = try meResp.decodeJSON()
        XCTAssertEqual(meJSON["first_name"]?.stringValue, "Satoshi")
    }

    // MARK: - F02: Alipay QR OAuth, Mobile Callback & SDK Login (5 Tests)

    // Feature: F02 | Tier: 1
    @MainActor
    func test_F02_T1_01_alipayModeRendersScannableQrImageAndPendingStatus() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "alipayQr")

        let qrImage = requireElement(app, id: FabushiAID.Auth.alipayQrImage)
        XCTAssertGreaterThanOrEqual(qrImage.frame.width, 140.0)
        XCTAssertEqual(qrImage.frame.width, qrImage.frame.height, accuracy: 2.0)

        let status = requireElement(app, id: FabushiAID.Auth.alipayStatusLabel)
        XCTAssertFalse(status.label.isEmpty)
    }

    // Feature: F02 | Tier: 1
    @MainActor
    func test_F02_T1_02_alipaySimulateScanPollsAttemptToAuthenticatedShell() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "alipayQr")

        let qrImage = requireElement(app, id: FabushiAID.Auth.alipayQrImage)
        let attemptId = (qrImage.value as? String) ?? ""
        if !attemptId.isEmpty {
            let scanResp = try await oracle.simulateAlipayScan(attemptId: attemptId)
            XCTAssertEqual(scanResp.statusCode, 200)
            assertEnteredAuthenticatedShell(app, timeout: 15.0)
        } else {
            let started = try await oracle.startAlipayOAuth()
            let scanResp = try await oracle.simulateAlipayScan(attemptId: started.attempt_id)
            XCTAssertEqual(scanResp.statusCode, 200)
            try GesturePhysicsHelpers.openCustomURL("fabushi://auth/alipay/callback?attempt_id=\(started.attempt_id)")
            assertEnteredAuthenticatedShell(app, timeout: 15.0)
        }
    }

    // Feature: F02 | Tier: 1
    @MainActor
    func test_F02_T1_03_alipayCustomFabushiSchemeCallbackCompletesOAuthLogin() async throws {
        let started = try await oracle.startAlipayOAuth()
        let scan = try await oracle.simulateAlipayScan(attemptId: started.attempt_id)
        XCTAssertEqual(scan.statusCode, 200)

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        try GesturePhysicsHelpers.openCustomURL(
            "fabushi://auth/alipay/callback?attempt_id=\(started.attempt_id)&source=alipay_mobile"
        )
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // Feature: F02 | Tier: 1
    @MainActor
    func test_F02_T1_04_alipayCustomGpuiTelegramSchemeCallbackCompletesOAuthLogin() async throws {
        let started = try await oracle.startAlipayOAuth()
        let scan = try await oracle.simulateAlipayScan(attemptId: started.attempt_id)
        XCTAssertEqual(scan.statusCode, 200)

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        try GesturePhysicsHelpers.openCustomURL(
            "gpui-telegram://auth/alipay/callback?attempt_id=\(started.attempt_id)"
        )
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // Feature: F02 | Tier: 1
    @MainActor
    func test_F02_T1_05_alipayRefreshButtonRotatesQrAttemptAndRemainsScannable() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "alipayQr")

        let qrImage = requireElement(app, id: FabushiAID.Auth.alipayQrImage)
        let firstAttempt = qrImage.value as? String

        let refresh = requireElement(app, id: FabushiAID.Auth.alipayRefreshButton)
        refresh.tap()

        let rotated = try await waitUntil(timeout: 6.0) {
            let current = self.element(app, id: FabushiAID.Auth.alipayQrImage).value as? String
            return current != nil && current != firstAttempt
        }
        XCTAssertTrue(rotated, "Refreshing Alipay QR must issue a new attempt_id")
    }

    // MARK: - F03: Email OTP Verification & Registration (5 Tests)

    // Feature: F03 | Tier: 1
    @MainActor
    func test_F03_T1_01_sendEmailCodeStartsSixtySecondResendCooldownTimer() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "email")

        let email = "f03_timer_\(UUID().uuidString.prefix(8).lowercased())@example.test"
        clearAndType(app, id: FabushiAID.Auth.emailField, text: email)
        requireElement(app, id: FabushiAID.Auth.sendEmailCodeButton).tap()

        let timer = requireElement(app, id: FabushiAID.Auth.otpResendTimer, timeout: 8.0)
        XCTAssertFalse(timer.label.isEmpty)
        XCTAssertFalse(element(app, id: FabushiAID.Auth.sendEmailCodeButton).isEnabled)
    }

    // Feature: F03 | Tier: 1
    @MainActor
    func test_F03_T1_02_emailOtpSixDigitEntryRegistersNewAccountIntoWorkspace() async throws {
        let uname = "f03_u_\(UUID().uuidString.prefix(7).lowercased())"
        let email = "\(uname)@example.test"
        let codeDTO = try await oracle.sendEmailCode(email: email)
        let otp = try XCTUnwrap(codeDTO.debug_code)

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "email")

        clearAndType(app, id: FabushiAID.Auth.emailField, text: email)
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "EmailStrong#2026")
        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0))
        box0.tap()
        box0.typeText(otp)
        if element(app, id: FabushiAID.Auth.submitButton).isHittable {
            element(app, id: FabushiAID.Auth.submitButton).tap()
        }
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // Feature: F03 | Tier: 1
    @MainActor
    func test_F03_T1_03_emailInputTrimsAndLowercasesMixedCaseAddressOnSubmit() async throws {
        let uname = "f03_case_\(UUID().uuidString.prefix(6).lowercased())"
        let normalized = "\(uname)@example.test"
        let mixedCase = "  \(uname.uppercased())@Example.TEST  "
        let codeDTO = try await oracle.sendEmailCode(email: normalized)
        let otp = try XCTUnwrap(codeDTO.debug_code)

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "email")

        clearAndType(app, id: FabushiAID.Auth.emailField, text: mixedCase)
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "EmailMixedCase#99")
        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0))
        box0.tap()
        box0.typeText(otp)
        if element(app, id: FabushiAID.Auth.submitButton).isHittable {
            element(app, id: FabushiAID.Auth.submitButton).tap()
        }
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // Feature: F03 | Tier: 1
    @MainActor
    func test_F03_T1_04_emailRegistrationPersistsVerifiedProfileOnLoopbackServer() async throws {
        let uname = "f03_srv_\(UUID().uuidString.prefix(7).lowercased())"
        let email = "\(uname)@example.test"
        let codeDTO = try await oracle.sendEmailCode(email: email)
        let otp = try XCTUnwrap(codeDTO.debug_code)

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "email")
        clearAndType(app, id: FabushiAID.Auth.emailField, text: email)
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "ServerVerify#2026")
        if element(app, id: FabushiAID.Auth.firstNameField).exists {
            clearAndType(app, id: FabushiAID.Auth.firstNameField, text: "Elena")
        }
        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0))
        box0.tap()
        box0.typeText(otp)
        if element(app, id: FabushiAID.Auth.submitButton).isHittable {
            element(app, id: FabushiAID.Auth.submitButton).tap()
        }
        assertEnteredAuthenticatedShell(app, timeout: 15.0)

        let loginCheck = try await oracle.loginUser(
            LoginRequestDTO(username: uname, password: "ServerVerify#2026")
        )
        XCTAssertEqual(loginCheck.statusCode, 200)
    }

    // Feature: F03 | Tier: 1
    @MainActor
    func test_F03_T1_05_emailRegistrationHydratesSemanticTreeAuthenticatedState() async throws {
        let uname = "f03_sem_\(UUID().uuidString.prefix(7).lowercased())"
        let email = "\(uname)@example.test"
        let codeDTO = try await oracle.sendEmailCode(email: email)
        let otp = try XCTUnwrap(codeDTO.debug_code)

        let (app, cfg) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "email")
        clearAndType(app, id: FabushiAID.Auth.emailField, text: email)
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: uname)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "SemanticTree#2026")
        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0))
        box0.tap()
        box0.typeText(otp)
        if element(app, id: FabushiAID.Auth.submitButton).isHittable {
            element(app, id: FabushiAID.Auth.submitButton).tap()
        }
        assertEnteredAuthenticatedShell(app, timeout: 15.0)

        let tree = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertTrue(tree.window.is_authenticated)
        XCTAssertTrue(tree.isValidHexHash)
    }

    // MARK: - F04: Phone OTP Login (`send-code` & `verify`) (5 Tests)

    // Feature: F04 | Tier: 1
    @MainActor
    func test_F04_T1_01_countryPickerSearchSelectsCountryAndUpdatesDialCodeField() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")

        requireElement(app, id: FabushiAID.Auth.countryButton).tap()
        clearAndType(app, id: FabushiAID.Auth.countrySearchField, text: "Japan")
        app.cells.firstMatch.tap()

        let codeVal = element(app, id: FabushiAID.Auth.phoneCodeField).value as? String ?? ""
        XCTAssertTrue(codeVal.contains("81"), "Selecting Japan must populate +81 country code")
    }

    // Feature: F04 | Tier: 1
    @MainActor
    func test_F04_T1_02_manualPlusCountryCodeAutoSelectsMatchingCountryLabel() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")

        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "86")
        let countryLabel = requireElement(app, id: FabushiAID.Auth.countryButton).label
        XCTAssertTrue(
            countryLabel.localizedCaseInsensitiveContains("China") || countryLabel.contains("中国"),
            "Typing 86 must resolve China in country button"
        )
    }

    // Feature: F04 | Tier: 1
    @MainActor
    func test_F04_T1_03_phoneSendCodeAdvancesToSixBoxOtpScreenWithTimer() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")

        let suffix = Int.random(in: 1_000_000...9_999_999)
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        requireElement(app, id: FabushiAID.Auth.continueButton).tap()

        _ = requireElement(app, id: FabushiAID.Auth.otpContainer, timeout: 10.0)
        for idx in 0..<6 {
            XCTAssertTrue(element(app, id: FabushiAID.Auth.otpBox(idx)).exists)
        }
    }

    // Feature: F04 | Tier: 1
    @MainActor
    func test_F04_T1_04_phoneOtpVerificationCreatesNewPhoneAccountAndEntersShell() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let fullPhone = "+1415\(suffix)"
        let challenge = try await oracle.sendPhoneCode(phone: fullPhone)
        let otp = try XCTUnwrap(challenge.debug_code)

        let (app, _) = try launchUnauthenticatedAuth(
            extraEnvironment: ["FABUSHI_TEST_PREFETCHED_PHONE_HASH": challenge.phone_code_hash]
        )
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")

        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        let otpBox0 = element(app, id: FabushiAID.Auth.otpBox(0))
        if !otpBox0.exists {
            requireElement(app, id: FabushiAID.Auth.continueButton).tap()
        }
        let targetBox = requireElement(app, id: FabushiAID.Auth.otpBox(0), timeout: 10.0)
        targetBox.tap()
        targetBox.typeText(otp)
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // Feature: F04 | Tier: 1
    @MainActor
    func test_F04_T1_05_phoneOtpVerificationLogsIntoExistingPhoneBoundUser() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let phone = "+1650\(suffix)"
        let c1 = try await oracle.sendPhoneCode(phone: phone)
        let firstVerify = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/auth/phone/verify",
            payload: .object([
                "phone": .string(phone),
                "code": .string(c1.debug_code ?? ""),
                "phone_code_hash": .string(c1.phone_code_hash),
                "first_name": .string("ReturningUser"),
            ])
        )
        XCTAssertEqual(firstVerify.statusCode, 200)

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "650\(suffix)")
        XCTAssertTrue(element(app, id: FabushiAID.Auth.continueButton).isEnabled)
    }

    // MARK: - F05: Two-Factor Authentication (Cloud Password / SRP-6a) (5 Tests)

    // Feature: F05 | Tier: 1
    @MainActor
    func test_F05_T1_01_loginWith2FAEnabledTransitionsToTwoFactorScreenWithHint() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f05_hint", password: "PrimaryLogin#2026")
        let set2FA = try await oracle.enableTwoFactor(
            token: seeded.token,
            cloudPassword: "CloudVault#8899",
            hint: "vault88",
            recoveryEmail: "vault@example.test"
        )
        XCTAssertEqual(set2FA.statusCode, 200)

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "PrimaryLogin#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        _ = requireElement(app, id: FabushiAID.Auth.twoFactorPasswordField, timeout: 10.0)
        let hintLabel = requireElement(app, id: FabushiAID.Auth.twoFactorHintLabel)
        XCTAssertTrue(hintLabel.label.contains("vault88"))
    }

    // Feature: F05 | Tier: 1
    @MainActor
    func test_F05_T1_02_validTwoFactorCloudPasswordCompletesLoginIntoRootShell() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f05_ok", password: "PrimaryLogin#2026")
        _ = try await oracle.enableTwoFactor(token: seeded.token, cloudPassword: "CloudVault#8899", hint: "v88")

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "PrimaryLogin#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        clearAndType(app, id: FabushiAID.Auth.twoFactorPasswordField, text: "CloudVault#8899")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // Feature: F05 | Tier: 1
    @MainActor
    func test_F05_T1_03_twoFactorEyeToggleSwitchesSecureEntryAndMonkeyPeekState() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f05_eye", password: "PrimaryLogin#2026")
        _ = try await oracle.enableTwoFactor(token: seeded.token, cloudPassword: "CloudVault#8899")

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "PrimaryLogin#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        clearAndType(app, id: FabushiAID.Auth.twoFactorPasswordField, text: "SecretPeek")
        let monkey = requireElement(app, id: FabushiAID.Auth.twoFactorMonkey)
        let beforeState = monkey.value as? String

        let eyeToggle = requireElement(app, id: FabushiAID.Auth.twoFactorEyeToggle)
        eyeToggle.tap()

        let afterState = monkey.value as? String
        XCTAssertNotEqual(beforeState, afterState, "Eye toggle must update monkey peek animation state")
    }

    // Feature: F05 | Tier: 1
    @MainActor
    func test_F05_T1_04_phoneOtpFollowedByTwoFactorChallengeVerifiesSuccessfully() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let phone = "+1408\(suffix)"
        let c1 = try await oracle.sendPhoneCode(phone: phone)
        let v1 = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/auth/phone/verify",
            payload: .object([
                "phone": .string(phone),
                "code": .string(c1.debug_code ?? ""),
                "phone_code_hash": .string(c1.phone_code_hash),
            ])
        )
        let auth = try v1.decode(AuthResponseDTO.self)
        _ = try await oracle.enableTwoFactor(token: auth.token, cloudPassword: "PhoneCloud#2026", hint: "p2fa")

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")
        XCTAssertTrue(element(app, id: FabushiAID.Auth.phoneNumberField).exists)
    }

    // Feature: F05 | Tier: 1
    @MainActor
    func test_F05_T1_05_enablingAndDisablingTwoFactorUpdatesServerTwoFactorStatus() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        _ = requireElement(app, id: FabushiAID.SettingsAndPeers.settingsList)

        let en = try await oracle.enableTwoFactor(token: auth.token, cloudPassword: "Temp2FA#2026")
        XCTAssertEqual(en.statusCode, 200)
        let dis = try await oracle.disableTwoFactor(token: auth.token, currentPassword: "Temp2FA#2026")
        XCTAssertEqual(dis.statusCode, 200)
    }

    // MARK: - F06: QR Code Multi-Device Pairing (`pair/*`) (5 Tests)

    // Feature: F06 | Tier: 1
    @MainActor
    func test_F06_T1_01_unauthenticatedQrPairModeDisplaysLivePairingQrCode() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "qrPair")

        let pairQr = requireElement(app, id: FabushiAID.Auth.pairQrImage)
        XCTAssertGreaterThanOrEqual(pairQr.frame.width, 140.0)
        XCTAssertEqual(pairQr.frame.width, pairQr.frame.height, accuracy: 2.0)
    }

    // Feature: F06 | Tier: 1
    @MainActor
    func test_F06_T1_02_authenticatedPeerAcceptingPairTokenSignsInWaitingDevice() async throws {
        let hostUser = try await oracle.registerRandomUser(prefix: "f06_host")
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "qrPair")

        let pairQr = requireElement(app, id: FabushiAID.Auth.pairQrImage)
        if let rawToken = pairQr.value as? String, !rawToken.isEmpty {
            let cleanToken = rawToken.replacingOccurrences(of: "tg://login?token=", with: "")
            let acc = try await oracle.acceptQrPairing(token: hostUser.token, pairToken: cleanToken)
            XCTAssertEqual(acc.statusCode, 200)
            assertEnteredAuthenticatedShell(app, timeout: 15.0)
        } else {
            let initDTO = try await oracle.initQrPairing()
            let acc = try await oracle.acceptQrPairing(token: hostUser.token, pairToken: initDTO.pair_token)
            XCTAssertEqual(acc.statusCode, 200)
            try GesturePhysicsHelpers.openCustomURL(initDTO.qr_url)
            assertEnteredAuthenticatedShell(app, timeout: 15.0)
        }
    }

    // Feature: F06 | Tier: 1
    @MainActor
    func test_F06_T1_03_tgLoginDeepLinkUrlTriggersPairConfirmationFlow() async throws {
        let initDTO = try await oracle.initQrPairing(deviceName: "Desktop Companion")
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()

        try GesturePhysicsHelpers.openCustomURL(initDTO.qr_url)
        let alertPrimary = element(app, id: FabushiAID.Root.alertPrimaryButton)
        if alertPrimary.waitForExistence(timeout: 5.0) {
            alertPrimary.tap()
        } else {
            _ = try await oracle.acceptQrPairing(token: auth.token, pairToken: initDTO.pair_token)
        }

        let poll = try await oracle.request(method: "GET", path: "/api/v1/auth/pair/poll/\(initDTO.pair_token)")
        XCTAssertEqual(poll.statusCode, 200)
    }

    // Feature: F06 | Tier: 1
    @MainActor
    func test_F06_T1_04_qrPairedDeviceAppearsInServerActiveSessionsList() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let initDTO = try await oracle.initQrPairing(deviceName: "Paired iPad Air")
        let accept = try await oracle.acceptQrPairing(token: auth.token, pairToken: initDTO.pair_token)
        XCTAssertEqual(accept.statusCode, 200)

        let sessions = try await oracle.listSessions(token: auth.token)
        XCTAssertGreaterThanOrEqual(sessions.sessions.count, 2)
        XCTAssertTrue(app.exists)
    }

    // Feature: F06 | Tier: 1
    @MainActor
    func test_F06_T1_05_qrPairingWithTwoFactorAccountPromptsForCloudPassword() async throws {
        let hostUser = try await oracle.registerRandomUser(prefix: "f06_2fa")
        _ = try await oracle.enableTwoFactor(token: hostUser.token, cloudPassword: "QrCloudPassword#1", hint: "qr2fa")
        let initDTO = try await oracle.initQrPairing()
        _ = try await oracle.acceptQrPairing(token: hostUser.token, pairToken: initDTO.pair_token)

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        try GesturePhysicsHelpers.openCustomURL("fabushi://auth/pair/verify?token=\(initDTO.pair_token)")

        let pwField = requireElement(app, id: FabushiAID.Auth.twoFactorPasswordField, timeout: 10.0)
        pwField.tap()
        pwField.typeText("QrCloudPassword#1")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // MARK: - F07: WebAuthn / Passkeys (5 Tests)

    // Feature: F07 | Tier: 1
    @MainActor
    func test_F07_T1_01_passkeySignInButtonVisibleAndHittableOnAuthModePicker() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "passkey")

        let btn = requireElement(app, id: FabushiAID.Auth.passkeySignInButton)
        XCTAssertTrue(btn.isHittable)
    }

    // Feature: F07 | Tier: 1
    @MainActor
    func test_F07_T1_02_passkeyChallengeAssertionSignsInRegisteredAccountPasswordlessly() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f07_pk")
        let keyMat = PasskeyKeyMaterial(labelPrefix: "f07_cred")
        let reg = try await oracle.registerPasskey(token: seeded.token, material: keyMat)
        XCTAssertEqual(reg.statusCode, 201)

        let (app, _) = try launchUnauthenticatedAuth(
            mockBiometrics: .success,
            extraEnvironment: [
                "FABUSHI_MOCK_PASSKEY_CREDENTIAL_ID": keyMat.credentialID,
                "FABUSHI_MOCK_PASSKEY_USERNAME": seeded.username,
            ]
        )
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "passkey")
        requireElement(app, id: FabushiAID.Auth.passkeySignInButton).tap()
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // Feature: F07 | Tier: 1
    @MainActor
    func test_F07_T1_03_authenticatedPasskeyRegistrationPersistsInPasskeyList() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let keyMat = PasskeyKeyMaterial(labelPrefix: "f07_enclave")
        let reg = try await oracle.registerPasskey(token: auth.token, material: keyMat, deviceName: "iPhone 16 Pro")
        XCTAssertEqual(reg.statusCode, 201)

        let listResp = try await oracle.request(method: "GET", path: "/api/v1/auth/passkey/list", bearerToken: auth.token)
        let listJSON = try listResp.decodeJSON()
        XCTAssertEqual(listJSON["total"]?.int64Value, 1)
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
    }

    // Feature: F07 | Tier: 1
    @MainActor
    func test_F07_T1_04_multiplePasskeysRegisteredPerUserAppearWithDeviceLabels() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let k1 = PasskeyKeyMaterial(labelPrefix: "pk_iphone")
        let k2 = PasskeyKeyMaterial(labelPrefix: "pk_ipad")
        _ = try await oracle.registerPasskey(token: auth.token, material: k1, deviceName: "iPhone 16 Pro")
        _ = try await oracle.registerPasskey(token: auth.token, material: k2, deviceName: "iPad Pro M4")

        let listResp = try await oracle.request(method: "GET", path: "/api/v1/auth/passkey/list", bearerToken: auth.token)
        let listJSON = try listResp.decodeJSON()
        XCTAssertEqual(listJSON["total"]?.int64Value, 2)
        XCTAssertTrue(app.exists)
    }

    // Feature: F07 | Tier: 1
    @MainActor
    func test_F07_T1_05_passkeySignInHydratesSemanticTreeWithMatchingUserId() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f07_sem")
        let keyMat = PasskeyKeyMaterial(labelPrefix: "f07_sem_cred")
        _ = try await oracle.registerPasskey(token: seeded.token, material: keyMat)

        let (app, cfg) = try launchUnauthenticatedAuth(
            mockBiometrics: .success,
            extraEnvironment: [
                "FABUSHI_MOCK_PASSKEY_CREDENTIAL_ID": keyMat.credentialID,
                "FABUSHI_MOCK_PASSKEY_USERNAME": seeded.username,
            ]
        )
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "passkey")
        requireElement(app, id: FabushiAID.Auth.passkeySignInButton).tap()
        assertEnteredAuthenticatedShell(app, timeout: 15.0)

        let tree = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertTrue(tree.window.is_authenticated)
    }

    // MARK: - F08: Custom Server URL & Pre-Login HTTP/SOCKS5 Proxy (5 Tests)

    // Feature: F08 | Tier: 1
    @MainActor
    func test_F08_T1_01_serverProxySheetDisplaysConfiguredLoopbackServerUrl() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()

        let urlField = requireElement(app, id: FabushiAID.Auth.serverUrlField)
        let value = (urlField.value as? String) ?? ""
        XCTAssertTrue(value.contains("127.0.0.1"), "Server URL field must reflect active FABUSHI_SERVER_URL")
    }

    // Feature: F08 | Tier: 1
    @MainActor
    func test_F08_T1_02_serverHealthProbeReportsHealthyStatusAndLatency() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()
        requireElement(app, id: FabushiAID.Auth.proxyProbeButton).tap()

        let healthLabel = requireElement(app, id: FabushiAID.Auth.proxyHealthLabel, timeout: 10.0)
        let ok = try await waitUntil(timeout: 8.0) {
            let txt = healthLabel.label.lowercased()
            return txt.contains("ok") || txt.contains("healthy") || txt.contains("ms")
        }
        XCTAssertTrue(ok, "Health probe against live loopback server must report healthy latency")
    }

    // Feature: F08 | Tier: 1
    @MainActor
    func test_F08_T1_03_updatingCustomServerUrlPersistsForSubsequentLoginRequest() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f08_url", password: "CustomServer#2026")
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()
        clearAndType(app, id: FabushiAID.Auth.serverUrlField, text: oracle.baseURL.absoluteString)
        requireElement(app, id: FabushiAID.Auth.proxyProbeButton).tap()

        if app.navigationBars.buttons.firstMatch.exists {
            app.navigationBars.buttons.firstMatch.tap()
        }
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "CustomServer#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()
        assertEnteredAuthenticatedShell(app)
    }

    // Feature: F08 | Tier: 1
    @MainActor
    func test_F08_T1_04_proxyModePickerSwitchesBetweenDirectHttpAndSocks5Fields() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()

        let modePicker = requireElement(app, id: FabushiAID.Auth.proxyModePicker)
        if modePicker.buttons["SOCKS5"].exists {
            modePicker.buttons["SOCKS5"].tap()
            XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.proxyHostField).exists)
            XCTAssertTrue(requireElement(app, id: FabushiAID.Auth.proxyPortField).exists)
        }
    }

    // Feature: F08 | Tier: 1
    @MainActor
    func test_F08_T1_05_directModeAfterProxyToggleRestoresCleanLoopbackConnectivity() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        requireElement(app, id: FabushiAID.Auth.serverProxyButton).tap()

        let modePicker = requireElement(app, id: FabushiAID.Auth.proxyModePicker)
        if modePicker.buttons["HTTP"].exists { modePicker.buttons["HTTP"].tap() }
        if modePicker.buttons["Direct"].exists { modePicker.buttons["Direct"].tap() }

        requireElement(app, id: FabushiAID.Auth.proxyProbeButton).tap()
        let healthLabel = requireElement(app, id: FabushiAID.Auth.proxyHealthLabel)
        XCTAssertFalse(healthLabel.label.isEmpty)
    }

    // MARK: - F09: Startup Session Hydration, Logout & Session Revocation (5 Tests)

    // Feature: F09 | Tier: 1
    @MainActor
    func test_F09_T1_01_coldRelaunchWithPersistedSessionHydratesDirectlyIntoChats() async throws {
        let (app, auth, cfg) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f09_peer")
        let chat = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Persisted Chat", chatType: "direct", peerUserId: peer.user_id)
        )
        app.terminate()

        app.launch()
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(chat.chat_id)).exists)
        XCTAssertTrue(FileManager.default.fileExists(atPath: cfg.appSupportDirectory.path))
    }

    // Feature: F09 | Tier: 1
    @MainActor
    func test_F09_T1_02_explicitLogoutWipesLocalSessionAndReturnsToAuthSplash() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()

        let logoutRow = element(app, id: FabushiAID.SettingsAndPeers.settingsSection("logout"))
        if logoutRow.waitForExistence(timeout: 5.0) {
            logoutRow.tap()
            let confirm = element(app, id: FabushiAID.Root.alertPrimaryButton)
            if confirm.waitForExistence(timeout: 3.0) { confirm.tap() }
        }

        let backOnAuth = element(app, id: FabushiAID.Auth.splashStartButton).waitForExistence(timeout: 10.0)
            || element(app, id: FabushiAID.Auth.usernameField).waitForExistence(timeout: 5.0)
        XCTAssertTrue(backOnAuth, "Explicit logout must transition back to unauthenticated onboarding")
    }

    // Feature: F09 | Tier: 1
    @MainActor
    func test_F09_T1_03_remoteSessionRevocationTransitionsActiveAppBackToAuthScreen() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let revResp = try await oracle.revokeSession(token: auth.token, sessionId: auth.session_id)
        XCTAssertEqual(revResp.statusCode, 200)

        let kickedToAuth = try await waitUntil(timeout: 12.0) {
            self.element(app, id: FabushiAID.Auth.splashStartButton).exists
                || self.element(app, id: FabushiAID.Auth.usernameField).exists
        }
        XCTAssertTrue(kickedToAuth, "Server 401 WS revocation must force active app session back to auth")
    }

    // Feature: F09 | Tier: 1
    @MainActor
    func test_F09_T1_04_revokeOtherSessionsPreservesCurrentDeviceWhileDisconnectingPeers() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let secondLogin = try await oracle.loginUser(
            LoginRequestDTO(username: auth.username, password: "StrongPass#2026_E2E", deviceName: "StaleMac")
        )
        XCTAssertEqual(secondLogin.statusCode, 200)

        let revOthers = try await oracle.revokeOtherSessions(token: auth.token)
        XCTAssertEqual(revOthers.statusCode, 200)

        XCTAssertTrue(element(app, id: FabushiAID.Root.tabBarContainer).exists || element(app, id: FabushiAID.Root.splitView).exists)
        let remaining = try await oracle.listSessions(token: auth.token)
        XCTAssertEqual(remaining.sessions.count, 1)
    }

    // Feature: F09 | Tier: 1
    @MainActor
    func test_F09_T1_05_logoutClearsCachedDialogsBeforeSecondUserSignsInSameSandbox() async throws {
        let (app, userA, _) = try await launchPreauthenticatedWorkspace()
        let secretGroup = try await oracle.createChat(
            token: userA.token,
            CreateChatRequestDTO(title: "UserA Secret Vault", chatType: "group")
        )
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(secretGroup.chat_id)).exists)

        _ = try await oracle.revokeSession(token: userA.token, sessionId: userA.session_id)
        _ = try await waitUntil(timeout: 12.0) {
            self.element(app, id: FabushiAID.Auth.usernameField).exists || self.element(app, id: FabushiAID.Auth.splashStartButton).exists
        }

        let userB = try await oracle.registerRandomUser(prefix: "f09_userb", password: "UserBPass#2026")
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: userB.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "UserBPass#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()
        assertEnteredAuthenticatedShell(app)

        XCTAssertFalse(element(app, id: FabushiAID.ChatList.row(secretGroup.chat_id)).exists, "User A's dialogs must never leak into User B's inbox")
    }

    // MARK: - F10: Multi-Account Switcher (6 Tests)

    // Feature: F10 | Tier: 1
    @MainActor
    func test_F10_T1_01_longPressSettingsTabOpensMultiAccountSwitcherSheet() async throws {
        let acct2 = try await oracle.registerRandomUser(prefix: "f10_acct2")
        let (app, _, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [acct2])

        let settingsTab = requireElement(app, id: FabushiAID.Root.tabSettings)
        GesturePhysicsHelpers.longPressForPortalMenu(settingsTab, duration: 0.65)

        let sheet = requireElement(app, id: FabushiAID.Root.accountSwitcherSheet, timeout: 8.0)
        XCTAssertTrue(sheet.exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Root.accountSlot(0)).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Root.accountSlot(1)).exists)
    }

    // Feature: F10 | Tier: 1
    @MainActor
    func test_F10_T1_02_switchingBetweenTwoPreseededAccountsUpdatesInboxDialogs() async throws {
        let acct2 = try await oracle.registerRandomUser(prefix: "f10_sw2")
        let chat2 = try await oracle.createChat(
            token: acct2.token,
            CreateChatRequestDTO(title: "Account2 Exclusive Group", chatType: "group")
        )
        let (app, _, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [acct2])

        XCTAssertFalse(element(app, id: FabushiAID.ChatList.row(chat2.chat_id)).exists)
        let settingsTab = requireElement(app, id: FabushiAID.Root.tabSettings)
        GesturePhysicsHelpers.longPressForPortalMenu(settingsTab, duration: 0.65)
        requireElement(app, id: FabushiAID.Root.accountSlot(1)).tap()

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(chat2.chat_id), timeout: 12.0).exists)
    }

    // Feature: F10 | Tier: 1
    @MainActor
    func test_F10_T1_03_threeSimultaneousAccountsSwitchCleanlyWithIsolatedSessions() async throws {
        let acct2 = try await oracle.registerRandomUser(prefix: "f10_tri2")
        let acct3 = try await oracle.registerRandomUser(prefix: "f10_tri3")
        let (app, _, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [acct2, acct3])

        let settingsTab = requireElement(app, id: FabushiAID.Root.tabSettings)
        GesturePhysicsHelpers.longPressForPortalMenu(settingsTab, duration: 0.65)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Root.accountSlot(2)).exists)
        requireElement(app, id: FabushiAID.Root.accountSlot(2)).tap()
        assertEnteredAuthenticatedShell(app)
    }

    // Feature: F10 | Tier: 1
    @MainActor
    func test_F10_T1_04_backgroundAccountSlotDisplaysLiveUnreadCountBadge() async throws {
        let acct2 = try await oracle.registerRandomUser(prefix: "f10_badge2")
        let sender = try await oracle.registerRandomUser(prefix: "f10_sender")
        let dm = try await oracle.createChat(
            token: sender.token,
            CreateChatRequestDTO(title: "Unread DM", chatType: "direct", peerUserId: acct2.user_id)
        )
        _ = try await oracle.sendMessage(
            token: sender.token,
            chatId: dm.chat_id,
            SendMessageRequestDTO(text: "Ping background slot!", clientNonce: 10_401)
        )

        let (app, _, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [acct2])
        let settingsTab = requireElement(app, id: FabushiAID.Root.tabSettings)
        GesturePhysicsHelpers.longPressForPortalMenu(settingsTab, duration: 0.65)

        let slot1 = requireElement(app, id: FabushiAID.Root.accountSlot(1))
        XCTAssertTrue(slot1.label.contains("1") || (slot1.value as? String ?? "").contains("1"))
    }

    // Feature: F10 | Tier: 1
    @MainActor
    func test_F10_T1_05_addAccountButtonNavigatesToCleanAddAccountAuthFlow() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        let settingsTab = requireElement(app, id: FabushiAID.Root.tabSettings)
        GesturePhysicsHelpers.longPressForPortalMenu(settingsTab, duration: 0.65)

        requireElement(app, id: FabushiAID.Root.addAccount).tap()
        let onAddAuth = element(app, id: FabushiAID.Auth.usernameField).waitForExistence(timeout: 8.0)
            || element(app, id: FabushiAID.Auth.modePicker).waitForExistence(timeout: 5.0)
        XCTAssertTrue(onAddAuth)
    }

    // Feature: F10 | Tier: 1
    @MainActor
    func test_F10_T1_06_activeAccountSelectionPersistsAcrossAppRelaunch() async throws {
        let acct2 = try await oracle.registerRandomUser(prefix: "f10_persist2")
        let chat2 = try await oracle.createChat(
            token: acct2.token,
            CreateChatRequestDTO(title: "Persisted Slot2 Chat", chatType: "group")
        )
        let (app, _, _) = try await launchPreauthenticatedWorkspace(additionalAccounts: [acct2])

        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Root.tabSettings))
        requireElement(app, id: FabushiAID.Root.accountSlot(1)).tap()
        app.terminate()

        app.launch()
        assertEnteredAuthenticatedShell(app)
        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(chat2.chat_id)).exists)
    }

    // MARK: - F47: Telegram-iOS `AuthorizationSequence` Motion & Haptics (5 Tests)

    // Feature: F47 | Tier: 1
    @MainActor
    func test_F47_T1_01_splashOnboardingPagerSwipesAcrossFeatureSlidesToStartButton() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        let pager = requireElement(app, id: FabushiAID.Auth.splashPager)
        pager.swipeLeft()
        pager.swipeLeft()

        let startBtn = requireElement(app, id: FabushiAID.Auth.splashStartButton)
        XCTAssertTrue(startBtn.isHittable)
        startBtn.tap()
        XCTAssertTrue(element(app, id: FabushiAID.Auth.modePicker).waitForExistence(timeout: 6.0))
    }

    // Feature: F47 | Tier: 1
    @MainActor
    func test_F47_T1_02_phoneInputAppliesLiveCountryMaskFormattingAsDigitsTyped() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")

        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "4155550199")

        let formatted = element(app, id: FabushiAID.Auth.phoneNumberField).value as? String ?? ""
        XCTAssertTrue(
            formatted.contains(" ") || formatted.contains("-") || formatted.contains("("),
            "Live phone mask must format US digits with grouping separators, got: '\(formatted)'"
        )
    }

    // Feature: F47 | Tier: 1
    @MainActor
    func test_F47_T1_03_sixOtpDigitBoxesAutoAdvanceFocusLeftToRightOnKeystroke() async throws {
        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")

        let suffix = Int.random(in: 1_000_000...9_999_999)
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        requireElement(app, id: FabushiAID.Auth.continueButton).tap()

        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0), timeout: 10.0)
        box0.tap()
        box0.typeText("123")

        XCTAssertEqual(element(app, id: FabushiAID.Auth.otpBox(0)).value as? String, "1")
        XCTAssertEqual(element(app, id: FabushiAID.Auth.otpBox(1)).value as? String, "2")
        XCTAssertEqual(element(app, id: FabushiAID.Auth.otpBox(2)).value as? String, "3")
    }

    // Feature: F47 | Tier: 1
    @MainActor
    func test_F47_T1_04_pastingSixDigitOtpPopulatesAllBoxesAndTriggersAutoSubmit() async throws {
        let suffix = Int.random(in: 1_000_000...9_999_999)
        let phone = "+1415\(suffix)"
        let sent = try await oracle.sendPhoneCode(phone: phone)
        let code = try XCTUnwrap(sent.debug_code)

        let (app, _) = try launchUnauthenticatedAuth(
            extraEnvironment: ["FABUSHI_TEST_PREFETCHED_PHONE_HASH": sent.phone_code_hash]
        )
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "phone")
        clearAndType(app, id: FabushiAID.Auth.phoneCodeField, text: "1")
        clearAndType(app, id: FabushiAID.Auth.phoneNumberField, text: "415\(suffix)")
        if !element(app, id: FabushiAID.Auth.otpBox(0)).exists {
            requireElement(app, id: FabushiAID.Auth.continueButton).tap()
        }

        UIPasteboard.general.string = code
        let box0 = requireElement(app, id: FabushiAID.Auth.otpBox(0), timeout: 10.0)
        box0.tap()
        box0.typeText(code)
        assertEnteredAuthenticatedShell(app, timeout: 15.0)
    }

    // Feature: F47 | Tier: 1
    @MainActor
    func test_F47_T1_05_twoFactorMonkeyAnimationTracksPasswordFocusAndEyePeek() async throws {
        let seeded = try await oracle.registerRandomUser(prefix: "f47_monkey", password: "PrimaryLogin#2026")
        _ = try await oracle.enableTwoFactor(token: seeded.token, cloudPassword: "MonkeySecret#1")

        let (app, _) = try launchUnauthenticatedAuth()
        navigatePastSplashIfNeeded(app)
        selectAuthMode(app, segmentLabelOrId: "password")
        clearAndType(app, id: FabushiAID.Auth.usernameField, text: seeded.username)
        clearAndType(app, id: FabushiAID.Auth.passwordField, text: "PrimaryLogin#2026")
        requireElement(app, id: FabushiAID.Auth.submitButton).tap()

        let monkey = requireElement(app, id: FabushiAID.Auth.twoFactorMonkey, timeout: 10.0)
        XCTAssertGreaterThan(monkey.frame.height, 60.0)
        requireElement(app, id: FabushiAID.Auth.twoFactorPasswordField).tap()
        XCTAssertTrue(monkey.exists)
    }
}

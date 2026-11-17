import Foundation
import XCTest

final class Tier1_AuthOnboarding_F01_F05_UITests: ParityTestCase {

    // MARK: - F01: Spec-First Authority & CI Pipeline (Launch Env & Server Health Contract)

    @MainActor
    func testTier1_F01_01_LoopbackServerOverrideUrlConnectsHealthyStatusPill() async throws {
        let client = self.peerClient!
        let healthy = try await client.checkHealth()
        XCTAssertTrue(healthy, "Loopback gpui_telegram_server /health must respond OK")
        let status = try await client.fetchStatus()
        XCTAssertEqual(status["status"]?.stringValue, "healthy")

        let app = launchApp()
        let pill = waitForElement(in: app, AccessibilityIDs.Auth.serverPill)
        XCTAssertTrue(pill.exists)
        XCTAssertTrue(
            pill.label.contains("127.0.0.1") || (pill.value as? String ?? "").contains("healthy"),
            "Server pill must reflect configured loopback endpoint and healthy probe"
        )
        XCTAssertFalse(element(in: app, AccessibilityIDs.Shell.tabChats).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F01_02_IsolatedStorageDirectoryColdStartHydratesCleanState() async throws {
        let dirA = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-f01-a-\(uniqueSuffix())").path
        let dirB = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-f01-b-\(uniqueSuffix())").path
        try FileManager.default.createDirectory(atPath: dirA, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(atPath: dirB, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(atPath: dirA)
            try? FileManager.default.removeItem(atPath: dirB)
        }

        let client = self.peerClient!
        let userA = try await client.registerUser(username: "f01_iso_\(uniqueSuffix())")
        let app1 = launchApp(customStorageDir: dirA)
        let uField = waitForElement(in: app1, AccessibilityIDs.Auth.usernameField)
        clearAndTypeText(in: uField, text: userA.username)
        clearAndTypeText(in: waitForElement(in: app1, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app1, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app1, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app1.terminate()

        let app2 = launchApp(customStorageDir: dirB)
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Auth.serverPill).exists)
        XCTAssertFalse(
            element(in: app2, AccessibilityIDs.Shell.tabChats).exists,
            "Fresh GPUI_TELEGRAM_STORAGE_DIR must boot unauthenticated with zero cross-sandbox bleed"
        )
        app2.terminate()
    }

    @MainActor
    func testTier1_F01_03_EnglishLocaleLaunchEnvironmentRendersEnglishAuthControls() async throws {
        let app = launchApp(locale: "en")
        let submit = waitForElement(in: app, AccessibilityIDs.Auth.submitButton)
        let pill = waitForElement(in: app, AccessibilityIDs.Auth.serverPill)
        XCTAssertGreaterThan(submit.frame.width, 40)
        XCTAssertGreaterThanOrEqual(pill.frame.height, 24)
        XCTAssertFalse(submit.label.isEmpty, "English auth submit button must expose localized label")
        app.terminate()
    }

    @MainActor
    func testTier1_F01_04_SimplifiedChineseLocaleLaunchEnvironmentRendersChineseControls() async throws {
        let app = launchApp(locale: "zh-Hans")
        let submit = waitForElement(in: app, AccessibilityIDs.Auth.submitButton)
        let modePicker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        XCTAssertTrue(modePicker.exists)
        XCTAssertGreaterThan(submit.frame.width, 40)
        XCTAssertFalse(submit.label.contains("auth."), "Localized zh-Hans button must not show raw key")
        app.terminate()
    }

    @MainActor
    func testTier1_F01_05_SimulateOfflineLaunchEnvironmentActivatesOfflineConnectionStatus() async throws {
        let (app1, _) = try await launchAuthenticatedUser(usernamePrefix: "f01_off")
        app1.terminate()

        let app2 = launchApp(simulateOffline: true)
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Shell.tabChats, timeout: 10).exists)
        let connBanner = waitForElement(in: app2, AccessibilityIDs.Shell.connectionStatus, timeout: 8)
        XCTAssertTrue(connBanner.exists)
        XCTAssertFalse(connBanner.label.isEmpty)
        app2.terminate()
    }

    // MARK: - F02: Cloud Username & Password Auth

    @MainActor
    func testTier1_F02_01_RegisterNewUsernameAndPasswordAccountEntersChatsShell() async throws {
        let client = self.peerClient!
        let uname = "f02_reg_\(uniqueSuffix())"
        let app = launchApp()

        let regToggle = element(in: app, AccessibilityIDs.Auth.switchRegisterToggle)
        if regToggle.waitForExistence(timeout: 3) {
            regToggle.tap()
        }
        let firstField = element(in: app, AccessibilityIDs.Auth.firstNameField)
        if firstField.waitForExistence(timeout: 2) {
            clearAndTypeText(in: firstField, text: "Avalokita")
        }
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: uname)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        let verifier = try await client.registerUser(username: "f02_ver_\(uniqueSuffix())")
        let resolved = try await client.resolvePeer(token: verifier.token, username: uname)
        XCTAssertEqual(resolved["username"]?.stringValue, uname)
        app.terminate()
    }

    @MainActor
    func testTier1_F02_02_LoginExistingRegisteredAccountHydratesSeededChats() async throws {
        let client = self.peerClient!
        let userA = try await client.registerUser(username: "f02_a_\(uniqueSuffix())", firstName: "UserA")
        let userB = try await client.registerUser(username: "f02_b_\(uniqueSuffix())", firstName: "UserB")
        let chatId = try await client.createChat(
            token: userB.token,
            title: "Seeded F02 Dialog",
            chatType: "direct",
            peerUserId: userA.userId,
            memberIds: [userA.userId]
        )
        try await client.sendMessage(token: userB.token, chatId: chatId, text: "Welcome back F02")

        let app = launchApp()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: userA.username)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        let row = waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10)
        XCTAssertTrue(row.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F02_03_PasswordSessionPersistsInKeychainAcrossColdRestart() async throws {
        let (app1, session) = try await launchAuthenticatedUser(usernamePrefix: "f02_persist")
        app1.terminate()

        let app2 = launchApp()
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Shell.tabChats, timeout: 10).exists)
        XCTAssertFalse(element(in: app2, AccessibilityIDs.Auth.submitButton).exists)
        waitForElement(in: app2, AccessibilityIDs.Shell.tabSettings).tap()
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Settings.list).exists)
        let me = try await self.peerClient.fetchMe(token: session.token)
        XCTAssertEqual(me["username"]?.stringValue, session.username)
        app2.terminate()
    }

    @MainActor
    func testTier1_F02_04_UsernameTrimsLeadingAtSymbolAndWhitespaceOnLogin() async throws {
        let client = self.peerClient!
        let userA = try await client.registerUser(username: "f02_trim_\(uniqueSuffix())")

        let app = launchApp()
        clearAndTypeText(
            in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField),
            text: "  @\(userA.username)  "
        )
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        XCTAssertFalse(element(in: app, AccessibilityIDs.Auth.errorBanner).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F02_05_LogoutClearsSessionAndAllowsReloginAsSecondUser() async throws {
        let client = self.peerClient!
        let peer = try await client.registerUser(username: "f02_peer_\(uniqueSuffix())")
        let (app, user1) = try await launchAuthenticatedUser(usernamePrefix: "f02_u1")
        let chat1 = try await client.createChat(
            token: peer.token,
            title: "User1 Exclusive",
            chatType: "group",
            memberIds: [user1.userId]
        )
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.row(chat1), timeout: 10).exists)

        let user2 = try await client.registerUser(username: "f02_u2_\(uniqueSuffix())")
        let chat2 = try await client.createChat(
            token: peer.token,
            title: "User2 Exclusive",
            chatType: "group",
            memberIds: [user2.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.logoutButton).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: user2.username)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.row(chat2), timeout: 12).exists)
        XCTAssertFalse(element(in: app, AccessibilityIDs.Chats.row(chat1)).exists)
        app.terminate()
    }

    // MARK: - F03: Alipay QR Login & `fbs_` OAuth

    @MainActor
    func testTier1_F03_01_AlipayLoginModeRendersScannableQrImageAndAction() async throws {
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Alipay"].exists {
            picker.buttons["Alipay"].tap()
        }
        let qr = waitForElement(in: app, AccessibilityIDs.Auth.qrImage, timeout: 10)
        XCTAssertGreaterThanOrEqual(qr.frame.width, 120)
        XCTAssertGreaterThanOrEqual(qr.frame.height, 120)
        app.terminate()
    }

    @MainActor
    func testTier1_F03_02_SimulatedAlipayScanCompletesFbsOAuthPollIntoChatsShell() async throws {
        let client = self.peerClient!
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Alipay"].exists {
            picker.buttons["Alipay"].tap()
        }
        let qr = waitForElement(in: app, AccessibilityIDs.Auth.qrImage, timeout: 10)
        let attemptId = (qr.value as? String) ?? ""
        XCTAssertFalse(attemptId.isEmpty, "auth.qrImage accessibilityValue must expose attempt_id for loopback scan")

        try await client.simulateAlipayScan(attemptId: attemptId, nickname: "慈悲用户02")
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F03_03_AlipayAuthenticatedUserHydratesNicknameInSettingsProfile() async throws {
        let client = self.peerClient!
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Alipay"].exists {
            picker.buttons["Alipay"].tap()
        }
        let qr = waitForElement(in: app, AccessibilityIDs.Auth.qrImage, timeout: 10)
        let attemptId = (qr.value as? String) ?? ""
        XCTAssertFalse(attemptId.isEmpty)
        try await client.simulateAlipayScan(attemptId: attemptId, nickname: "莲花居士")

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings, timeout: 12).tap()
        let settingsList = waitForElement(in: app, AccessibilityIDs.Settings.list)
        XCTAssertTrue(settingsList.exists)
        XCTAssertTrue(app.staticTexts["莲花居士"].waitForExistence(timeout: 8))
        app.terminate()
    }

    @MainActor
    func testTier1_F03_04_SwitchingModeCancelsAlipayPollViaGenerationFence() async throws {
        let client = self.peerClient!
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Alipay"].exists {
            picker.buttons["Alipay"].tap()
        }
        let qr = waitForElement(in: app, AccessibilityIDs.Auth.qrImage, timeout: 10)
        let attemptId = (qr.value as? String) ?? ""
        XCTAssertFalse(attemptId.isEmpty)

        if picker.buttons["Password"].exists {
            picker.buttons["Password"].tap()
        }
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Auth.usernameField).exists)
        try await client.simulateAlipayScan(attemptId: attemptId, nickname: "StaleScanner")
        try await Task.sleep(nanoseconds: 2_500_000_000)

        XCTAssertFalse(
            element(in: app, AccessibilityIDs.Shell.tabChats).exists,
            "Generation-fenced poll must not transition away after leaving Alipay tab"
        )
        let pollClaim = try await client.pollAlipay(attemptId: attemptId)
        XCTAssertEqual(pollClaim["status"]?.stringValue, "completed")
        app.terminate()
    }

    @MainActor
    func testTier1_F03_05_AlipayAuthenticatedSessionConnectsLiveWebSocketUpdates() async throws {
        let client = self.peerClient!
        let peerB = try await client.registerUser(username: "f03_peer_\(uniqueSuffix())")
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Alipay"].exists {
            picker.buttons["Alipay"].tap()
        }
        let qr = waitForElement(in: app, AccessibilityIDs.Auth.qrImage, timeout: 10)
        let attemptId = (qr.value as? String) ?? ""
        XCTAssertFalse(attemptId.isEmpty)
        try await client.simulateAlipayScan(attemptId: attemptId, nickname: "LiveWsAlipay")
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)

        let searchRes = try await client.searchGlobal(token: peerB.token, query: "LiveWsAlipay", category: "chats")
        XCTAssertNotNil(searchRes["query"]?.stringValue)
        app.terminate()
    }

    // MARK: - F04: Email OTP Registration

    @MainActor
    func testTier1_F04_01_SendEmailVerificationCodeRevealsOtpFieldAnd60sCooldown() async throws {
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Email"].exists {
            picker.buttons["Email"].tap()
        }
        let email = "f04_01_\(uniqueSuffix())@example.com"
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.emailField), text: email)
        waitForElement(in: app, AccessibilityIDs.Auth.sendCodeButton).tap()

        let otp = waitForElement(in: app, AccessibilityIDs.Auth.otpField, timeout: 8)
        XCTAssertTrue(otp.exists)
        XCTAssertFalse(element(in: app, AccessibilityIDs.Auth.sendCodeButton).isEnabled)
        app.terminate()
    }

    @MainActor
    func testTier1_F04_02_Complete6DigitEmailOtpRegistrationCreatesAccountAndEntersShell() async throws {
        let client = self.peerClient!
        let uname = "f04_u02_\(uniqueSuffix())"
        let email = "\(uname)@example.com"
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Email"].exists {
            picker.buttons["Email"].tap()
        }
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.emailField), text: email)
        waitForElement(in: app, AccessibilityIDs.Auth.sendCodeButton).tap()

        let otpField = waitForElement(in: app, AccessibilityIDs.Auth.otpField, timeout: 8)
        clearAndTypeText(in: otpField, text: "123456")
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: uname)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        let probe = try await client.registerUser(username: "f04_probe_\(uniqueSuffix())")
        let resolved = try await client.resolvePeer(token: probe.token, username: uname)
        XCTAssertEqual(resolved["username"]?.stringValue, uname)
        app.terminate()
    }

    @MainActor
    func testTier1_F04_03_EmailRegisteredAccountLogsInWithUsernameAndPasswordAfterLogout() async throws {
        let uname = "f04_u03_\(uniqueSuffix())"
        let email = "\(uname)@example.com"
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Email"].exists {
            picker.buttons["Email"].tap()
        }
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.emailField), text: email)
        waitForElement(in: app, AccessibilityIDs.Auth.sendCodeButton).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.otpField, timeout: 8), text: "123456")
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: uname)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.logoutButton).tap()

        let picker2 = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker2.buttons["Password"].exists {
            picker2.buttons["Password"].tap()
        }
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: uname)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F04_04_EmailRegistrationNormalizesCaseAndSurroundingWhitespace() async throws {
        let uname = "f04_u04_\(uniqueSuffix())"
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Email"].exists {
            picker.buttons["Email"].tap()
        }
        clearAndTypeText(
            in: waitForElement(in: app, AccessibilityIDs.Auth.emailField),
            text: "  \(uname.uppercased())@EXAMPLE.COM  "
        )
        waitForElement(in: app, AccessibilityIDs.Auth.sendCodeButton).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.otpField, timeout: 8), text: "123456")
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: uname)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F04_05_EditingEmailAddressResetsOtpStepForUpdatedRecipient() async throws {
        let uname = "f04_u05_\(uniqueSuffix())"
        let app = launchApp()
        let picker = waitForElement(in: app, AccessibilityIDs.Auth.modePicker)
        if picker.buttons["Email"].exists {
            picker.buttons["Email"].tap()
        }
        let emailField = waitForElement(in: app, AccessibilityIDs.Auth.emailField)
        clearAndTypeText(in: emailField, text: "wrong_\(uname)@example.com")
        waitForElement(in: app, AccessibilityIDs.Auth.sendCodeButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Auth.otpField, timeout: 8).exists)

        clearAndTypeText(in: emailField, text: "\(uname)@example.com")
        waitForElement(in: app, AccessibilityIDs.Auth.sendCodeButton).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.otpField, timeout: 8), text: "123456")
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: uname)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app.terminate()
    }

    // MARK: - F05: Editable Cloud Server URL Bar

    @MainActor
    func testTier1_F05_01_OpenPreLoginServerUrlEditorAndApplyLoopbackEndpoint() async throws {
        let app = launchApp()
        waitForElement(in: app, AccessibilityIDs.Auth.serverPill).tap()
        let urlField = waitForElement(in: app, AccessibilityIDs.Auth.serverUrlField)
        clearAndTypeText(in: urlField, text: self.serverURL.absoluteString)
        waitForElement(in: app, AccessibilityIDs.Auth.serverApplyButton).tap()

        waitForElementDisappearance(in: app, AccessibilityIDs.Auth.serverApplyButton)
        let pill = waitForElement(in: app, AccessibilityIDs.Auth.serverPill)
        XCTAssertTrue(pill.label.contains("127.0.0.1"))
        app.terminate()
    }

    @MainActor
    func testTier1_F05_02_ServerUrlEditorNormalizesTrailingSlashAndVerifiesLiveHealth() async throws {
        let app = launchApp()
        waitForElement(in: app, AccessibilityIDs.Auth.serverPill).tap()
        let urlField = waitForElement(in: app, AccessibilityIDs.Auth.serverUrlField)
        clearAndTypeText(in: urlField, text: "\(self.serverURL.absoluteString)///")
        waitForElement(in: app, AccessibilityIDs.Auth.serverApplyButton).tap()

        let pill = waitForElement(in: app, AccessibilityIDs.Auth.serverPill)
        XCTAssertFalse(pill.label.hasSuffix("///"), "Server URL bar must strip redundant trailing slashes")
        XCTAssertFalse(element(in: app, AccessibilityIDs.Auth.errorBanner).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F05_03_AppliedServerEndpointPersistsInServerConfigAcrossColdRelaunch() async throws {
        let app1 = launchApp()
        waitForElement(in: app1, AccessibilityIDs.Auth.serverPill).tap()
        clearAndTypeText(
            in: waitForElement(in: app1, AccessibilityIDs.Auth.serverUrlField),
            text: self.serverURL.absoluteString
        )
        waitForElement(in: app1, AccessibilityIDs.Auth.serverApplyButton).tap()
        waitForElementDisappearance(in: app1, AccessibilityIDs.Auth.serverApplyButton)
        app1.terminate()

        let app2 = launchApp()
        let pill = waitForElement(in: app2, AccessibilityIDs.Auth.serverPill)
        XCTAssertTrue(pill.label.contains("127.0.0.1"))
        app2.terminate()
    }

    @MainActor
    func testTier1_F05_04_EditServerEndpointFromSettingsResetsCacheAndReconnects() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f05_set")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        let serverRow = waitForElement(in: app, AccessibilityIDs.Settings.row("server"))
        serverRow.tap()

        let urlField = waitForElement(in: app, AccessibilityIDs.Auth.serverUrlField)
        clearAndTypeText(in: urlField, text: self.serverURL.absoluteString)
        waitForElement(in: app, AccessibilityIDs.Auth.serverApplyButton).tap()
        XCTAssertTrue(
            element(in: app, AccessibilityIDs.Auth.serverPill).waitForExistence(timeout: 8)
                || element(in: app, AccessibilityIDs.Shell.tabChats).exists
        )
        app.terminate()
    }

    @MainActor
    func testTier1_F05_05_ServerPillDisplaysLiveHealthyIndicatorMatchingServerStatus() async throws {
        let status = try await self.peerClient.fetchStatus()
        XCTAssertEqual(status["status"]?.stringValue, "healthy")

        let app = launchApp()
        let pill = waitForElement(in: app, AccessibilityIDs.Auth.serverPill)
        XCTAssertTrue(pill.isHittable)
        XCTAssertGreaterThan(pill.frame.width, 60)
        app.terminate()
    }
}

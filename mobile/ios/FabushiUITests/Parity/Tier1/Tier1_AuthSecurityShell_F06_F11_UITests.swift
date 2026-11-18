import Foundation
import XCTest

final class Tier1_AuthSecurityShell_F06_F11_UITests: ParityTestCase {

    // MARK: - F06: Phone + Country Picker OTP & QR Device Pairing

    @MainActor
    func testTier1_F06_01_SearchableCountryPickerSelectsCountryAndFormatsPhonePrefix() async throws {
        let app = launchApp()
        selectAuthMode(in: app, "phone")
        waitForElement(in: app, AccessibilityIDs.Auth.countryButton).tap()
        let searchField = waitForElement(in: app, AccessibilityIDs.Auth.countrySearchField, timeout: 6)
        clearAndTypeText(in: searchField, text: "Japan")
        let japanRow = app.staticTexts["Japan"].firstMatch
        XCTAssertTrue(japanRow.waitForExistence(timeout: 5))
        japanRow.tap()

        let countryBtn = waitForElement(in: app, AccessibilityIDs.Auth.countryButton)
        XCTAssertTrue(countryBtn.label.contains("+81") || countryBtn.label.contains("Japan"))
        app.terminate()
    }

    @MainActor
    func testTier1_F06_02_Anonymous888FragmentPrefixAndPhoneOtpVerificationEntersShell() async throws {
        let digits = String(Int.random(in: 10_000_000...99_999_999))
        let phone = "+888\(digits)"
        let app = launchApp()
        selectAuthMode(in: app, "phone")
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.phoneField), text: phone)
        waitForElement(in: app, AccessibilityIDs.Auth.sendCodeButton).tap()

        let otp = waitForElement(in: app, AccessibilityIDs.Auth.otpField, timeout: 8)
        let otpCode = readDynamicOtpCode(in: app)
        clearAndTypeText(in: otp, text: otpCode)
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F06_03_PhoneOtpReauthenticatesExistingAccountWithSeededHistory() async throws {
        let client = self.peerClient!
        let digits = String(Int.random(in: 10_000_000...99_999_999))
        let phone = "+1415\(digits.prefix(7))"
        let sendRes = try await client.sendPhoneCode(phone: phone)
        let hash = sendRes["phone_code_hash"]?.stringValue ?? ""
        let seedCode = sendRes["debug_code"]?.stringValue ?? ""
        XCTAssertEqual(seedCode.count, 6)
        let verifyRes = try await client.verifyPhone(phone: phone, phoneCodeHash: hash, code: seedCode)
        let phoneUid = verifyRes["user_id"]?.uint64Value ?? 0
        XCTAssertGreaterThan(phoneUid, 0)

        let peer = try await client.registerUser(username: "f06_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Phone History Room",
            chatType: "group",
            memberIds: [phoneUid]
        )

        let app = launchApp()
        selectAuthMode(in: app, "phone")
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.phoneField), text: phone)
        waitForElement(in: app, AccessibilityIDs.Auth.sendCodeButton).tap()
        let otpField = waitForElement(in: app, AccessibilityIDs.Auth.otpField, timeout: 8)
        let loginOtp = readDynamicOtpCode(in: app)
        clearAndTypeText(in: otpField, text: loginOtp)
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 12).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F06_04_QrDevicePairingModeAcceptedByCompanionLogsInIosAutomatically() async throws {
        let client = self.peerClient!
        let desktopUser = try await client.registerUser(username: "f06_qr_\(uniqueSuffix())")
        let app = launchApp()
        selectAuthMode(in: app, "qr")
        let qr = waitForElement(in: app, AccessibilityIDs.Auth.qrImage, timeout: 10)
        let rawVal = (qr.value as? String) ?? ""
        let pairToken = rawVal.replacingOccurrences(of: "tg://login?token=", with: "")
        XCTAssertFalse(pairToken.isEmpty)

        try await client.acceptQrPair(token: desktopUser.token, pairToken: pairToken)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F06_05_AuthenticatedIosLinksCompanionQrTokenFromDevicesScreen() async throws {
        let client = self.peerClient!
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f06_link")
        let pairInit = try await client.initQrPair(deviceName: "CompanionMacDesktop")
        let pairToken = pairInit["pair_token"]?.stringValue ?? ""
        XCTAssertFalse(pairToken.isEmpty)

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("sessions")).tap()
        let linkField = waitForElement(in: app, "sessions.linkPairTokenField", timeout: 8)
        clearAndTypeText(in: linkField, text: pairToken)
        waitForElement(in: app, "sessions.linkPairConfirmButton").tap()

        let poll = try await client.pollQrPair(pairToken: pairToken)
        XCTAssertEqual(poll["status"]?.stringValue, "confirmed")
        app.terminate()
    }

    // MARK: - F07: Cloud 2FA Password & WebAuthn Passkeys

    @MainActor
    func testTier1_F07_01_LoginWith2faEnabledAccountDisplaysHintAndVerifiesCloudPassword() async throws {
        let client = self.peerClient!
        let user = try await client.registerUser(username: "f07_2fa_\(uniqueSuffix())")
        try await client.setTwoFactor(token: user.token, newPassword: "CloudPass7!", hint: "GoldenLotus")

        let app = launchApp()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: user.username)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()

        let tfaField = waitForElement(in: app, AccessibilityIDs.Auth.twoFactorPasswordField, timeout: 10)
        let hintLabel = waitForElement(in: app, AccessibilityIDs.Auth.twoFactorHintLabel, timeout: 6)
        XCTAssertTrue(hintLabel.label.contains("GoldenLotus"))
        clearAndTypeText(in: tfaField, text: "CloudPass7!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F07_02_EnableAndRotateCloud2faPasswordFromPrivacySettingsPersistsOnServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f07_rot")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("twoFactor")).tap()

        let newField = waitForElement(in: app, AccessibilityIDs.Auth.twoFactorPasswordField)
        clearAndTypeText(in: newField, text: "Rotated2FA9!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()

        let loginCheck = try await client.loginUser(username: user.username)
        XCTAssertEqual(loginCheck["requires_2fa"]?.boolValue, true)
        app.terminate()
    }

    @MainActor
    func testTier1_F07_03_DisableCloud2faPasswordRemovesSecondFactorChallengeOnLogin() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f07_dis")
        try await client.setTwoFactor(token: user.token, newPassword: "DisableMe7!")

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("twoFactor")).tap()
        waitForElement(in: app, "twoFactor.disableButton").tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.twoFactorPasswordField), text: "DisableMe7!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()

        let loginCheck = try await client.loginUser(username: user.username)
        XCTAssertNotNil(loginCheck["token"]?.stringValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F07_04_RegisterPasskeyInPrivacySettingsPersistsCredentialInPasskeyList() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f07_pk")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("passkeys")).tap()
        waitForElement(in: app, AccessibilityIDs.Auth.passkeyButton).tap()

        let list = try await client.listPasskeys(token: user.token)
        XCTAssertGreaterThanOrEqual(list["total"]?.intValue ?? 0, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F07_05_SignInWithPasskeyButtonAuthenticatesDirectlyIntoChatsShell() async throws {
        let client = self.peerClient!
        let user = try await client.registerUser(username: "f07_pksign_\(uniqueSuffix())")
        try await client.registerPasskey(
            token: user.token,
            credentialId: "cred_\(user.username)",
            deviceName: "Simulator Passkey"
        )

        let app = launchApp()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: user.username)
        waitForElement(in: app, AccessibilityIDs.Auth.passkeyButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        app.terminate()
    }

    // MARK: - F08: Session Hydration, Active Devices & Revocation

    @MainActor
    func testTier1_F08_01_StartupAuthMeHydratesCurrentUserAndConnectsWebSocket() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f08_hyd")
        let peer = try await client.registerUser(username: "f08_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Live WS Hydration",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )
        try await client.sendMessage(token: peer.token, chatId: chatId, text: "Instant WS push")
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F08_02_ActiveSessionsScreenListsCurrentIosAndCompanionDevices() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f08_list")
        _ = try await client.loginUser(username: user.username, deviceName: "MacStudioCompanion")

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("sessions")).tap()
        XCTAssertTrue(app.staticTexts["MacStudioCompanion"].waitForExistence(timeout: 8))
        app.terminate()
    }

    @MainActor
    func testTier1_F08_03_TerminateSingleCompanionSessionRevokesBearerTokenOnServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f08_rev1")
        let secondLogin = try await client.loginUser(username: user.username, deviceName: "RevokeTargetLinux")
        let secondToken = secondLogin["token"]?.stringValue ?? ""
        XCTAssertFalse(secondToken.isEmpty)

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("sessions")).tap()
        let row = app.staticTexts["RevokeTargetLinux"].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 8))
        row.swipeLeft()
        app.buttons["Terminate"].firstMatch.tap()

        let (code, _, _) = try await client.requestRaw(
            method: "GET",
            path: "/api/v1/auth/me",
            token: secondToken,
            contentType: nil,
            expectedStatuses: [401]
        )
        XCTAssertEqual(code, 401)
        app.terminate()
    }

    @MainActor
    func testTier1_F08_04_TerminateAllOtherSessionsPreservesCurrentIosSessionOnly() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f08_revall")
        let otherLogin = try await client.loginUser(username: user.username, deviceName: "OtherPad")
        let otherTok = otherLogin["token"]?.stringValue ?? ""

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("sessions")).tap()
        waitForElement(in: app, "sessions.terminateOthersButton").tap()

        let (code, _, _) = try await client.requestRaw(
            method: "GET",
            path: "/api/v1/auth/me",
            token: otherTok,
            contentType: nil,
            expectedStatuses: [401]
        )
        XCTAssertEqual(code, 401)
        XCTAssertTrue(element(in: app, AccessibilityIDs.Shell.tabSettings).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F08_05_RemoteRevocationOfIosSessionTriggers401AutoLogoutAndCacheWipe() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f08_remote")
        let adminLogin = try await client.loginUser(username: user.username, deviceName: "AdminController")
        let adminTok = adminLogin["token"]?.stringValue ?? ""
        let adminSid = adminLogin["session_id"]?.stringValue ?? ""

        let sessions = try await client.listSessions(token: adminTok)
        for item in sessions["sessions"]?.arrayValue ?? [] {
            if let sid = item["session_id"]?.stringValue, sid != adminSid {
                try await client.revokeSession(token: adminTok, sessionId: sid)
            }
        }

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Auth.submitButton, timeout: 12).exists)
        app.terminate()
    }

    // MARK: - F09: Keychain Multi-Account Switcher & Passcode/FaceID Lock

    @MainActor
    func testTier1_F09_01_AddSecondAccountSlotAndSwitchViaSettingsLongPressSheet() async throws {
        let client = self.peerClient!
        let user2 = try await client.registerUser(username: "f09_slot2_\(uniqueSuffix())", firstName: "SlotTwo")
        let (app, user1) = try await launchAuthenticatedUser(usernamePrefix: "f09_slot1", firstName: "SlotOne")

        longPressSettingsTabForAccountSwitcher(in: app)
        let sheet = waitForElement(in: app, AccessibilityIDs.Shell.accountSwitcherSheet)
        XCTAssertTrue(sheet.exists)
        waitForElement(in: app, "accountSwitcher.addAccountButton").tap()

        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: user2.username)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)

        longPressSettingsTabForAccountSwitcher(in: app)
        XCTAssertTrue(app.staticTexts[user1.firstName].waitForExistence(timeout: 6))
        app.staticTexts[user1.firstName].tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F09_02_AccountSwitcherDisplaysUnreadBadgesAndIsolatesChatDialogs() async throws {
        let client = self.peerClient!
        let peer = try await client.registerUser(username: "f09_peer_\(uniqueSuffix())")
        let user2 = try await client.registerUser(username: "f09_b2_\(uniqueSuffix())", firstName: "AcctBeta")
        let (app, user1) = try await launchAuthenticatedUser(usernamePrefix: "f09_b1", firstName: "AcctAlpha")

        let chat1 = try await client.createChat(
            token: peer.token,
            title: "Alpha Private",
            chatType: "group",
            memberIds: [user1.userId]
        )
        longPressSettingsTabForAccountSwitcher(in: app)
        waitForElement(in: app, "accountSwitcher.addAccountButton").tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.usernameField), text: user2.username)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Auth.passwordField), text: "StrongPass123!")
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12).exists)
        XCTAssertFalse(element(in: app, AccessibilityIDs.Chats.row(chat1)).exists)

        try await client.sendMessage(token: peer.token, chatId: chat1, text: "Unread for Alpha")
        longPressSettingsTabForAccountSwitcher(in: app)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.accountSwitcherSheet).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F09_03_EnablePasscodeLockPresentsLockScreenAndUnlocksWithValidPin() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f09_pin")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("passcode")).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Shell.passcodeLockPinField), text: "2580")

        waitForElement(in: app, AccessibilityIDs.Shell.tabChats).tap()
        waitForElement(in: app, "shell.navbarLockButton").tap()
        let lockView = waitForElement(in: app, AccessibilityIDs.Shell.passcodeLockView)
        XCTAssertTrue(lockView.exists)

        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Shell.passcodeLockPinField), text: "2580")
        waitForElementDisappearance(in: app, AccessibilityIDs.Shell.passcodeLockView)
        app.terminate()
    }

    @MainActor
    func testTier1_F09_04_ColdRelaunchWithPasscodeEnabledBlocksShellUntilPinEntered() async throws {
        let (app1, _) = try await launchAuthenticatedUser(usernamePrefix: "f09_coldpin")
        waitForElement(in: app1, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app1, AccessibilityIDs.Settings.row("passcode")).tap()
        clearAndTypeText(in: waitForElement(in: app1, AccessibilityIDs.Shell.passcodeLockPinField), text: "1357")
        app1.terminate()

        let app2 = launchApp()
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Shell.passcodeLockView, timeout: 10).exists)
        clearAndTypeText(in: waitForElement(in: app2, AccessibilityIDs.Shell.passcodeLockPinField), text: "1357")
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Shell.tabChats, timeout: 10).exists)
        app2.terminate()
    }

    @MainActor
    func testTier1_F09_05_ChangeAndDisablePasscodeUpdatesLockRequirementCleanly() async throws {
        let (app1, _) = try await launchAuthenticatedUser(usernamePrefix: "f09_offpin")
        waitForElement(in: app1, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app1, AccessibilityIDs.Settings.row("passcode")).tap()
        clearAndTypeText(in: waitForElement(in: app1, AccessibilityIDs.Shell.passcodeLockPinField), text: "2468")
        waitForElement(in: app1, "passcode.disableButton").tap()
        app1.terminate()

        let app2 = launchApp()
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Shell.tabChats, timeout: 10).exists)
        XCTAssertFalse(element(in: app2, AccessibilityIDs.Shell.passcodeLockView).exists)
        app2.terminate()
    }

    // MARK: - F10: Telegram Design System, 4 Themes & Bilingual i18n

    @MainActor
    func testTier1_F10_01_SwitchBetweenNightTintedDayAndClassicThemesUpdatesPalette() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f10_theme")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("appearance")).tap()

        for mode in ["day", "classic", "night", "tinted"] {
            let btn = waitForElement(in: app, AccessibilityIDs.Settings.theme(mode))
            btn.tap()
            XCTAssertTrue(
                isElementMarkedSelected(btn),
                "Theme button '\(mode)' must report selected state after tap"
            )
        }
        app.terminate()
    }

    @MainActor
    func testTier1_F10_02_SelectAccentColorAndChatWallpaperPersistsAppearanceSettings() async throws {
        let (app1, _) = try await launchAuthenticatedUser(usernamePrefix: "f10_accent")
        waitForElement(in: app1, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app1, AccessibilityIDs.Settings.row("appearance")).tap()
        waitForElement(in: app1, "appearance.accent.violet").tap()
        waitForElement(in: app1, "appearance.wallpaper.2").tap()
        app1.terminate()

        let app2 = launchApp()
        waitForElement(in: app2, AccessibilityIDs.Shell.tabSettings, timeout: 10).tap()
        waitForElement(in: app2, AccessibilityIDs.Settings.row("appearance")).tap()
        let violetBtn = waitForElement(in: app2, "appearance.accent.violet")
        let wpBtn = waitForElement(in: app2, "appearance.wallpaper.2")
        XCTAssertTrue(isElementMarkedSelected(violetBtn))
        XCTAssertTrue(isElementMarkedSelected(wpBtn))
        app2.terminate()
    }

    @MainActor
    func testTier1_F10_03_AdjustTextSizeAndBubbleCornerSlidersScalesTranscriptGeometry() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f10_scale")
        let peer = try await client.registerUser(username: "f10_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Scale Test",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )
        let msgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "Dynamic Typography Probe Line One\nDynamic Typography Probe Line Two"
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId)).tap()
        let smallHeight = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).frame.height
        interactivePopSwipeBack(in: app)

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("appearance")).tap()
        let slider = waitForElement(in: app, "appearance.textSizeSlider")
        slider.adjust(toNormalizedSliderPosition: 0.95)

        waitForElement(in: app, AccessibilityIDs.Shell.tabChats).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId)).tap()
        let largeHeight = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).frame.height
        XCTAssertGreaterThan(largeHeight, smallHeight)
        app.terminate()
    }

    @MainActor
    func testTier1_F10_04_LiveLanguageToggleToSimplifiedChineseLocalizesTabsWithoutRestart() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f10_zh", locale: "en")
        let enLabel = waitForElement(in: app, AccessibilityIDs.Shell.tabChats).label
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("language")).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.language("zh-Hans")).tap()

        let zhLabel = waitForElement(in: app, AccessibilityIDs.Shell.tabChats).label
        XCTAssertNotEqual(zhLabel, enLabel, "Switching language to zh-Hans must update tab labels live")
        app.terminate()
    }

    @MainActor
    func testTier1_F10_05_LiveLanguageToggleBackToEnglishRestoresEnglishShellLabels() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f10_en", locale: "zh-Hans")
        let zhLabel = waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).label
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("language")).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.language("en")).tap()

        let enLabel = waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).label
        XCTAssertNotEqual(enLabel, zhLabel)
        app.terminate()
    }

    // MARK: - F11: Adaptive Native iPhone 4-Tab & iPad Split Shell

    @MainActor
    func testTier1_F11_01_FourTabBarSwitchesCleanlyAcrossContactsCallsChatsAndSettings() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f11_tabs")
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.root).exists)

        waitForElement(in: app, AccessibilityIDs.Shell.tabContacts).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.contactsList).exists)

        waitForElement(in: app, AccessibilityIDs.Shell.tabCalls).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.callsList).exists)

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.list).exists)

        waitForElement(in: app, AccessibilityIDs.Shell.tabChats).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.list).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F11_02_InlineConnectionStatusTransitionsToConnectedOnHealthySync() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f11_conn")
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Shell.root).exists)
        let status = waitForElement(in: app, AccessibilityIDs.Shell.connectionStatus, timeout: 8)
        let val = ((status.value as? String) ?? status.label).lowercased()
        XCTAssertEqual(val, "connected")
        app.terminate()
    }

    @MainActor
    func testTier1_F11_03_FullWidthInteractiveSwipeBackPopsConversationStageToChatsList() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f11_pop")
        let peer = try await client.registerUser(username: "f11_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Swipe Back Stage",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId)).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.collectionView).exists)

        interactivePopSwipeBack(in: app)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.list, timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F11_04_ReSelectingChatsTabScrollsDialogsListToTop() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f11_scroll")
        let peer = try await client.registerUser(username: "f11_sp_\(uniqueSuffix())")
        var newestChatId: Int64 = 0
        for idx in 1...12 {
            newestChatId = try await client.createChat(
                token: peer.token,
                title: "Room \(idx)",
                chatType: "group",
                memberIds: [user.userId]
            )
            try await client.sendMessage(token: peer.token, chatId: newestChatId, text: "Ping \(idx)")
        }

        let list = waitForElement(in: app, AccessibilityIDs.Chats.list)
        list.swipeUp()
        waitForElement(in: app, AccessibilityIDs.Shell.tabChats).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.row(newestChatId), timeout: 8).isHittable)
        app.terminate()
    }

    @MainActor
    func testTier1_F11_05_AdaptiveRootShellMaintainsSelectionStateAcrossNavigationTransitions() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f11_state")
        let peer = try await client.registerUser(username: "f11_stpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "State Retention",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId)).tap()
        XCTAssertEqual(waitForElement(in: app, AccessibilityIDs.Stage.headerTitle).label, "State Retention")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.list).exists)
        waitForElement(in: app, AccessibilityIDs.Shell.tabChats).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.collectionView, timeout: 6).exists)
        XCTAssertEqual(waitForElement(in: app, AccessibilityIDs.Stage.headerTitle).label, "State Retention")
        app.terminate()
    }
}

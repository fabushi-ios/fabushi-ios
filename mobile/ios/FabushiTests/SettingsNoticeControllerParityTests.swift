import XCTest
@testable import Fabushi

@MainActor
final class SettingsNoticeControllerParityTests: XCTestCase {
    private func event(
        _ kind: SurfaceNoticeKind = .error,
        _ operation: PluginsNoticeOperation = .install,
        _ message: String = "notice"
    ) -> RootSettingsNoticeEvent {
        RootSettingsNoticeEvent(
            kind: kind,
            operation: .plugins(operation),
            message: message
        )
    }

    func testRootControllerPublishesReplacesResetsAndDisposes() {
        let controller = SettingsNoticeController()
        controller.updateScope(accountKey: "account-a", surface: .plugins)

        controller.publish(event(.error, .install, "first"))
        let firstRevision = controller.snapshot?.revision
        XCTAssertEqual(controller.snapshot?.event, event(.error, .install, "first"))

        controller.publish(event(.error, .install, "first"))
        XCTAssertNotEqual(controller.snapshot?.revision, firstRevision)

        controller.publish(event(.success, .install, "second"))
        XCTAssertEqual(controller.snapshot?.event, event(.success, .install, "second"))

        controller.reset()
        XCTAssertNil(controller.snapshot)

        controller.publish(event(.success, .load, "after-reset"))
        XCTAssertNotNil(controller.snapshot)
        controller.dispose()
        XCTAssertNil(controller.snapshot)

        controller.publish(event(.error, .load, "ignored"))
        XCTAssertNil(controller.snapshot)
    }

    func testAccountOrSurfaceScopeChangeClearsAndRejectsStaleAsyncCompletion() {
        let controller = SettingsNoticeController()
        controller.updateScope(accountKey: "account-a", surface: .plugins)
        let staleFence = controller.makeFence()
        controller.publish(event(.success, .install, "installed"), fence: staleFence)
        XCTAssertNotNil(controller.snapshot)

        controller.updateScope(accountKey: "account-b", surface: .plugins)
        XCTAssertNil(controller.snapshot)
        controller.publish(event(.error, .install, "late"), fence: staleFence)
        XCTAssertNil(controller.snapshot)

        let currentFence = controller.makeFence()
        controller.updateScope(accountKey: "account-b", surface: .settings)
        controller.publish(event(.error, .install, "wrong-surface"), fence: currentFence)
        XCTAssertNil(controller.snapshot)
    }

    func testSameScopeKeepsFenceValid() {
        let controller = SettingsNoticeController()
        controller.updateScope(accountKey: "account-a", surface: .plugins)
        let fence = controller.makeFence()
        controller.updateScope(accountKey: "account-a", surface: .plugins)
        controller.publish(event(.success, .load, "ok"), fence: fence)
        XCTAssertEqual(controller.snapshot?.event.message, "ok")
    }

    func testPresenterExpiryMatchesDesktopContract() {
        XCTAssertEqual(
            SettingsNoticePresentationPolicy.dismissDelayMilliseconds(for: .success),
            3_500
        )
        XCTAssertEqual(
            SettingsNoticePresentationPolicy.dismissDelayMilliseconds(for: .error),
            6_000
        )
    }

    func testPresenterMapsAssertiveErrorsAheadOfPoliteSuccessAnnouncements() {
        XCTAssertEqual(
            SettingsNoticePresentationPolicy
                .accessibilityAnnouncementDelayMilliseconds(for: .error),
            0
        )
        XCTAssertGreaterThan(
            SettingsNoticePresentationPolicy
                .accessibilityAnnouncementDelayMilliseconds(for: .success),
            0
        )
        XCTAssertLessThan(
            SettingsNoticePresentationPolicy
                .accessibilityAnnouncementDelayMilliseconds(for: .success),
            SettingsNoticePresentationPolicy.dismissDelayMilliseconds(for: .success)
        )
    }

    func testPublisherFeedsTypedRootAndLegacyStringSink() {
        let controller = SettingsNoticeController()
        controller.updateScope(accountKey: "account-a", surface: .plugins)
        let fence = controller.makeFence()
        var legacyStatus: String?

        SurfaceNoticePublisher.publish(
            event(.success, .install, "installed"),
            controller: controller,
            fence: fence,
            legacyStatus: { legacyStatus = $0 }
        )

        XCTAssertEqual(controller.snapshot?.event, event(.success, .install, "installed"))
        XCTAssertEqual(legacyStatus, "installed")
    }

    func testSettingsFactoryPreservesTypedUiPreferencesAndCallMediaOperations() {
        let ui = SettingsNoticeEventFactory.settings(
            .success,
            operation: .uiPreferences,
            message: "updated"
        )
        XCTAssertEqual(ui.kind, .success)
        XCTAssertEqual(ui.operation.rawValue, "settings-ui-preferences")

        let media = SettingsNoticeEventFactory.settings(
            .error,
            operation: .callMedia,
            message: "denied"
        )
        XCTAssertEqual(media.kind, .error)
        XCTAssertEqual(media.operation.rawValue, "settings-call-media")
    }

    func testTypedOperationSetsMatchDesktopSurfaceNoticeContract() {
        XCTAssertEqual(
            Set(SettingsNoticeOperation.allCases.map(\.rawValue)),
            Set([
                "settings-load",
                "settings-account",
                "settings-auto-review",
                "settings-theme",
                "settings-local-tool-permission",
                "settings-security-key",
                "settings-time-zone",
                "settings-ui-preferences",
                "settings-call-media",
                "settings-router-provider",
                "settings-usage-cancel-trial",
                "settings-update-check",
                "settings-update-install",
                "settings-update-auto-update-when-idle",
                "settings-update-track",
            ])
        )
        XCTAssertEqual(
            Set(PluginsNoticeOperation.allCases.map(\.rawValue)),
            Set([
                "plugins-load",
                "plugins-private-skills-load",
                "plugins-private-skill-delete",
                "plugins-private-skill-update",
                "plugins-private-skill-toggle",
                "plugins-private-skill-sync",
                "plugins-authenticate",
                "plugins-browser-remove",
                "plugins-account-rename",
                "plugins-account-remove",
                "plugins-install",
                "plugins-edit-setup",
                "plugins-remove",
                "plugins-server-tools-load",
                "plugins-server-tool-toggle",
            ])
        )
    }
}

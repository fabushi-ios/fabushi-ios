import XCTest
@testable import Fabushi

@MainActor
final class MobileAppAlertParityTests: XCTestCase {
    func testConfirmCancelQueueAndFailureSettleDeterministically() async {
        let controller = MobileAppAlertController()

        let first = Task {
            await controller.alert(.init(
                title: "Delete conversation",
                description: nil,
                body: nil,
                warning: nil,
                confirmLabel: "Delete",
                pendingLabel: nil,
                cancelLabel: "Cancel",
                destructive: true,
                secondary: nil,
                wide: false,
                perform: { nil }
            ))
        }
        await Task.yield()
        XCTAssertEqual(controller.state?.request.title, "Delete conversation")

        let rejected = await controller.alert(.init(
            title: "Should not queue",
            description: nil,
            body: nil,
            warning: nil,
            confirmLabel: "OK",
            pendingLabel: nil,
            cancelLabel: "Cancel",
            destructive: false,
            secondary: nil,
            wide: false,
            perform: nil
        ))
        XCTAssertFalse(rejected)

        controller.confirm()
        await Task.yield()
        XCTAssertTrue(await first.value)
        XCTAssertNil(controller.state)

        let active = Task {
            await controller.alert(.init(
                title: "First",
                description: nil,
                body: nil,
                warning: nil,
                confirmLabel: "Continue",
                pendingLabel: nil,
                cancelLabel: nil,
                destructive: false,
                secondary: nil,
                wide: false,
                perform: nil
            ))
        }
        await Task.yield()
        let queued = Task {
            await controller.alert(.init(
                title: "Second",
                description: nil,
                body: nil,
                warning: nil,
                confirmLabel: "Continue",
                pendingLabel: nil,
                cancelLabel: nil,
                destructive: false,
                secondary: nil,
                wide: false,
                perform: nil
            ))
        }
        await Task.yield()
        let third = await controller.alert(.init(
            title: "Third",
            description: nil,
            body: nil,
            warning: nil,
            confirmLabel: "Continue",
            pendingLabel: nil,
            cancelLabel: nil,
            destructive: false,
            secondary: nil,
            wide: false,
            perform: nil
        ))
        XCTAssertFalse(third)

        controller.confirm()
        XCTAssertTrue(await active.value)
        XCTAssertEqual(controller.state?.request.title, "Second")
        controller.cancel()
        XCTAssertFalse(await queued.value)

        let failed = Task {
            await controller.alert(.init(
                title: "Retryable",
                description: nil,
                body: nil,
                warning: nil,
                confirmLabel: "Retry",
                pendingLabel: "Working",
                cancelLabel: "Cancel",
                destructive: false,
                secondary: nil,
                wide: false,
                perform: { "provider unavailable" }
            ))
        }
        await Task.yield()
        controller.confirm()
        XCTAssertTrue(controller.state?.isPerforming == true)
        await Task.yield()
        XCTAssertEqual(controller.state?.failure, "provider unavailable")
        controller.cancel()
        XCTAssertFalse(await failed.value)
    }

    func testDisposeRejectsActiveQueuedAndFutureAlerts() async {
        let controller = MobileAppAlertController()
        let active = Task {
            await controller.alert(.init(
                title: "A", description: nil, body: nil, warning: nil,
                confirmLabel: "A", pendingLabel: nil, cancelLabel: nil,
                destructive: false, secondary: nil, wide: false, perform: nil
            ))
        }
        await Task.yield()
        let queued = Task {
            await controller.alert(.init(
                title: "B", description: nil, body: nil, warning: nil,
                confirmLabel: "B", pendingLabel: nil, cancelLabel: nil,
                destructive: false, secondary: nil, wide: false, perform: nil
            ))
        }
        await Task.yield()
        controller.dispose()
        XCTAssertFalse(await active.value)
        XCTAssertFalse(await queued.value)

        let future = await controller.alert(.init(
            title: "C", description: nil, body: nil, warning: nil,
            confirmLabel: "C", pendingLabel: nil, cancelLabel: nil,
            destructive: false, secondary: nil, wide: false, perform: nil
        ))
        XCTAssertFalse(future)
    }
}

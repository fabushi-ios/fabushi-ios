import XCTest
@testable import Fabushi

@MainActor
final class HostSettingsFieldParityTests: XCTestCase {
    private struct Settings: Equatable {
        var value: Bool?
    }

    private enum TestError: Error {
        case transportDown
    }

    func testWriteEpochMarksSnapshotsStaleAcrossWriteAndSettlement() {
        let writes = WriteEpoch()
        let before = writes.snapshot()
        let settle = writes.begin()

        XCTAssertTrue(writes.isStale(before))

        settle()
        XCTAssertTrue(writes.isStale(before))

        let after = writes.snapshot()
        XCTAssertFalse(writes.isStale(after))

        settle()
        XCTAssertFalse(writes.isStale(after))
    }

    func testWriteEpochResetInvalidatesOldSettlementClosure() {
        let writes = WriteEpoch()
        let settle = writes.begin()
        writes.reset()
        let afterReset = writes.snapshot()

        settle()

        XCTAssertFalse(writes.isStale(afterReset))
    }

    func testBoxSettingsFieldExplicitRemoteValueRepaintsLocalMirror() async {
        var readable = true
        var remote = Settings(value: false)
        var local: Bool? = true
        var pushes: [Bool] = []

        let field = BoxSettingsField<Settings, Bool>(
            port: HostSettingsPort(
                isReadable: { readable },
                read: { remote },
                write: { value in
                    pushes.append(value)
                    remote.value = value
                    return remote
                },
                value: { $0.value }
            ),
            mirror: HostSettingsMirror(
                read: { local },
                write: { local = $0 },
                clear: { local = nil }
            )
        )

        XCTAssertEqual(await field.absorbFromBox(), .repainted)
        XCTAssertEqual(local, false)
        XCTAssertTrue(pushes.isEmpty)
        XCTAssertTrue(readable)
    }

    func testBoxSettingsFieldWritesLocalAnswerBackAfterTransportReturnsToUnwrittenRemote() async {
        var readable = false
        var remote = Settings(value: nil)
        var local: Bool?
        var pushes: [Bool] = []

        let field = BoxSettingsField<Settings, Bool>(
            port: HostSettingsPort(
                isReadable: { readable },
                read: {
                    guard readable else { throw TestError.transportDown }
                    return remote
                },
                write: { value in
                    guard readable else { throw TestError.transportDown }
                    pushes.append(value)
                    remote.value = value
                    return remote
                },
                value: { $0.value }
            ),
            mirror: HostSettingsMirror(
                read: { local },
                write: { local = $0 },
                clear: { local = nil }
            )
        )

        XCTAssertEqual(await field.apply(true), .unreachable)
        XCTAssertEqual(local, true)
        XCTAssertTrue(pushes.isEmpty)

        readable = true
        XCTAssertEqual(await field.absorbFromBox(), .none)

        XCTAssertEqual(local, true)
        XCTAssertEqual(remote.value, true)
        XCTAssertEqual(pushes, [true])
    }

    func testBoxSettingsFieldClearsOldLocalMirrorWhenRemoteIsUnwrittenAndSessionHasNoAnswer() async {
        var remote = Settings(value: nil)
        var local: Bool? = true

        let field = BoxSettingsField<Settings, Bool>(
            port: HostSettingsPort(
                isReadable: { true },
                read: { remote },
                write: { value in
                    remote.value = value
                    return remote
                },
                value: { $0.value }
            ),
            mirror: HostSettingsMirror(
                read: { local },
                write: { local = $0 },
                clear: { local = nil }
            )
        )

        XCTAssertEqual(await field.absorbFromBox(), .cleared)
        XCTAssertNil(local)
    }
}

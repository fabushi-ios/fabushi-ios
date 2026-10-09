import AVFoundation
import Foundation
import XCTest
@testable import Fabushi

final class HumanCallMediaPortTests: XCTestCase {
    @MainActor
    func testPermissionMappingMatchesDesktopCallMediaContract() {
        XCTAssertEqual(HumanCallMediaPort.permission(for: .authorized), .granted)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .denied), .denied)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .restricted), .denied)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .notDetermined), .prompt)
    }

    func testMediaPermissionWireValuesStayStable() {
        XCTAssertEqual(HumanCallMediaPermission.granted.rawValue, "granted")
        XCTAssertEqual(HumanCallMediaPermission.denied.rawValue, "denied")
        XCTAssertEqual(HumanCallMediaPermission.prompt.rawValue, "prompt")
        XCTAssertEqual(HumanCallMediaPermission.notRequested.rawValue, "not-requested")
    }
    @MainActor
    func testStoredMediaPreferenceKeepsSystemDefaultDistinctFromRuntimeFallback() {
        let suiteName = "HumanCallMediaPortTests.media-preferences"
        guard let defaults = UserDefaults(suiteName: suiteName) else {
            XCTFail("Unable to create isolated UserDefaults suite")
            return
        }
        defaults.removePersistentDomain(forName: suiteName)
        defer { defaults.removePersistentDomain(forName: suiteName) }

        let port = HumanCallMediaPort(defaults: defaults)
        XCTAssertNil(port.storedPreferences().microphoneId)
        XCTAssertNil(port.storedPreferences().cameraId)

        port.setPreferredDeviceId("mic-test", kind: .microphone)
        port.setPreferredDeviceId("camera-test", kind: .camera)
        XCTAssertEqual(port.storedPreferences().microphoneId, "mic-test")
        XCTAssertEqual(port.storedPreferences().cameraId, "camera-test")

        port.setPreferredDeviceId(nil, kind: .microphone)
        port.setPreferredDeviceId(nil, kind: .camera)
        XCTAssertNil(port.storedPreferences().microphoneId)
        XCTAssertNil(port.storedPreferences().cameraId)
    }

    @MainActor
    func testRuntimeFallbackNeverRewritesStoredMediaIntent() throws {
        let suiteName = "HumanCallMediaPortTests.runtime-fallback"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defaults.removePersistentDomain(forName: suiteName)
        defer { defaults.removePersistentDomain(forName: suiteName) }

        let port = HumanCallMediaPort(defaults: defaults)
        let devices = [
            HumanCallMediaDevice(id: "mic-built-in", name: "Built-in microphone", kind: .microphone),
            HumanCallMediaDevice(id: "camera-front", name: "Front camera", kind: .camera),
        ]

        XCTAssertEqual(
            HumanCallMediaPort.resolvedDeviceId(
                preferredId: port.storedPreferences().microphoneId,
                kind: .microphone,
                devices: devices
            ),
            "mic-built-in"
        )
        XCTAssertNil(port.storedPreferences().microphoneId)

        port.setPreferredDeviceId("mic-removed", kind: .microphone)
        XCTAssertEqual(
            HumanCallMediaPort.resolvedDeviceId(
                preferredId: port.storedPreferences().microphoneId,
                kind: .microphone,
                devices: devices
            ),
            "mic-built-in"
        )
        XCTAssertEqual(port.storedPreferences().microphoneId, "mic-removed")

        XCTAssertEqual(
            HumanCallMediaPort.resolvedDeviceId(
                preferredId: port.storedPreferences().cameraId,
                kind: .camera,
                devices: devices
            ),
            "camera-front"
        )
        XCTAssertNil(port.storedPreferences().cameraId)
    }


}

extension HumanCallMediaPortTests {
    func testHumanCallSessionProjectionKeepsLifecycleIdentityAndActions() {
        let record = HumanCallSessionRecord(raw: [
            "id": "call-1",
            "scopeId": "conversation-1",
            "creatorId": "alice",
            "state": "ringing",
            "generation": 3,
            "participantIds": ["alice", "bob"],
            "mediaCapabilities": [
                "microphone": "granted",
                "camera": "denied",
            ],
            "deviceSelection": [
                "microphoneId": "mic-built-in",
                "cameraId": "camera-front",
            ],
            "updatedAtMs": 1234,
        ])

        XCTAssertEqual(record?.id, "call-1")
        XCTAssertEqual(record?.scopeId, "conversation-1")
        XCTAssertEqual(record?.generation, 3)
        XCTAssertEqual(record?.participantIds, ["alice", "bob"])
        XCTAssertEqual(record?.mediaCapabilities["microphone"], "granted")
        XCTAssertEqual(record?.deviceSelection["microphoneId"], "mic-built-in")
        XCTAssertEqual(record?.deviceSelection["cameraId"], "camera-front")
        XCTAssertEqual(record?.stateLabel, "响铃中")
        XCTAssertEqual(record?.canAccept, true)
        XCTAssertEqual(record?.canDecline, true)
        XCTAssertEqual(record?.canHangUp, true)
        XCTAssertEqual(record?.isTerminal, false)
    }

    func testHumanCallSessionProjectionFailsClosedAndTerminalStateDisablesActions() {
        XCTAssertNil(HumanCallSessionRecord(raw: [
            "scopeId": "conversation-1",
            "creatorId": "alice",
            "state": "ringing",
        ]))

        let ended = HumanCallSessionRecord(raw: [
            "id": "call-2",
            "scopeId": "conversation-1",
            "creatorId": "alice",
            "state": "ended",
            "generation": 4,
            "participantIds": ["alice", "bob"],
            "terminalReason": "hangup",
            "updatedAtMs": 5678,
        ])

        XCTAssertEqual(ended?.isTerminal, true)
        XCTAssertEqual(ended?.canAccept, false)
        XCTAssertEqual(ended?.canDecline, false)
        XCTAssertEqual(ended?.canHangUp, false)
        XCTAssertEqual(ended?.terminalReason, "hangup")
    }
}

extension HumanCallMediaPortTests {
    func testCallTransportLeaseProjectionFailsClosedAndKeepsOwnership() {
        let lease = HumanCallTransportLease(raw: [
            "userId": "alice",
            "deviceId": "ios-device",
            "role": "peer",
            "isOwner": true,
            "claimAvailable": false,
            "generation": 7,
        ])
        XCTAssertEqual(lease?.deviceId, "ios-device")
        XCTAssertEqual(lease?.role, "peer")
        XCTAssertEqual(lease?.isOwner, true)
        XCTAssertEqual(lease?.generation, 7)

        XCTAssertNil(HumanCallTransportLease(raw: [
            "userId": "alice",
            "deviceId": "ios-device",
            "role": "unexpected",
            "isOwner": true,
            "claimAvailable": false,
        ]))
    }

    func testCallSignalProjectionPreservesSequenceAndRejectsUnknownKinds() {
        let signal = HumanCallSignalRecord(raw: [
            "callId": "call-1",
            "generation": 3,
            "seq": 11,
            "senderDeviceId": "peer-device",
            "kind": "candidate",
            "payload": [
                "candidate": "candidate:1",
                "sdpMLineIndex": 0,
            ],
        ])
        XCTAssertEqual(signal?.callId, "call-1")
        XCTAssertEqual(signal?.generation, 3)
        XCTAssertEqual(signal?.seq, 11)
        XCTAssertEqual(signal?.senderDeviceId, "peer-device")
        XCTAssertEqual(signal?.payload["candidate"] as? String, "candidate:1")

        XCTAssertNil(HumanCallSignalRecord(raw: [
            "callId": "call-1",
            "generation": 3,
            "seq": 12,
            "senderDeviceId": "peer-device",
            "kind": "unknown",
            "payload": [:],
        ]))
    }

    @MainActor
    func testPreferredMediaDeviceResolutionUsesExactMatch() {
        let devices = [
            HumanCallMediaDevice(id: "mic-built-in", name: "iPhone 麦克风", kind: .microphone),
            HumanCallMediaDevice(id: "camera-front", name: "前置摄像头", kind: .camera),
            HumanCallMediaDevice(id: "camera-back", name: "后置摄像头", kind: .camera),
        ]

        XCTAssertEqual(
            HumanCallMediaPort.resolvedDeviceId(
                preferredId: "camera-back",
                kind: .camera,
                devices: devices
            ),
            "camera-back"
        )
    }

    @MainActor
    func testStalePreferredMediaDeviceFallsBackWithinKind() {
        let devices = [
            HumanCallMediaDevice(id: "mic-built-in", name: "iPhone 麦克风", kind: .microphone),
            HumanCallMediaDevice(id: "camera-front", name: "前置摄像头", kind: .camera),
            HumanCallMediaDevice(id: "camera-back", name: "后置摄像头", kind: .camera),
        ]

        XCTAssertEqual(
            HumanCallMediaPort.resolvedDeviceId(
                preferredId: "camera-removed",
                kind: .camera,
                devices: devices
            ),
            "camera-front"
        )
        XCTAssertEqual(
            HumanCallMediaPort.resolvedDeviceId(
                preferredId: "camera-front",
                kind: .microphone,
                devices: devices
            ),
            "mic-built-in"
        )
        XCTAssertNil(
            HumanCallMediaPort.resolvedDeviceId(
                preferredId: "camera-front",
                kind: .camera,
                devices: devices.filter { $0.kind == .microphone }
            )
        )
    }

    @MainActor
    func testMediaDevicePreferencesPersistAndClear() throws {
        let suiteName = "HumanCallMediaPortTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let port = HumanCallMediaPort(defaults: defaults)

        port.setPreferredDeviceId(" mic-built-in ", kind: .microphone)
        port.setPreferredDeviceId("camera-front", kind: .camera)
        XCTAssertEqual(
            port.storedPreferences(),
            HumanCallMediaPreferences(
                microphoneId: "mic-built-in",
                cameraId: "camera-front"
            )
        )

        port.setPreferredDeviceId(nil, kind: .microphone)
        port.setPreferredDeviceId("   ", kind: .camera)
        XCTAssertEqual(
            port.storedPreferences(),
            HumanCallMediaPreferences(microphoneId: nil, cameraId: nil)
        )
    }
}

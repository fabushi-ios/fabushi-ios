import AVFoundation
import Foundation
import ReplayKit

internal enum HumanCallMediaPermission: String, Sendable {
    case granted
    case denied
    case prompt
    case notRequested = "not-requested"
}

internal struct HumanCallMediaPermissions: Equatable, Sendable {
    let microphone: HumanCallMediaPermission
    let camera: HumanCallMediaPermission
}

internal struct HumanCallMediaDevice: Identifiable, Equatable, Sendable {
    enum Kind: String, Sendable {
        case microphone
        case camera
    }

    let id: String
    let name: String
    let kind: Kind
}

internal struct HumanCallMediaPreferences: Equatable, Sendable {
    let microphoneId: String?
    let cameraId: String?
}

internal struct HumanCallScreenShareCapability: Equatable, Sendable {
    let available: Bool
}

internal enum HumanCallMediaPortFailure: LocalizedError {
    case deviceUnavailable(HumanCallMediaDevice.Kind)

    var errorDescription: String? {
        switch self {
        case .deviceUnavailable(.microphone):
            return "所选麦克风当前不可用。"
        case .deviceUnavailable(.camera):
            return "所选摄像头当前不可用。"
        }
    }
}

@MainActor
internal final class HumanCallMediaPort {
    private enum PreferenceKey {
        static let microphone = "fabushi.human-call.preferred-microphone-id"
        static let camera = "fabushi.human-call.preferred-camera-id"
    }

    private let audioSession: AVAudioSession
    private let screenRecorder: RPScreenRecorder
    private let defaults: UserDefaults

    init(
        audioSession: AVAudioSession = .sharedInstance(),
        screenRecorder: RPScreenRecorder = .shared(),
        defaults: UserDefaults = .standard
    ) {
        self.audioSession = audioSession
        self.screenRecorder = screenRecorder
        self.defaults = defaults
    }

    static func permission(for status: AVAuthorizationStatus) -> HumanCallMediaPermission {
        switch status {
        case .authorized:
            return .granted
        case .denied, .restricted:
            return .denied
        case .notDetermined:
            return .prompt
        @unknown default:
            return .prompt
        }
    }

    static func resolvedDeviceId(
        preferredId: String?,
        kind: HumanCallMediaDevice.Kind,
        devices: [HumanCallMediaDevice]
    ) -> String? {
        let candidates = devices.filter { $0.kind == kind }
        guard !candidates.isEmpty else { return nil }
        if let preferredId = normalizedIdentifier(preferredId),
           candidates.contains(where: { $0.id == preferredId })
        {
            return preferredId
        }
        return candidates.first?.id
    }

    func requestPermissions(audio: Bool, video: Bool) async -> HumanCallMediaPermissions {
        async let microphone = requestPermission(mediaType: .audio, requested: audio)
        async let camera = requestPermission(mediaType: .video, requested: video)
        return await HumanCallMediaPermissions(
            microphone: microphone,
            camera: camera
        )
    }

    func devices() -> [HumanCallMediaDevice] {
        var result: [HumanCallMediaDevice] = []
        var seen = Set<String>()

        for input in audioSession.availableInputs ?? [] {
            let id = input.uid.trimmingCharacters(in: .whitespacesAndNewlines)
            let name = input.portName.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !id.isEmpty, !name.isEmpty, seen.insert("microphone:\(id)").inserted else {
                continue
            }
            result.append(.init(id: id, name: name, kind: .microphone))
        }

        let discovery = AVCaptureDevice.DiscoverySession(
            deviceTypes: [
                .builtInWideAngleCamera,
                .builtInUltraWideCamera,
                .builtInTelephotoCamera,
                .builtInDualCamera,
                .builtInDualWideCamera,
                .builtInTripleCamera,
                .builtInTrueDepthCamera,
            ],
            mediaType: .video,
            position: .unspecified
        )
        let cameras = discovery.devices.sorted { left, right in
            let leftRank = Self.cameraPositionRank(left.position)
            let rightRank = Self.cameraPositionRank(right.position)
            if leftRank != rightRank { return leftRank < rightRank }
            return left.uniqueID < right.uniqueID
        }
        for device in cameras {
            let id = device.uniqueID.trimmingCharacters(in: .whitespacesAndNewlines)
            let name = device.localizedName.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !id.isEmpty, !name.isEmpty, seen.insert("camera:\(id)").inserted else {
                continue
            }
            result.append(.init(id: id, name: name, kind: .camera))
        }

        return result
    }

    func storedPreferences() -> HumanCallMediaPreferences {
        HumanCallMediaPreferences(
            microphoneId: Self.normalizedIdentifier(defaults.string(forKey: PreferenceKey.microphone)),
            cameraId: Self.normalizedIdentifier(defaults.string(forKey: PreferenceKey.camera))
        )
    }

    func resolvedPreferences() -> HumanCallMediaPreferences {
        let available = devices()
        let stored = storedPreferences()
        return HumanCallMediaPreferences(
            microphoneId: Self.resolvedDeviceId(
                preferredId: stored.microphoneId,
                kind: .microphone,
                devices: available
            ),
            cameraId: Self.resolvedDeviceId(
                preferredId: stored.cameraId,
                kind: .camera,
                devices: available
            )
        )
    }

    func setPreferredDeviceId(_ id: String?, kind: HumanCallMediaDevice.Kind) {
        let key = kind == .microphone ? PreferenceKey.microphone : PreferenceKey.camera
        if let normalized = Self.normalizedIdentifier(id) {
            defaults.set(normalized, forKey: key)
        } else {
            defaults.removeObject(forKey: key)
        }
    }

    @discardableResult
    func applyPreferredMicrophone() throws -> HumanCallMediaDevice? {
        let availableInputs = audioSession.availableInputs ?? []
        let preferredId = storedPreferences().microphoneId
        let selected = preferredId.flatMap { id in
            availableInputs.first(where: { $0.uid == id })
        } ?? availableInputs.first

        guard let selected else {
            setPreferredDeviceId(nil, kind: .microphone)
            return nil
        }
        try audioSession.setPreferredInput(selected)
        setPreferredDeviceId(selected.uid, kind: .microphone)
        return HumanCallMediaDevice(
            id: selected.uid,
            name: selected.portName,
            kind: .microphone
        )
    }

    @discardableResult
    func selectMicrophone(deviceId: String) throws -> HumanCallMediaDevice {
        guard let input = (audioSession.availableInputs ?? []).first(where: { $0.uid == deviceId }) else {
            throw HumanCallMediaPortFailure.deviceUnavailable(.microphone)
        }
        try audioSession.setPreferredInput(input)
        setPreferredDeviceId(input.uid, kind: .microphone)
        return HumanCallMediaDevice(id: input.uid, name: input.portName, kind: .microphone)
    }

    @discardableResult
    func selectCamera(deviceId: String) throws -> HumanCallMediaDevice {
        guard let device = devices().first(where: { $0.kind == .camera && $0.id == deviceId }) else {
            throw HumanCallMediaPortFailure.deviceUnavailable(.camera)
        }
        setPreferredDeviceId(device.id, kind: .camera)
        return device
    }

    var activeMicrophoneId: String? {
        Self.normalizedIdentifier(audioSession.preferredInput?.uid)
            ?? Self.normalizedIdentifier(audioSession.currentRoute.inputs.first?.uid)
    }

    func screenShareCapability() -> HumanCallScreenShareCapability {
        HumanCallScreenShareCapability(available: screenRecorder.isAvailable)
    }

    private func requestPermission(
        mediaType: AVMediaType,
        requested: Bool
    ) async -> HumanCallMediaPermission {
        guard requested else { return .notRequested }

        let current = AVCaptureDevice.authorizationStatus(for: mediaType)
        let normalized = Self.permission(for: current)
        guard normalized == .prompt else { return normalized }

        let granted = await AVCaptureDevice.requestAccess(for: mediaType)
        return granted ? .granted : .denied
    }

    private static func normalizedIdentifier(_ value: String?) -> String? {
        guard let value else { return nil }
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return normalized.isEmpty ? nil : normalized
    }

    private static func cameraPositionRank(_ position: AVCaptureDevice.Position) -> Int {
        switch position {
        case .front:
            return 0
        case .back:
            return 1
        case .unspecified:
            return 2
        @unknown default:
            return 3
        }
    }
}

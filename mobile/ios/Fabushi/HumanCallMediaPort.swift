import AVFoundation
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

internal struct HumanCallScreenShareCapability: Equatable, Sendable {
    let available: Bool
}

@MainActor
internal final class HumanCallMediaPort {
    private let audioSession: AVAudioSession
    private let screenRecorder: RPScreenRecorder

    init(
        audioSession: AVAudioSession = .sharedInstance(),
        screenRecorder: RPScreenRecorder = .shared()
    ) {
        self.audioSession = audioSession
        self.screenRecorder = screenRecorder
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
        for device in discovery.devices {
            let id = device.uniqueID.trimmingCharacters(in: .whitespacesAndNewlines)
            let name = device.localizedName.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !id.isEmpty, !name.isEmpty, seen.insert("camera:\(id)").inserted else {
                continue
            }
            result.append(.init(id: id, name: name, kind: .camera))
        }

        return result
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
}

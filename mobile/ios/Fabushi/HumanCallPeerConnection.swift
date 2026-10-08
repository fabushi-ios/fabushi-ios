import AVFoundation
import Foundation
@preconcurrency import LiveKitWebRTC

@MainActor
final class HumanCallPeerConnection: NSObject {
    enum Failure: LocalizedError {
        case malformedICE
        case peerConnectionCreation
        case microphoneUnavailable
        case cameraUnavailable
        case malformedSignal

        var errorDescription: String? {
            switch self {
            case .malformedICE:
                return "通话服务返回了无效的 ICE 配置。"
            case .peerConnectionCreation:
                return "无法建立通话媒体连接。"
            case .microphoneUnavailable:
                return "无法建立麦克风音轨。"
            case .cameraUnavailable:
                return "无法建立摄像头音轨。"
            case .malformedSignal:
                return "通话信令格式无效。"
            }
        }
    }

    struct IceServer: Sendable {
        let urls: [String]
        let username: String?
        let credential: String?

        init?(raw: [String: Any]) {
            if let url = raw["urls"] as? String, !url.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                urls = [url]
            } else if let values = raw["urls"] as? [String] {
                let normalized = values.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
                guard !normalized.isEmpty else { return nil }
                urls = normalized
            } else {
                return nil
            }
            username = raw["username"] as? String
            credential = raw["credential"] as? String
        }
    }

    enum State: Sendable {
        case new
        case connecting
        case connected
        case disconnected
        case failed
        case closed
    }

    var onLocalCandidate: (([String: Any]) -> Void)?
    var onStateChange: ((State) -> Void)?
    var onRemoteVideoTrack: ((LKRTCVideoTrack?) -> Void)?

    private static let factory: LKRTCPeerConnectionFactory = {
        LKRTCInitializeSSL()
        return LKRTCPeerConnectionFactory()
    }()

    private var connection: LKRTCPeerConnection?
    private var audioTrack: LKRTCAudioTrack?
    private var videoTrack: LKRTCVideoTrack?
    private var videoSource: LKRTCVideoSource?
    private var cameraCapturer: LKRTCCameraVideoCapturer?

    func configure(iceServers: [IceServer], enableVideo: Bool) async throws {
        close()

        let configuration = LKRTCConfiguration()
        configuration.sdpSemantics = .unifiedPlan
        configuration.continualGatheringPolicy = .gatherContinually
        configuration.iceServers = iceServers.map {
            LKRTCIceServer(urlStrings: $0.urls, username: $0.username, credential: $0.credential)
        }
        guard let connection = Self.factory.peerConnection(
            with: configuration,
            constraints: LKRTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil),
            delegate: nil
        ) else {
            throw Failure.peerConnectionCreation
        }
        self.connection = connection
        connection.delegate = self

        let audioSource = Self.factory.audioSource(
            with: LKRTCMediaConstraints(
                mandatoryConstraints: [
                    "googNoiseSuppression": "true",
                    "googHighpassFilter": "true",
                    "googEchoCancellation": "true",
                    "googAutoGainControl": "true",
                ],
                optionalConstraints: nil
            )
        )
        let audioTrack = Self.factory.audioTrack(with: audioSource, trackId: "fabushi-human-call-audio")
        self.audioTrack = audioTrack
        connection.add(audioTrack, streamIds: ["fabushi-human-call"])

        if enableVideo {
            try await enableCamera()
        }

        try configureAudioSession()
    }

    func setMuted(_ muted: Bool) {
        audioTrack?.isEnabled = !muted
    }

    func setCameraEnabled(_ enabled: Bool) async throws {
        if enabled, videoTrack == nil {
            try await enableCamera()
        }
        videoTrack?.isEnabled = enabled
    }

    func makeOffer(iceRestart: Bool = false) async throws -> [String: Any] {
        guard let connection else { throw Failure.peerConnectionCreation }
        let constraints: LKRTCMediaConstraints
        if iceRestart {
            constraints = LKRTCMediaConstraints(
                mandatoryConstraints: ["IceRestart": "true"],
                optionalConstraints: nil
            )
        } else {
            constraints = LKRTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)
        }
        let offer = try await connection.offer(for: constraints)
        try await connection.setLocalDescription(offer)
        return ["type": "offer", "sdp": offer.sdp]
    }

    func applyOffer(_ payload: [String: Any]) async throws -> [String: Any] {
        guard
            let connection,
            let sdp = payload["sdp"] as? String,
            !sdp.isEmpty
        else { throw Failure.malformedSignal }
        try await connection.setRemoteDescription(LKRTCSessionDescription(type: .offer, sdp: sdp))
        let answer = try await connection.answer(
            for: LKRTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)
        )
        try await connection.setLocalDescription(answer)
        return ["type": "answer", "sdp": answer.sdp]
    }

    func applyAnswer(_ payload: [String: Any]) async throws {
        guard
            let connection,
            let sdp = payload["sdp"] as? String,
            !sdp.isEmpty
        else { throw Failure.malformedSignal }
        guard connection.signalingState == .haveLocalOffer else { return }
        try await connection.setRemoteDescription(LKRTCSessionDescription(type: .answer, sdp: sdp))
    }

    func applyCandidate(_ payload: [String: Any]) async throws {
        guard
            let connection,
            let candidate = payload["candidate"] as? String,
            !candidate.isEmpty
        else { throw Failure.malformedSignal }
        let sdpMid = payload["sdpMid"] as? String
        let sdpMLineIndex = (payload["sdpMLineIndex"] as? NSNumber)?.int32Value ?? 0
        try await connection.add(
            LKRTCIceCandidate(
                sdp: candidate,
                sdpMLineIndex: sdpMLineIndex,
                sdpMid: sdpMid
            )
        )
    }

    func close() {
        if let cameraCapturer {
            cameraCapturer.stopCapture()
        }
        cameraCapturer = nil
        videoTrack = nil
        videoSource = nil
        audioTrack = nil
        onRemoteVideoTrack?(nil)
        connection?.delegate = nil
        connection?.close()
        connection = nil
        try? AVAudioSession.sharedInstance().setActive(
            false,
            options: .notifyOthersOnDeactivation
        )
        onStateChange?(.closed)
    }

    private func enableCamera() async throws {
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
            throw Failure.cameraUnavailable
        }
        guard let connection else { throw Failure.peerConnectionCreation }

        if let videoTrack {
            videoTrack.isEnabled = true
            return
        }

        let devices = LKRTCCameraVideoCapturer.captureDevices()
        guard let device = devices.first(where: { $0.position == .front }) ?? devices.first else {
            throw Failure.cameraUnavailable
        }
        let formats = LKRTCCameraVideoCapturer.supportedFormats(for: device)
        guard let format = formats.min(by: { left, right in
            let l = CMVideoFormatDescriptionGetDimensions(left.formatDescription)
            let r = CMVideoFormatDescriptionGetDimensions(right.formatDescription)
            let lScore = abs(Int(l.width) - 1280) + abs(Int(l.height) - 720)
            let rScore = abs(Int(r.width) - 1280) + abs(Int(r.height) - 720)
            return lScore < rScore
        }) else {
            throw Failure.cameraUnavailable
        }
        let range = format.videoSupportedFrameRateRanges.first
        let fps = Int(min(30, max(1, range?.maxFrameRate ?? 30)))

        let source = Self.factory.videoSource()
        let capturer = LKRTCCameraVideoCapturer(delegate: source)
        let track = Self.factory.videoTrack(with: source, trackId: "fabushi-human-call-video")
        track.isEnabled = true
        connection.add(track, streamIds: ["fabushi-human-call"])
        self.videoSource = source
        self.cameraCapturer = capturer
        self.videoTrack = track

        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            capturer.startCapture(with: device, format: format, fps: fps) { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            }
        }
    }

    private func configureAudioSession() throws {
        let audioSession = AVAudioSession.sharedInstance()
        try audioSession.setCategory(
            .playAndRecord,
            mode: .videoChat,
            options: [.defaultToSpeaker, .allowBluetooth]
        )
        try audioSession.setActive(true)
    }
}

extension HumanCallPeerConnection: LKRTCPeerConnectionDelegate {
    nonisolated func peerConnectionShouldNegotiate(_ peerConnection: LKRTCPeerConnection) {}

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange stateChanged: LKRTCSignalingState) {}

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didAdd stream: LKRTCMediaStream) {
        let track = stream.videoTracks.first
        Task { @MainActor [weak self] in
            self?.onRemoteVideoTrack?(track)
        }
    }

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didRemove stream: LKRTCMediaStream) {
        Task { @MainActor [weak self] in
            self?.onRemoteVideoTrack?(nil)
        }
    }

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didGenerate candidate: LKRTCIceCandidate) {
        let payload: [String: Any] = [
            "candidate": candidate.sdp,
            "sdpMLineIndex": candidate.sdpMLineIndex,
            "sdpMid": candidate.sdpMid as Any,
        ]
        Task { @MainActor [weak self] in
            self?.onLocalCandidate?(payload)
        }
    }

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didRemove candidates: [LKRTCIceCandidate]) {}

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCIceConnectionState) {
        let mapped: State
        switch newState {
        case .new:
            mapped = .new
        case .checking:
            mapped = .connecting
        case .connected, .completed:
            mapped = .connected
        case .disconnected:
            mapped = .disconnected
        case .failed:
            mapped = .failed
        case .closed:
            mapped = .closed
        case .count:
            mapped = .failed
        @unknown default:
            mapped = .failed
        }
        Task { @MainActor [weak self] in
            self?.onStateChange?(mapped)
        }
    }

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCIceGatheringState) {}

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didOpen dataChannel: LKRTCDataChannel) {}
}

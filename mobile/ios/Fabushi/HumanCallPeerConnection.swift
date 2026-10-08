import AVFoundation
import CoreMedia
import Foundation
import ReplayKit
@preconcurrency import LiveKitWebRTC

@MainActor
final class HumanCallPeerConnection: NSObject {
    enum Failure: LocalizedError {
        case malformedICE
        case peerConnectionCreation
        case microphoneUnavailable
        case cameraUnavailable
        case screenShareUnavailable
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
            case .screenShareUnavailable:
                return "当前设备无法开始屏幕共享。"
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
    var onLocalVideoTrack: ((LKRTCVideoTrack?) -> Void)?
    var onRemoteVideoTrack: ((LKRTCVideoTrack?) -> Void)?
    var onScreenShareChange: ((Bool) -> Void)?
    var onScreenShareFailure: ((String) -> Void)?

    private static let factory: LKRTCPeerConnectionFactory = {
        LKRTCInitializeSSL()
        return LKRTCPeerConnectionFactory()
    }()

    private let screenRecorder = RPScreenRecorder.shared()
    private var connection: LKRTCPeerConnection?
    private var audioTrack: LKRTCAudioTrack?
    private var videoTrack: LKRTCVideoTrack?
    private var videoSource: LKRTCVideoSource?
    private var videoSender: LKRTCRtpSender?
    private var cameraCapturer: LKRTCCameraVideoCapturer?
    private var screenTrack: LKRTCVideoTrack?
    private var screenSource: LKRTCVideoSource?
    private var screenCapturer: LKRTCVideoCapturer?
    private var requestedCameraEnabled = false

    private(set) var activeCameraDeviceId: String?
    private(set) var isScreenSharing = false

    func configure(
        iceServers: [IceServer],
        enableVideo: Bool,
        preferredCameraId: String? = nil
    ) async throws {
        if connection != nil {
            close()
        }

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

        try prepareStableVideoSender(on: connection)
        if enableVideo {
            try await setCameraEnabled(true, preferredCameraId: preferredCameraId)
        }

        try configureAudioSession()
    }

    func setMuted(_ muted: Bool) {
        audioTrack?.isEnabled = !muted
    }

    func setCameraEnabled(_ enabled: Bool, preferredCameraId: String? = nil) async throws {
        requestedCameraEnabled = enabled
        guard enabled else {
            videoTrack?.isEnabled = false
            if !isScreenSharing {
                onLocalVideoTrack?(nil)
            }
            return
        }

        try await startCamera(preferredCameraId: preferredCameraId)
        videoTrack?.isEnabled = true
        if !isScreenSharing {
            videoSender?.track = videoTrack
            onLocalVideoTrack?(videoTrack)
        }
    }

    func selectCamera(deviceId: String) async throws {
        guard requestedCameraEnabled else {
            activeCameraDeviceId = deviceId
            return
        }
        try await startCamera(preferredCameraId: deviceId)
        if !isScreenSharing {
            videoSender?.track = videoTrack
            onLocalVideoTrack?(videoTrack)
        }
    }

    func setScreenShareEnabled(_ enabled: Bool) async throws {
        if enabled {
            try await startScreenShare()
        } else {
            await stopScreenShare()
        }
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
        if isScreenSharing || screenTrack != nil {
            screenRecorder.stopCapture()
        }
        cameraCapturer = nil
        screenCapturer = nil
        screenTrack = nil
        screenSource = nil
        videoSender = nil
        videoTrack = nil
        videoSource = nil
        audioTrack = nil
        activeCameraDeviceId = nil
        requestedCameraEnabled = false
        isScreenSharing = false
        onLocalVideoTrack?(nil)
        onRemoteVideoTrack?(nil)
        onScreenShareChange?(false)
        connection?.delegate = nil
        connection?.close()
        connection = nil
        try? AVAudioSession.sharedInstance().setActive(
            false,
            options: .notifyOthersOnDeactivation
        )
        onStateChange?(.closed)
    }

    private func prepareStableVideoSender(on connection: LKRTCPeerConnection) throws {
        let source = Self.factory.videoSource()
        let track = Self.factory.videoTrack(with: source, trackId: "fabushi-human-call-video")
        track.isEnabled = false
        guard let sender = connection.add(track, streamIds: ["fabushi-human-call"]) else {
            throw Failure.peerConnectionCreation
        }
        videoSource = source
        videoTrack = track
        videoSender = sender
    }

    private func startCamera(preferredCameraId: String?) async throws {
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
            throw Failure.cameraUnavailable
        }
        guard let videoSource, let videoTrack else {
            throw Failure.peerConnectionCreation
        }

        let devices = LKRTCCameraVideoCapturer.captureDevices()
        let preferred = preferredCameraId?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let device = preferred.flatMap({ id in devices.first(where: { $0.uniqueID == id }) })
            ?? devices.first(where: { $0.position == .front })
            ?? devices.first
        else {
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

        await cameraCapturer?.stopCapture()
        let capturer = LKRTCCameraVideoCapturer(delegate: videoSource)
        cameraCapturer = capturer
        do {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                capturer.startCapture(with: device, format: format, fps: fps) { error in
                    if let error {
                        continuation.resume(throwing: error)
                    } else {
                        continuation.resume()
                    }
                }
            }
        } catch {
            if cameraCapturer === capturer {
                cameraCapturer = nil
            }
            throw error
        }
        activeCameraDeviceId = device.uniqueID
        videoTrack.isEnabled = requestedCameraEnabled
    }

    private func startScreenShare() async throws {
        guard !isScreenSharing else { return }
        guard screenRecorder.isAvailable, let videoSender else {
            throw Failure.screenShareUnavailable
        }

        let source = Self.factory.videoSource(forScreenCast: true)
        let capturer = LKRTCVideoCapturer(delegate: source)
        let track = Self.factory.videoTrack(with: source, trackId: "fabushi-human-call-screen")
        track.isEnabled = true
        screenSource = source
        screenCapturer = capturer
        screenTrack = track

        do {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                screenRecorder.startCapture { [weak self, weak source, weak capturer] sampleBuffer, type, error in
                    if let error {
                        let message = error.localizedDescription
                        Task { @MainActor [weak self] in
                            self?.handleScreenCaptureFailure(message)
                        }
                        return
                    }
                    guard
                        type == .video,
                        let source,
                        let capturer,
                        let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer)
                    else { return }
                    let rtcBuffer = LKRTCCVPixelBuffer(pixelBuffer: pixelBuffer)
                    let frame = LKRTCVideoFrame(
                        buffer: rtcBuffer,
                        rotation: Self.rotation(for: sampleBuffer),
                        timeStampNs: Self.timestampNanoseconds(for: sampleBuffer)
                    )
                    source.capturer(capturer, didCapture: frame)
                } completionHandler: { error in
                    if let error {
                        continuation.resume(throwing: error)
                    } else {
                        continuation.resume()
                    }
                }
            }
        } catch {
            screenSource = nil
            screenCapturer = nil
            screenTrack = nil
            throw error
        }

        videoSender.track = track
        isScreenSharing = true
        onLocalVideoTrack?(track)
        onScreenShareChange?(true)
    }

    private func stopScreenShare() async {
        guard isScreenSharing || screenTrack != nil else { return }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            screenRecorder.stopCapture { _ in
                continuation.resume()
            }
        }
        restoreCameraAfterScreenShare()
    }

    private func handleScreenCaptureFailure(_ message: String) {
        screenRecorder.stopCapture()
        restoreCameraAfterScreenShare()
        onScreenShareFailure?(message)
    }

    private func restoreCameraAfterScreenShare() {
        screenTrack?.isEnabled = false
        screenTrack = nil
        screenSource = nil
        screenCapturer = nil
        isScreenSharing = false
        videoSender?.track = videoTrack
        videoTrack?.isEnabled = requestedCameraEnabled
        onLocalVideoTrack?(requestedCameraEnabled ? videoTrack : nil)
        onScreenShareChange?(false)
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

    nonisolated private static func timestampNanoseconds(for sampleBuffer: CMSampleBuffer) -> Int64 {
        let presentation = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let seconds = CMTimeGetSeconds(presentation)
        if seconds.isFinite, seconds >= 0 {
            return Int64(seconds * Double(NSEC_PER_SEC))
        }
        return Int64(ProcessInfo.processInfo.systemUptime * Double(NSEC_PER_SEC))
    }

    nonisolated private static func rotation(for sampleBuffer: CMSampleBuffer) -> LKRTCVideoRotation {
        guard
            let value = CMGetAttachment(
                sampleBuffer,
                key: RPVideoSampleOrientationKey as CFString,
                attachmentModeOut: nil
            ) as? NSNumber
        else { return ._0 }
        switch value.uint32Value {
        case 3:
            return ._180
        case 6:
            return ._90
        case 8:
            return ._270
        default:
            return ._0
        }
    }
}

extension HumanCallPeerConnection: LKRTCPeerConnectionDelegate {
    nonisolated func peerConnectionShouldNegotiate(_ peerConnection: LKRTCPeerConnection) {}

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange stateChanged: LKRTCSignalingState) {}

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didAdd stream: LKRTCMediaStream) {
        publishRemoteVideoTrack(stream.videoTracks.first)
    }

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didRemove stream: LKRTCMediaStream) {
        publishRemoteVideoTrack(nil)
    }

    nonisolated func peerConnection(
        _ peerConnection: LKRTCPeerConnection,
        didAdd rtpReceiver: LKRTCRtpReceiver,
        streams mediaStreams: [LKRTCMediaStream]
    ) {
        publishRemoteVideoTrack(rtpReceiver.track as? LKRTCVideoTrack)
    }

    nonisolated func peerConnection(
        _ peerConnection: LKRTCPeerConnection,
        didRemove rtpReceiver: LKRTCRtpReceiver
    ) {
        if rtpReceiver.track is LKRTCVideoTrack {
            publishRemoteVideoTrack(nil)
        }
    }

    nonisolated func peerConnection(
        _ peerConnection: LKRTCPeerConnection,
        didStartReceivingOn transceiver: LKRTCRtpTransceiver
    ) {
        publishRemoteVideoTrack(transceiver.receiver.track as? LKRTCVideoTrack)
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

    nonisolated private func publishRemoteVideoTrack(_ track: LKRTCVideoTrack?) {
        Task { @MainActor [weak self] in
            self?.onRemoteVideoTrack?(track)
        }
    }
}

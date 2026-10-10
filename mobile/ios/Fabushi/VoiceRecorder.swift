import AVFoundation
import Foundation
import Observation

enum VoiceRecorderErrorCode: String, Equatable, Sendable {
    case microphonePermissionDenied = "MICROPHONE_PERMISSION_DENIED"
    case audioDeviceUnavailable = "AUDIO_DEVICE_UNAVAILABLE"
    case recordingError = "RECORDING_ERROR"
    case unknown = "UNKNOWN"
}


@MainActor
@Observable
final class VoiceRecorder: NSObject, AVAudioRecorderDelegate {
    static let minimumRecordingDuration: TimeInterval = 0.5
    static let maximumRecordingDuration: TimeInterval = 300

    private(set) var isRecording = false
    private(set) var elapsedSeconds: Int = 0
    private(set) var didReachMaximumDuration = false
    private(set) var waveformLevel: Double = 0
    private(set) var waveformSamples: [Double] = []
    private(set) var errorCode: VoiceRecorderErrorCode?
    private(set) var errorRecoverable = true
    var errorMessage: String?

    static func normalizedWaveformLevel(decibels: Float) -> Double {
        let floorDb: Float = -60
        let clamped = min(0, max(floorDb, decibels))
        return Double((clamped - floorDb) / -floorDb)
    }

    static func shouldTranscribe(duration: TimeInterval) -> Bool {
        duration >= minimumRecordingDuration
    }

    static func reachedMaximumDuration(_ duration: TimeInterval) -> Bool {
        duration >= maximumRecordingDuration
    }

    static func appendingWaveformSample(
        _ samples: [Double],
        level: Double,
        limit: Int = 24
    ) -> [Double] {
        let boundedLimit = max(1, limit)
        let boundedLevel = min(1, max(0, level))
        var next = samples
        next.append(boundedLevel)
        if next.count > boundedLimit {
            next.removeFirst(next.count - boundedLimit)
        }
        return next
    }

    static func userMessage(for code: VoiceRecorderErrorCode) -> String {
        switch code {
        case .microphonePermissionDenied:
            return "Microphone access denied. Please enable microphone permissions in Settings."
        case .audioDeviceUnavailable:
            return "No microphone is available. Please check your audio input and try again."
        case .recordingError:
            return "Recording was interrupted. Please try again."
        case .unknown:
            return "Voice input is unavailable. Please try again."
        }
    }

    private func setFailure(
        _ code: VoiceRecorderErrorCode,
        message: String? = nil,
        recoverable: Bool
    ) {
        errorCode = code
        errorRecoverable = recoverable
        errorMessage = message ?? Self.userMessage(for: code)
    }

    private func clearFailure() {
        errorCode = nil
        errorRecoverable = true
        errorMessage = nil
    }

    private var recorder: AVAudioRecorder?
    private var timer: Timer?
    private var outputURL: URL?

    func start() async {
        clearFailure()
        let granted = await withCheckedContinuation { continuation in
            AVAudioApplication.requestRecordPermission { allowed in
                continuation.resume(returning: allowed)
            }
        }
        guard granted else {
            setFailure(.microphonePermissionDenied, recoverable: false)
            return
        }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, mode: .spokenAudio, options: [.defaultToSpeaker, .allowBluetooth])
            try session.setActive(true)
            if let inputs = session.availableInputs, inputs.isEmpty {
                try? session.setActive(false, options: .notifyOthersOnDeactivation)
                setFailure(.audioDeviceUnavailable, recoverable: true)
                return
            }
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("fabushi-voice", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let url = directory.appendingPathComponent("voice-\(UUID().uuidString.lowercased()).m4a")
            let settings: [String: Any] = [
                AVFormatIDKey: Int(kAudioFormatMPEG4AAC),
                AVSampleRateKey: 44_100,
                AVNumberOfChannelsKey: 1,
                AVEncoderAudioQualityKey: AVAudioQuality.medium.rawValue,
            ]
            let recorder = try AVAudioRecorder(url: url, settings: settings)
            recorder.delegate = self
            recorder.isMeteringEnabled = true
            recorder.prepareToRecord()
            guard recorder.record() else { throw NSError(domain: "FabushiVoice", code: 1, userInfo: [NSLocalizedDescriptionKey: "无法开始录音"]) }
            self.recorder = recorder
            outputURL = url
            elapsedSeconds = 0
            didReachMaximumDuration = false
            waveformLevel = 0
            waveformSamples = []
            isRecording = true
            timer?.invalidate()
            timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
                Task { @MainActor in
                    guard let self, let recorder = self.recorder, self.isRecording else { return }
                    recorder.updateMeters()
                    let duration = recorder.currentTime
                    self.elapsedSeconds = Int(duration)
                    self.waveformLevel = Self.normalizedWaveformLevel(
                        decibels: recorder.averagePower(forChannel: 0)
                    )
                    self.waveformSamples = Self.appendingWaveformSample(
                        self.waveformSamples,
                        level: self.waveformLevel
                    )
                    if Self.reachedMaximumDuration(duration) {
                        self.didReachMaximumDuration = true
                        self.timer?.invalidate()
                        self.timer = nil
                    }
                }
            }
        } catch {
            let code: VoiceRecorderErrorCode
            if let avError = error as? AVError, avError.code == .deviceNotConnected {
                code = .audioDeviceUnavailable
            } else {
                code = .recordingError
            }
            setFailure(code, message: error.localizedDescription, recoverable: true)
            isRecording = false
            recorder = nil
            if let outputURL { try? FileManager.default.removeItem(at: outputURL) }
            outputURL = nil
            timer?.invalidate()
            timer = nil
            waveformLevel = 0
            waveformSamples = []
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        }
    }

    func stop() -> (url: URL, data: Data)? {
        guard isRecording, let recorder, let outputURL else { return nil }
        let recordedDuration = recorder.currentTime
        recorder.stop()
        timer?.invalidate()
        timer = nil
        isRecording = false
        didReachMaximumDuration = false
        waveformLevel = 0
        waveformSamples = []
        self.recorder = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)

        guard Self.shouldTranscribe(duration: recordedDuration) else {
            try? FileManager.default.removeItem(at: outputURL)
            self.outputURL = nil
            return nil
        }
        guard let data = try? Data(contentsOf: outputURL), !data.isEmpty else {
            setFailure(.recordingError, message: "The recording file is empty.", recoverable: true)
            try? FileManager.default.removeItem(at: outputURL)
            self.outputURL = nil
            return nil
        }
        self.outputURL = nil
        return (outputURL, data)
    }

    func cancel() {
        recorder?.stop()
        timer?.invalidate()
        timer = nil
        isRecording = false
        didReachMaximumDuration = false
        waveformLevel = 0
        waveformSamples = []
        if let outputURL { try? FileManager.default.removeItem(at: outputURL) }
        recorder = nil
        outputURL = nil
        clearFailure()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    func audioRecorderDidFinishRecording(_ recorder: AVAudioRecorder, successfully flag: Bool) {
        guard !flag, self.recorder === recorder else { return }
        timer?.invalidate()
        timer = nil
        isRecording = false
        didReachMaximumDuration = false
        waveformLevel = 0
        waveformSamples = []
        self.recorder = nil
        if let outputURL { try? FileManager.default.removeItem(at: outputURL) }
        outputURL = nil
        setFailure(.recordingError, recoverable: true)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

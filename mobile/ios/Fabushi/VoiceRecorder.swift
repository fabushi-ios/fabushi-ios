import AVFoundation
import Foundation
import Observation

@MainActor
@Observable
final class VoiceRecorder: NSObject, AVAudioRecorderDelegate {
    static let minimumRecordingDuration: TimeInterval = 0.5
    static let maximumRecordingDuration: TimeInterval = 300

    private(set) var isRecording = false
    private(set) var elapsedSeconds: Int = 0
    private(set) var didReachMaximumDuration = false
    private(set) var waveformLevel: Double = 0
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

    private var recorder: AVAudioRecorder?
    private var timer: Timer?
    private var outputURL: URL?

    func start() async {
        errorMessage = nil
        let granted = await withCheckedContinuation { continuation in
            AVAudioApplication.requestRecordPermission { allowed in
                continuation.resume(returning: allowed)
            }
        }
        guard granted else {
            errorMessage = "请允许麦克风权限后再发送语音"
            return
        }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, mode: .spokenAudio, options: [.defaultToSpeaker, .allowBluetooth])
            try session.setActive(true)
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
                    if Self.reachedMaximumDuration(duration) {
                        self.didReachMaximumDuration = true
                        self.timer?.invalidate()
                        self.timer = nil
                    }
                }
            }
        } catch {
            errorMessage = error.localizedDescription
            isRecording = false
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
        self.recorder = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)

        guard Self.shouldTranscribe(duration: recordedDuration) else {
            try? FileManager.default.removeItem(at: outputURL)
            self.outputURL = nil
            return nil
        }
        guard let data = try? Data(contentsOf: outputURL), !data.isEmpty else {
            errorMessage = "录音文件为空"
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
        if let outputURL { try? FileManager.default.removeItem(at: outputURL) }
        recorder = nil
        outputURL = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

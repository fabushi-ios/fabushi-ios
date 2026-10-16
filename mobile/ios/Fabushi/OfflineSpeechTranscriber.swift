import Foundation
import Speech

/// iOS-native replacement for Desktop's bundled offline ASR executable.
///
/// This adapter is deliberately fail-closed: every recognition request requires
/// Apple's on-device recognizer and never falls back to a network recognizer.
@MainActor
final class OfflineSpeechTranscriber {
    enum TranscriptionError: LocalizedError {
        case authorizationDenied
        case recognizerUnavailable
        case onDeviceRecognitionUnavailable
        case emptyTranscript

        var errorDescription: String? {
            switch self {
            case .authorizationDenied:
                return "请允许语音识别权限后再使用语音输入"
            case .recognizerUnavailable:
                return "当前语言的语音识别器不可用"
            case .onDeviceRecognitionUnavailable:
                return "此设备不支持当前语言的离线语音识别"
            case .emptyTranscript:
                return "没有识别到可用文字"
            }
        }
    }

    private var activeTask: SFSpeechRecognitionTask?
    private var activeContinuation: CheckedContinuation<String, Error>?
    private var activeGeneration: UUID?

    static func makeRequest(fileURL: URL) -> SFSpeechURLRecognitionRequest {
        let request = SFSpeechURLRecognitionRequest(url: fileURL)
        request.shouldReportPartialResults = false
        request.requiresOnDeviceRecognition = true
        return request
    }

    func transcribe(fileURL: URL, locale: Locale = .current) async throws -> String {
        let status = await Self.requestAuthorization()
        guard status == .authorized else { throw TranscriptionError.authorizationDenied }
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
            throw TranscriptionError.recognizerUnavailable
        }
        guard recognizer.supportsOnDeviceRecognition else {
            throw TranscriptionError.onDeviceRecognitionUnavailable
        }

        cancel()
        let generation = UUID()
        activeGeneration = generation
        let request = Self.makeRequest(fileURL: fileURL)

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                activeContinuation = continuation
                activeTask = recognizer.recognitionTask(with: request) { [weak self] result, error in
                    Task { @MainActor in
                        self?.settle(generation: generation, result: result, error: error)
                    }
                }
            }
        } onCancel: {
            Task { @MainActor [weak self] in self?.cancel() }
        }
    }

    func cancel() {
        activeTask?.cancel()
        activeTask = nil
        activeGeneration = nil
        if let continuation = activeContinuation {
            activeContinuation = nil
            continuation.resume(throwing: CancellationError())
        }
    }

    private func settle(generation: UUID, result: SFSpeechRecognitionResult?, error: Error?) {
        guard activeGeneration == generation, let continuation = activeContinuation else { return }
        if let error {
            activeTask = nil
            activeGeneration = nil
            activeContinuation = nil
            continuation.resume(throwing: error)
            return
        }
        guard let result, result.isFinal else { return }
        activeTask = nil
        activeGeneration = nil
        activeContinuation = nil
        let text = result.bestTranscription.formattedString.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty {
            continuation.resume(throwing: TranscriptionError.emptyTranscript)
        } else {
            continuation.resume(returning: text)
        }
    }

    private static func requestAuthorization() async -> SFSpeechRecognizerAuthorizationStatus {
        if SFSpeechRecognizer.authorizationStatus() != .notDetermined {
            return SFSpeechRecognizer.authorizationStatus()
        }
        return await withCheckedContinuation { continuation in
            SFSpeechRecognizer.requestAuthorization { status in
                continuation.resume(returning: status)
            }
        }
    }
}

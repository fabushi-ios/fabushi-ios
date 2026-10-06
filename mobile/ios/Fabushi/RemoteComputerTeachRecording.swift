import AVFoundation
import Foundation
import UIKit
import WebKit

@MainActor
protocol RemoteComputerTeachRecordingSourcing: AnyObject {
    func status() async throws -> TeachRecordingStatus
    func start(agentID: String, entryPoint: String) async throws -> TeachRecordingStatus
    func stop(agentID: String, save: Bool) async throws -> TeachRecordingStatus
}

@MainActor
final class IOSRemoteComputerTeachRecordingSource: RemoteComputerTeachRecordingSourcing {
    enum SourceError: LocalizedError {
        case bridgeUnavailable
        case invalidResponse

        var errorDescription: String? {
            switch self {
            case .bridgeUnavailable:
                return "Teach Recording Host bridge 不可用。"
            case .invalidResponse:
                return "Teach Recording Host 返回了无效响应。"
            }
        }
    }

    private let bridge: IOSPreloadBridge?

    init(bridge: IOSPreloadBridge?) {
        self.bridge = bridge
    }

    func status() async throws -> TeachRecordingStatus {
        try await execute(command: [
            "type": "teach.status",
            "requestId": requestID("status"),
        ])
    }

    func start(agentID: String, entryPoint: String) async throws -> TeachRecordingStatus {
        try await execute(command: [
            "type": "teach.start",
            "requestId": requestID("start"),
            "agentId": agentID,
            "entryPoint": entryPoint,
        ])
    }

    func stop(agentID: String, save: Bool) async throws -> TeachRecordingStatus {
        try await execute(command: [
            "type": "teach.stop",
            "requestId": requestID(save ? "save" : "discard"),
            "agentId": agentID,
            "save": save,
        ])
    }

    static func projectStatus(_ event: [String: Any]) throws -> TeachRecordingStatus {
        guard event["type"] as? String == "teach.changed",
              let raw = event["status"] as? [String: Any],
              let stateValue = raw["state"] as? String,
              let state = TeachRecordingStatus.State(rawValue: stateValue)
        else {
            throw SourceError.invalidResponse
        }

        let maxDuration =
            (raw["maxDurationMs"] as? NSNumber)?.intValue
            ?? raw["maxDurationMs"] as? Int
            ?? SAND_TEACH_MAX_DURATION_MS
        let startedAt =
            (raw["startedAtMs"] as? NSNumber)?.intValue
            ?? raw["startedAtMs"] as? Int
        let agentID = raw["agentId"] as? String
        let capturePath = raw["capturePath"] as? String

        if state == .recording {
            guard let agentID, !agentID.isEmpty, startedAt != nil else {
                throw SourceError.invalidResponse
            }
        }

        return .init(
            state: state,
            agentId: agentID,
            startedAtMs: startedAt,
            maxDurationMs: maxDuration,
            capturePath: capturePath
        )
    }

    private func execute(command: [String: Any]) async throws -> TeachRecordingStatus {
        guard let bridge else { throw SourceError.bridgeUnavailable }
        _ = try await bridge.request(
            method: "feature.execute",
            params: ["command": command]
        ).value

        let result = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 15_000
        ) { event in
            event["type"] as? String == "teach.changed"
        }
        guard let event = result.value as? [String: Any] else {
            throw SourceError.invalidResponse
        }
        return try Self.projectStatus(event)
    }

    private func requestID(_ suffix: String) -> String {
        "ios-teach-\(suffix)-\(UUID().uuidString.lowercased())"
    }
}

@MainActor
final class RemoteComputerTeachCaptureController: ObservableObject {
    enum CaptureError: LocalizedError {
        case viewerUnavailable
        case invalidDestination
        case snapshotUnavailable
        case writerSetup(String)
        case writerFailed(String)

        var errorDescription: String? {
            switch self {
            case .viewerUnavailable:
                return "当前没有可录制的 Computer 画面。"
            case .invalidDestination:
                return "Teach Recording 的保存路径无效。"
            case .snapshotUnavailable:
                return "无法从当前 Computer 画面取得录制帧。"
            case .writerSetup(let detail):
                return "无法启动 Teach Recording：\(detail)"
            case .writerFailed(let detail):
                return "Teach Recording 保存失败：\(detail)"
            }
        }
    }

    private weak var webView: WKWebView?
    private var captureTask: Task<Void, Never>?
    private var writer: AVAssetWriter?
    private var writerInput: AVAssetWriterInput?
    private var adaptor: AVAssetWriterInputPixelBufferAdaptor?
    private var destinationURL: URL?
    private var framesDirectory: URL?
    private var frameIndex: Int64 = 0
    private var width = 0
    private var height = 0

    var isRecording: Bool {
        captureTask != nil
    }

    func attach(_ webView: WKWebView) {
        self.webView = webView
    }

    func detach(_ webView: WKWebView) {
        if self.webView === webView {
            self.webView = nil
        }
    }

    func start(path: String) async throws {
        guard let webView else { throw CaptureError.viewerUnavailable }
        guard !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw CaptureError.invalidDestination
        }

        if isRecording {
            await stop(save: false)
        }

        let destination = URL(fileURLWithPath: path)
        let sessionDirectory = destination.deletingLastPathComponent()
        let frames = sessionDirectory.appendingPathComponent("frames", isDirectory: true)
        try FileManager.default.createDirectory(
            at: frames,
            withIntermediateDirectories: true
        )
        try? FileManager.default.removeItem(at: destination)

        guard let first = try await snapshot(webView),
              let cgImage = first.cgImage
        else {
            throw CaptureError.snapshotUnavailable
        }

        let targetWidth = Self.evenDimension(min(max(cgImage.width, 320), 1920))
        let targetHeight = Self.evenDimension(min(max(cgImage.height, 240), 1080))
        let writer: AVAssetWriter
        do {
            writer = try AVAssetWriter(outputURL: destination, fileType: .mp4)
        } catch {
            throw CaptureError.writerSetup(error.localizedDescription)
        }

        let input = AVAssetWriterInput(
            mediaType: .video,
            outputSettings: [
                AVVideoCodecKey: AVVideoCodecType.h264,
                AVVideoWidthKey: targetWidth,
                AVVideoHeightKey: targetHeight,
                AVVideoCompressionPropertiesKey: [
                    AVVideoAverageBitRateKey: 2_000_000,
                    AVVideoExpectedSourceFrameRateKey: 2,
                    AVVideoMaxKeyFrameIntervalKey: 4,
                ],
            ]
        )
        input.expectsMediaDataInRealTime = true
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: input,
            sourcePixelBufferAttributes: [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
                kCVPixelBufferWidthKey as String: targetWidth,
                kCVPixelBufferHeightKey as String: targetHeight,
            ]
        )
        guard writer.canAdd(input) else {
            throw CaptureError.writerSetup("video input is unsupported")
        }
        writer.add(input)
        guard writer.startWriting() else {
            throw CaptureError.writerSetup(writer.error?.localizedDescription ?? "writer did not start")
        }
        writer.startSession(atSourceTime: .zero)

        self.writer = writer
        writerInput = input
        self.adaptor = adaptor
        destinationURL = destination
        framesDirectory = frames
        width = targetWidth
        height = targetHeight
        frameIndex = 0

        try append(image: first)
        captureTask = Task { @MainActor [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                do {
                    try await Task.sleep(nanoseconds: 500_000_000)
                } catch {
                    break
                }
                guard !Task.isCancelled, let webView = self.webView else { break }
                do {
                    if let image = try await self.snapshot(webView) {
                        try self.append(image: image)
                    }
                } catch {
                    // One dropped frame must not destroy an otherwise valid
                    // recording; the Host owns final success/failure.
                }
            }
        }
    }

    func stop(save: Bool) async {
        captureTask?.cancel()
        captureTask = nil

        let localWriter = writer
        let localInput = writerInput
        let localDestination = destinationURL
        writer = nil
        writerInput = nil
        adaptor = nil
        destinationURL = nil
        framesDirectory = nil
        width = 0
        height = 0
        frameIndex = 0

        guard let localWriter, let localInput else { return }
        localInput.markAsFinished()
        await withCheckedContinuation { continuation in
            localWriter.finishWriting {
                continuation.resume()
            }
        }

        if !save, let localDestination {
            try? FileManager.default.removeItem(at: localDestination)
            try? FileManager.default.removeItem(
                at: localDestination
                    .deletingLastPathComponent()
                    .appendingPathComponent("frames", isDirectory: true)
            )
        }
    }

    private func snapshot(_ webView: WKWebView) async throws -> UIImage? {
        try await withCheckedThrowingContinuation { continuation in
            let configuration = WKSnapshotConfiguration()
            configuration.afterScreenUpdates = false
            webView.takeSnapshot(with: configuration) { image, error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: image)
                }
            }
        }
    }

    private func append(image: UIImage) throws {
        guard let writer,
              let input = writerInput,
              let adaptor,
              writer.status == .writing
        else {
            throw CaptureError.writerFailed(
                writer?.error?.localizedDescription ?? "writer is not active"
            )
        }
        guard input.isReadyForMoreMediaData else { return }
        guard let buffer = Self.pixelBuffer(
            image: image,
            width: width,
            height: height
        ) else {
            throw CaptureError.snapshotUnavailable
        }

        let timestamp = CMTime(value: frameIndex, timescale: 2)
        guard adaptor.append(buffer, withPresentationTime: timestamp) else {
            throw CaptureError.writerFailed(
                writer.error?.localizedDescription ?? "frame append failed"
            )
        }

        if frameIndex % 2 == 0,
           let framesDirectory,
           let data = image.jpegData(compressionQuality: 0.72)
        {
            let name = String(format: "frame-%05lld.jpg", frameIndex / 2)
            try? data.write(
                to: framesDirectory.appendingPathComponent(name),
                options: .atomic
            )
        }
        frameIndex += 1
    }

    private static func evenDimension(_ value: Int) -> Int {
        value.isMultiple(of: 2) ? value : value - 1
    }

    private static func pixelBuffer(
        image: UIImage,
        width: Int,
        height: Int
    ) -> CVPixelBuffer? {
        guard width > 0, height > 0 else { return nil }
        var buffer: CVPixelBuffer?
        let attributes: [String: Any] = [
            kCVPixelBufferCGImageCompatibilityKey as String: true,
            kCVPixelBufferCGBitmapContextCompatibilityKey as String: true,
        ]
        guard CVPixelBufferCreate(
            kCFAllocatorDefault,
            width,
            height,
            kCVPixelFormatType_32BGRA,
            attributes as CFDictionary,
            &buffer
        ) == kCVReturnSuccess,
        let buffer
        else {
            return nil
        }

        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        guard let base = CVPixelBufferGetBaseAddress(buffer),
              let context = CGContext(
                data: base,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: CVPixelBufferGetBytesPerRow(buffer),
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGBitmapInfo.byteOrder32Little.rawValue
                    | CGImageAlphaInfo.premultipliedFirst.rawValue
              ),
              let cgImage = image.cgImage
        else {
            return nil
        }

        context.setFillColor(UIColor.black.cgColor)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        let sourceWidth = CGFloat(cgImage.width)
        let sourceHeight = CGFloat(cgImage.height)
        let scale = min(CGFloat(width) / sourceWidth, CGFloat(height) / sourceHeight)
        let drawWidth = sourceWidth * scale
        let drawHeight = sourceHeight * scale
        let rect = CGRect(
            x: (CGFloat(width) - drawWidth) / 2,
            y: (CGFloat(height) - drawHeight) / 2,
            width: drawWidth,
            height: drawHeight
        )
        context.draw(cgImage, in: rect)
        return buffer
    }
}

struct RemoteComputerTeachRecordingArm: Equatable, Sendable {
    let agentID: String
    let entryPoint: String
}

@MainActor
final class RemoteComputerTeachRecordingOwner: ObservableObject {
    @Published private(set) var status = IDLE_TEACH_RECORDING_STATUS
    @Published private(set) var armed: RemoteComputerTeachRecordingArm?
    @Published private(set) var nowMilliseconds: Int
    @Published private(set) var isWorking = false
    @Published private(set) var errorMessage: String?

    private let source: any RemoteComputerTeachRecordingSourcing
    private let capture: RemoteComputerTeachCaptureController
    private var generation = 0
    private var timerTask: Task<Void, Never>?
    private var disposed = false

    init(
        source: any RemoteComputerTeachRecordingSourcing,
        capture: RemoteComputerTeachCaptureController,
        now: @escaping () -> Int = {
            Int(Date().timeIntervalSince1970 * 1_000)
        }
    ) {
        self.source = source
        self.capture = capture
        nowMilliseconds = now()
        self.now = now
    }

    private let now: () -> Int

    var elapsedMilliseconds: Int {
        guard status.state == .recording, let started = status.startedAtMs else { return 0 }
        return max(0, min(status.maxDurationMs, nowMilliseconds - started))
    }

    func arm(agentID: String, entryPoint: String = "fullscreen_title_bar") {
        guard !disposed, status.state != .recording else { return }
        armed = .init(agentID: agentID, entryPoint: entryPoint)
        errorMessage = nil
    }

    func dismissArm() {
        guard status.state != .recording else { return }
        armed = nil
    }

    func connect() async {
        await heal()
    }

    func noteReconnect() async {
        await heal()
    }

    func start(agentID: String, entryPoint: String) async {
        guard !disposed, !isWorking else { return }
        generation &+= 1
        let expectedGeneration = generation
        let previous = status
        let started = now()
        publish(.init(
            state: .recording,
            agentId: agentID,
            startedAtMs: started,
            maxDurationMs: SAND_TEACH_MAX_DURATION_MS
        ))
        armed = nil
        isWorking = true
        errorMessage = nil

        do {
            let remote = try await source.start(
                agentID: agentID,
                entryPoint: entryPoint
            )
            guard !disposed, generation == expectedGeneration else { return }
            guard let path = remote.capturePath, !path.isEmpty else {
                throw IOSRemoteComputerTeachRecordingSource.SourceError.invalidResponse
            }
            try await capture.start(path: path)
            guard !disposed, generation == expectedGeneration else {
                await capture.stop(save: false)
                return
            }
            publish(remote)
        } catch {
            if generation == expectedGeneration, !disposed {
                await capture.stop(save: false)
                _ = try? await source.stop(agentID: agentID, save: false)
                publish(previous)
                errorMessage = String(error.localizedDescription.prefix(240))
            }
        }
        if generation == expectedGeneration {
            isWorking = false
        }
    }

    func stop(save: Bool) async {
        guard !disposed,
              !isWorking,
              status.state == .recording,
              let agentID = status.agentId
        else { return }

        generation &+= 1
        let expectedGeneration = generation
        let previous = status
        isWorking = true
        errorMessage = nil
        publish(IDLE_TEACH_RECORDING_STATUS)
        await capture.stop(save: save)

        do {
            let remote = try await source.stop(agentID: agentID, save: save)
            guard !disposed, generation == expectedGeneration else { return }
            publish(remote)
        } catch {
            guard !disposed, generation == expectedGeneration else { return }
            if !awaitHeal(expectedGeneration: expectedGeneration) {
                publish(previous)
            }
            errorMessage = String(error.localizedDescription.prefix(240))
        }
        if generation == expectedGeneration {
            isWorking = false
        }
    }

    func reset() {
        guard !disposed else { return }
        generation &+= 1
        let activeAgent = status.state == .recording ? status.agentId : nil
        status = IDLE_TEACH_RECORDING_STATUS
        armed = nil
        cancelTimer()
        errorMessage = nil
        isWorking = false
        Task { @MainActor [weak self] in
            guard let self else { return }
            await capture.stop(save: false)
            if let activeAgent {
                _ = try? await source.stop(agentID: activeAgent, save: false)
            }
        }
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        generation &+= 1
        cancelTimer()
        armed = nil
        Task { @MainActor [capture] in
            await capture.stop(save: false)
        }
    }

    private func heal() async {
        guard !disposed else { return }
        generation &+= 1
        let expectedGeneration = generation
        _ = awaitHeal(expectedGeneration: expectedGeneration)
    }

    @discardableResult
    private func awaitHeal(expectedGeneration: Int) async -> Bool {
        do {
            let remote = try await source.status()
            guard !disposed, generation == expectedGeneration else { return false }
            if remote.state == .recording,
               let path = remote.capturePath,
               !capture.isRecording
            {
                try await capture.start(path: path)
            } else if remote.state == .idle, capture.isRecording {
                await capture.stop(save: false)
            }
            publish(remote)
            errorMessage = nil
            return true
        } catch {
            guard !disposed, generation == expectedGeneration else { return false }
            errorMessage = String(error.localizedDescription.prefix(240))
            return false
        }
    }

    private func publish(_ next: TeachRecordingStatus) {
        status = next
        nowMilliseconds = now()
        updateTimer()
    }

    private func updateTimer() {
        if status.state == .recording {
            guard timerTask == nil else { return }
            timerTask = Task { @MainActor [weak self] in
                while let self, !Task.isCancelled, !self.disposed,
                      self.status.state == .recording
                {
                    do {
                        try await Task.sleep(nanoseconds: 1_000_000_000)
                    } catch {
                        break
                    }
                    guard !Task.isCancelled else { break }
                    self.nowMilliseconds = self.now()
                    if self.elapsedMilliseconds >= self.status.maxDurationMs {
                        self.timerTask = nil
                        await self.stop(save: true)
                        return
                    }
                }
            }
        } else {
            cancelTimer()
        }
    }

    private func cancelTimer() {
        timerTask?.cancel()
        timerTask = nil
    }
}

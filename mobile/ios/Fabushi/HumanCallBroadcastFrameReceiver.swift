import CoreGraphics
import CoreImage
import CoreVideo
import Foundation

internal struct HumanCallBroadcastPixelFrame: @unchecked Sendable {
    let pixelBuffer: CVPixelBuffer
    let timestampNanoseconds: Int64
    let orientation: UInt32
    let sequence: UInt64
}

internal actor HumanCallBroadcastFrameReceiver {
    private let context = CIContext(options: [.cacheIntermediates: false])
    private var receiveTask: Task<Void, Never>?

    func start(
        sessionID: String,
        onFrame: @escaping @Sendable @MainActor (HumanCallBroadcastPixelFrame) -> Void,
        onFinished: @escaping @Sendable @MainActor () -> Void
    ) {
        receiveTask?.cancel()
        receiveTask = Task { [weak self] in
            guard let self else { return }
            var lastSequence: UInt64 = 0
            while !Task.isCancelled {
                if let snapshot = self.readSnapshot(
                    sessionID: sessionID,
                    afterSequence: lastSequence
                ) {
                    switch snapshot.metadata.state {
                    case .started:
                        break
                    case .finished:
                        await onFinished()
                        return
                    case .frame:
                        if let frame = snapshot.frame {
                            lastSequence = frame.sequence
                            await onFrame(frame)
                        }
                    }
                }
                do {
                    try await Task.sleep(for: .milliseconds(50))
                } catch {
                    return
                }
            }
        }
    }

    func stop() {
        receiveTask?.cancel()
        receiveTask = nil
    }

    private struct Snapshot {
        let metadata: HumanCallBroadcastFrameMetadata
        let frame: HumanCallBroadcastPixelFrame?
    }

    private func readSnapshot(
        sessionID: String,
        afterSequence: UInt64
    ) -> Snapshot? {
        guard
            let container = HumanCallBroadcastIPC.containerURL(),
            let metadataData = try? Data(
                contentsOf: container.appendingPathComponent(HumanCallBroadcastIPC.metadataFilename)
            ),
            let metadata = try? JSONDecoder().decode(
                HumanCallBroadcastFrameMetadata.self,
                from: metadataData
            ),
            metadata.sessionID == sessionID
        else {
            return nil
        }

        guard metadata.state == .frame else {
            return Snapshot(metadata: metadata, frame: nil)
        }
        guard
            metadata.sequence > afterSequence,
            let filename = metadata.frameFilename,
            !filename.isEmpty,
            let data = try? Data(contentsOf: container.appendingPathComponent(filename)),
            let image = CIImage(data: data)
        else {
            return nil
        }

        let bounds = image.extent.integral
        guard bounds.width > 0, bounds.height > 0 else { return nil }
        var pixelBuffer: CVPixelBuffer?
        let attributes: [CFString: Any] = [
            kCVPixelBufferCGImageCompatibilityKey: true,
            kCVPixelBufferCGBitmapContextCompatibilityKey: true,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        let status = CVPixelBufferCreate(
            kCFAllocatorDefault,
            Int(bounds.width),
            Int(bounds.height),
            kCVPixelFormatType_32BGRA,
            attributes as CFDictionary,
            &pixelBuffer
        )
        guard status == kCVReturnSuccess, let pixelBuffer else { return nil }
        context.render(
            image,
            to: pixelBuffer,
            bounds: bounds,
            colorSpace: CGColorSpaceCreateDeviceRGB()
        )
        return Snapshot(
            metadata: metadata,
            frame: HumanCallBroadcastPixelFrame(
                pixelBuffer: pixelBuffer,
                timestampNanoseconds: metadata.timestampNanoseconds,
                orientation: metadata.orientation,
                sequence: metadata.sequence
            )
        )
    }
}

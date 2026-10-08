import CoreImage
import CoreMedia
import Foundation
import ReplayKit

final class SampleHandler: RPBroadcastSampleHandler {
    private let context = CIContext(options: [.cacheIntermediates: false])
    private var sessionID: String?
    private var sequence: UInt64 = 0
    private var previousFrameFilename: String?
    private var lastFrameUptime: TimeInterval = 0

    override func broadcastStarted(withSetupInfo setupInfo: [String: NSObject]?) {
        guard
            let desired = HumanCallBroadcastIPC.desiredSessionID(),
            let container = HumanCallBroadcastIPC.containerURL()
        else {
            finishBroadcastWithError(Self.error("missing-session"))
            return
        }
        sessionID = desired
        do {
            try writeMetadata(
                .init(
                    sessionID: desired,
                    sequence: 0,
                    state: .started,
                    frameFilename: nil,
                    timestampNanoseconds: Self.uptimeNanoseconds(),
                    orientation: 0
                ),
                container: container
            )
        } catch {
            finishBroadcastWithError(error)
        }
    }

    override func broadcastPaused() {}

    override func broadcastResumed() {}

    override func broadcastFinished() {
        guard
            let sessionID,
            let container = HumanCallBroadcastIPC.containerURL()
        else { return }
        try? writeMetadata(
            .init(
                sessionID: sessionID,
                sequence: sequence,
                state: .finished,
                frameFilename: nil,
                timestampNanoseconds: Self.uptimeNanoseconds(),
                orientation: 0
            ),
            container: container
        )
        removePreviousFrame(container: container)
    }

    override func processSampleBuffer(
        _ sampleBuffer: CMSampleBuffer,
        with sampleBufferType: RPSampleBufferType
    ) {
        guard sampleBufferType == .video else { return }
        guard
            let sessionID,
            HumanCallBroadcastIPC.desiredSessionID() == sessionID,
            let container = HumanCallBroadcastIPC.containerURL(),
            let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer)
        else {
            if self.sessionID != nil {
                finishBroadcastWithError(Self.error("stale-session"))
            }
            return
        }

        let now = ProcessInfo.processInfo.systemUptime
        guard now - lastFrameUptime >= (1.0 / 12.0) else { return }
        lastFrameUptime = now

        let image = CIImage(cvPixelBuffer: pixelBuffer)
        guard let jpeg = context.jpegRepresentation(
            of: image,
            colorSpace: CGColorSpaceCreateDeviceRGB(),
            options: [.lossyCompressionQuality: 0.58]
        ) else { return }

        sequence &+= 1
        let filename = "human-call-broadcast-frame-(sequence).jpg"
        let frameURL = container.appendingPathComponent(filename)
        do {
            try jpeg.write(to: frameURL, options: .atomic)
            let orientation = (
                CMGetAttachment(
                    sampleBuffer,
                    key: RPVideoSampleOrientationKey as CFString,
                    attachmentModeOut: nil
                ) as? NSNumber
            )?.uint32Value ?? 0
            try writeMetadata(
                .init(
                    sessionID: sessionID,
                    sequence: sequence,
                    state: .frame,
                    frameFilename: filename,
                    timestampNanoseconds: Self.timestampNanoseconds(sampleBuffer),
                    orientation: orientation
                ),
                container: container
            )
            if let previousFrameFilename, previousFrameFilename != filename {
                try? FileManager.default.removeItem(
                    at: container.appendingPathComponent(previousFrameFilename)
                )
            }
            previousFrameFilename = filename
        } catch {
            finishBroadcastWithError(error)
        }
    }

    private func writeMetadata(
        _ metadata: HumanCallBroadcastFrameMetadata,
        container: URL
    ) throws {
        let data = try JSONEncoder().encode(metadata)
        try data.write(
            to: container.appendingPathComponent(HumanCallBroadcastIPC.metadataFilename),
            options: .atomic
        )
    }

    private func removePreviousFrame(container: URL) {
        guard let previousFrameFilename else { return }
        try? FileManager.default.removeItem(
            at: container.appendingPathComponent(previousFrameFilename)
        )
        self.previousFrameFilename = nil
    }

    private static func timestampNanoseconds(_ sampleBuffer: CMSampleBuffer) -> Int64 {
        let presentation = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let seconds = CMTimeGetSeconds(presentation)
        guard seconds.isFinite, seconds >= 0 else {
            return uptimeNanoseconds()
        }
        return Int64(seconds * Double(NSEC_PER_SEC))
    }

    private static func uptimeNanoseconds() -> Int64 {
        Int64(ProcessInfo.processInfo.systemUptime * Double(NSEC_PER_SEC))
    }

    private static func error(_ code: String) -> NSError {
        NSError(
            domain: "com.ombhrum.fabushi.broadcast",
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: "Fabushi screen-share session is unavailable ((code))."]
        )
    }
}

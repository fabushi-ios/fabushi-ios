@preconcurrency import AVFoundation
import Foundation
import Observation

@MainActor
@Observable
final class VoicePlaybackController {
    private var player: AVPlayer?
    private var assetLoader: NativeMessagingBlobAssetLoader?
    private var endObserver: NSObjectProtocol?
    private(set) var playingMessageId: String?
    var errorMessage: String?

    func toggle(message: ChatMessage, messaging: MessagingModel) async {
        if playingMessageId == message.id {
            stop()
            return
        }
        guard let blobId = message.mediaBlobId, message.mediaSizeBytes > 0 else {
            errorMessage = message.contentType == "audio" ? "音频文件不可用" : "语音文件不可用"
            return
        }

        stop()
        let loader = NativeMessagingBlobAssetLoader(
            messaging: messaging,
            blobId: blobId,
            sizeBytes: message.mediaSizeBytes,
            mimeType: message.mediaMimeType
        ) { [weak self] detail in
            guard let self, self.playingMessageId == message.id else { return }
            self.errorMessage = detail
            self.stop()
        }
        let item = AVPlayerItem(asset: loader.makeAsset())
        let player = AVPlayer(playerItem: item)
        assetLoader = loader
        self.player = player
        playingMessageId = message.id
        errorMessage = nil
        endObserver = NotificationCenter.default.addObserver(
            forName: .AVPlayerItemDidPlayToEndTime,
            object: item,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.stop() }
        }
        player.play()
    }

    func stop() {
        player?.pause()
        player = nil
        assetLoader?.cancelAll()
        assetLoader = nil
        if let endObserver {
            NotificationCenter.default.removeObserver(endObserver)
            self.endObserver = nil
        }
        playingMessageId = nil
    }

    deinit {
        if let endObserver {
            NotificationCenter.default.removeObserver(endObserver)
        }
    }
}

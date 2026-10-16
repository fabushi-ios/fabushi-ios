import AVFoundation
import Foundation
import Observation

@MainActor
@Observable
final class VoicePlaybackController: NSObject, AVAudioPlayerDelegate {
    private var player: AVAudioPlayer?
    private(set) var playingMessageId: String?
    var errorMessage: String?

    func toggle(message: ChatMessage, messaging: MessagingModel) async {
        if playingMessageId == message.id {
            stop()
            return
        }
        guard let blobId = message.mediaBlobId, message.mediaSizeBytes > 0 else {
            errorMessage = "语音文件不可用"
            return
        }
        do {
            let data = try await messaging.loadBlob(blobId: blobId, sizeBytes: message.mediaSizeBytes)
            let player = try AVAudioPlayer(data: data)
            player.delegate = self
            player.prepareToPlay()
            guard player.play() else { throw NSError(domain: "FabushiVoicePlayback", code: 1, userInfo: [NSLocalizedDescriptionKey: "无法播放语音"]) }
            self.player?.stop()
            self.player = player
            playingMessageId = message.id
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
            stop()
        }
    }

    func stop() {
        player?.stop()
        player = nil
        playingMessageId = nil
    }

    nonisolated func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor in self.stop() }
    }
}

import SwiftUI
import UIKit
@preconcurrency import LiveKitWebRTC

@MainActor
internal struct HumanCallVideoView: UIViewRepresentable {
    let track: LKRTCVideoTrack?
    let mirrored: Bool

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeUIView(context: Context) -> LKRTCMTLVideoView {
        let view = LKRTCMTLVideoView(frame: .zero)
        view.backgroundColor = .black
        view.contentMode = .scaleAspectFill
        view.clipsToBounds = true
        context.coordinator.attach(track, to: view)
        view.transform = mirrored ? CGAffineTransform(scaleX: -1, y: 1) : .identity
        return view
    }

    func updateUIView(_ uiView: LKRTCMTLVideoView, context: Context) {
        context.coordinator.attach(track, to: uiView)
        uiView.transform = mirrored ? CGAffineTransform(scaleX: -1, y: 1) : .identity
    }

    static func dismantleUIView(_ uiView: LKRTCMTLVideoView, coordinator: Coordinator) {
        coordinator.detach(from: uiView)
    }

    @MainActor
    final class Coordinator {
        private var track: LKRTCVideoTrack?

        func attach(_ newTrack: LKRTCVideoTrack?, to view: LKRTCMTLVideoView) {
            guard track !== newTrack else { return }
            track?.remove(view)
            track = newTrack
            newTrack?.add(view)
        }

        func detach(from view: LKRTCMTLVideoView) {
            track?.remove(view)
            track = nil
        }
    }
}

@MainActor
internal struct HumanCallVideoStage: View {
    let remoteTrack: LKRTCVideoTrack?
    let localTrack: LKRTCVideoTrack?
    let localMirrored: Bool
    let mediaState: String

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            if let remoteTrack {
                HumanCallVideoView(track: remoteTrack, mirrored: false)
                    .accessibilityLabel("远端通话视频")
                    .accessibilityIdentifier("human-call-remote-video")
            } else {
                ZStack {
                    Color.black
                    VStack(spacing: 8) {
                        Image(systemName: "person.crop.rectangle")
                            .font(.system(size: 34, weight: .medium))
                        Text(mediaState == "通话中" ? "等待对方视频" : "正在建立媒体连接")
                            .font(.caption)
                    }
                    .foregroundStyle(.white.opacity(0.8))
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("远端视频尚未可用")
                .accessibilityIdentifier("human-call-remote-video-placeholder")
            }

            if let localTrack {
                HumanCallVideoView(track: localTrack, mirrored: localMirrored)
                    .frame(width: 104, height: 142)
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 12, style: .continuous)
                            .stroke(.white.opacity(0.75), lineWidth: 1)
                    }
                    .padding(10)
                    .shadow(radius: 4)
                    .accessibilityLabel(localMirrored ? "本地摄像头预览" : "本地屏幕共享预览")
                    .accessibilityIdentifier("human-call-local-video")
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: 250)
        .background(.black)
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

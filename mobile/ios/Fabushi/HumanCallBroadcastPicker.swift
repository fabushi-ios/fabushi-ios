import ReplayKit
import SwiftUI

internal struct HumanCallBroadcastPicker: UIViewRepresentable {
    func makeUIView(context: Context) -> RPSystemBroadcastPickerView {
        let picker = RPSystemBroadcastPickerView(frame: .zero)
        picker.preferredExtension = HumanCallBroadcastIPC.extensionBundleIdentifier
        picker.showsMicrophoneButton = false
        picker.accessibilityLabel = "开始跨应用屏幕共享"
        picker.accessibilityIdentifier = "human-call-broadcast-picker"
        return picker
    }

    func updateUIView(_ uiView: RPSystemBroadcastPickerView, context: Context) {
        uiView.preferredExtension = HumanCallBroadcastIPC.extensionBundleIdentifier
        uiView.showsMicrophoneButton = false
    }
}

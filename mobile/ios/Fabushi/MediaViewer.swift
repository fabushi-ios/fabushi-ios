import AVKit
import PDFKit
import SwiftUI
import UIKit

let nativePdfPreviewByteCap = 25 * 1024 * 1024

internal func isNativePdfAttachment(mimeType: String?, fileName: String?) -> Bool {
    if mimeType?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == "application/pdf" {
        return true
    }
    guard let fileName else { return false }
    return URL(fileURLWithPath: fileName).pathExtension.lowercased() == "pdf"
}

private struct NativePDFPreview: UIViewRepresentable {
    let url: URL

    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displayMode = .singlePageContinuous
        view.displayDirection = .vertical
        view.usePageViewController(false)
        view.backgroundColor = .black
        return view
    }

    func updateUIView(_ view: PDFView, context: Context) {
        if view.document?.documentURL != url {
            view.document = PDFDocument(url: url)
            view.autoScales = true
        }
    }
}

struct MediaViewer: View {
    let message: ChatMessage
    @Bindable var messaging: MessagingModel
    let onClose: () -> Void

    @State private var data: Data?
    @State private var localURL: URL?
    @State private var errorMessage: String?
    @State private var loading = true

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()
                content
            }
            .navigationTitle(message.mediaFileName ?? mediaTitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("关闭", action: onClose)
                }
                if let localURL {
                    ToolbarItem(placement: .topBarTrailing) {
                        ShareLink(item: localURL) { Image(systemName: "square.and.arrow.up") }
                    }
                }
            }
        }
        .task(id: message.id) { await load() }
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            ProgressView("正在载入…").tint(.white).foregroundStyle(.white)
        } else if let errorMessage {
            ContentUnavailableView("无法打开媒体", systemImage: "exclamationmark.triangle", description: Text(errorMessage))
                .foregroundStyle(.white)
        } else if message.contentType == "photo", let data, let image = UIImage(data: data) {
            ScrollView([.horizontal, .vertical]) {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: UIScreen.main.bounds.width, minHeight: UIScreen.main.bounds.height * 0.72)
            }
        } else if message.contentType == "video", let localURL {
            VideoPlayer(player: AVPlayer(url: localURL)).ignoresSafeArea(edges: .bottom)
        } else if isNativePdfAttachment(mimeType: message.mediaMimeType, fileName: message.mediaFileName),
                  let localURL {
            NativePDFPreview(url: localURL)
                .ignoresSafeArea(edges: .bottom)
                .accessibilityLabel(message.mediaFileName ?? "PDF document")
        } else if isNativeSpreadsheetAttachment(
            mimeType: message.mediaMimeType,
            fileName: message.mediaFileName
        ), let data, let localURL {
            NativeSpreadsheetPreview(
                data: data,
                url: localURL,
                fileName: message.mediaFileName,
                mimeType: message.mediaMimeType
            )
        } else if let localURL {
            VStack(spacing: 18) {
                Image(systemName: "doc.fill").font(.system(size: 64)).foregroundStyle(.orange)
                Text(message.mediaFileName ?? "文件").font(.title3.bold()).foregroundStyle(.white)
                if let mime = message.mediaMimeType { Text(mime).font(.caption).foregroundStyle(.secondary) }
                Text(ByteCountFormatter.string(fromByteCount: Int64(message.mediaSizeBytes), countStyle: .file)).foregroundStyle(.secondary)
                ShareLink(item: localURL) { Label("导出或用其他 App 打开", systemImage: "square.and.arrow.up") }
                    .buttonStyle(.borderedProminent)
            }.padding()
        }
    }

    private var mediaTitle: String {
        switch message.contentType {
        case "photo": "图片"
        case "video": "视频"
        default: "文件"
        }
    }

    @MainActor
    private func load() async {
        loading = true
        errorMessage = nil
        defer { loading = false }
        guard let blobId = message.mediaBlobId, message.mediaSizeBytes > 0 else {
            errorMessage = "媒体文件不可用"
            return
        }
        if isNativePdfAttachment(mimeType: message.mediaMimeType, fileName: message.mediaFileName),
           message.mediaSizeBytes > nativePdfPreviewByteCap {
            errorMessage = "PDF 超过 25 MB，无法在 Fabushi 内预览"
            return
        }
        if isNativeSpreadsheetAttachment(
            mimeType: message.mediaMimeType,
            fileName: message.mediaFileName
        ), message.mediaSizeBytes > nativeSpreadsheetPreviewByteCap {
            errorMessage = "Spreadsheet 超过 25 MB，无法在 Fabushi 内预览；可导出后打开。"
            return
        }
        do {
            let bytes = try await messaging.loadBlob(blobId: blobId, sizeBytes: message.mediaSizeBytes)
            data = bytes
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("fabushi-media", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let safeName = (message.mediaFileName ?? "media-\(message.id)").replacingOccurrences(of: "/", with: "-")
            let url = directory.appendingPathComponent(safeName)
            try bytes.write(to: url, options: .atomic)
            localURL = url
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

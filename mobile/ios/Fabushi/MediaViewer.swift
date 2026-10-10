import AVKit
import PDFKit
import SwiftUI
import UIKit

let nativePdfPreviewByteCap = 25 * 1024 * 1024

internal struct NativeMediaImageTransform: Equatable {
    static let minimumScale: CGFloat = 1
    static let maximumScale: CGFloat = 5

    var scale: CGFloat = minimumScale
    var offset: CGSize = .zero

    static func clampedScale(_ value: CGFloat) -> CGFloat {
        min(maximumScale, max(minimumScale, value))
    }

    func zoomed(by factor: CGFloat) -> NativeMediaImageTransform {
        let nextScale = Self.clampedScale(scale * factor)
        if nextScale <= Self.minimumScale {
            return .init()
        }
        return .init(scale: nextScale, offset: offset)
    }

    func panned(by translation: CGSize) -> NativeMediaImageTransform {
        guard scale > Self.minimumScale else { return .init() }
        return .init(
            scale: scale,
            offset: .init(
                width: offset.width + translation.width,
                height: offset.height + translation.height
            )
        )
    }
}

internal func isNativePdfAttachment(mimeType: String?, fileName: String?) -> Bool {
    if mimeType?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == "application/pdf" {
        return true
    }
    guard let fileName else { return false }
    return URL(fileURLWithPath: fileName).pathExtension.lowercased() == "pdf"
}

internal func mediaViewerAttachments(for message: ChatMessage) -> [ChatMediaAttachment] {
    if !message.mediaAttachments.isEmpty {
        return message.mediaAttachments
    }
    guard let blobId = message.mediaBlobId else { return [] }
    return [
        ChatMediaAttachment(
            id: "\(message.id)#\(blobId)",
            messageId: message.id,
            contentType: message.contentType,
            fileName: message.mediaFileName,
            blobId: blobId,
            mimeType: message.mediaMimeType,
            sizeBytes: message.mediaSizeBytes,
            groupIndex: message.mediaGroupIndex
        )
    ]
}

internal func clampedMediaAttachmentIndex(_ index: Int, count: Int) -> Int {
    guard count > 0 else { return 0 }
    return min(max(0, index), count - 1)
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
    @State private var loadGeneration = 0
    @State private var imageTransform = NativeMediaImageTransform()
    @State private var selectedAttachmentIndex = 0
    @GestureState private var imageMagnification: CGFloat = 1
    @GestureState private var imageDragTranslation: CGSize = .zero

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()
                content
            }
            .navigationTitle(selectedAttachment?.fileName ?? mediaTitle)
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
        .safeAreaInset(edge: .bottom) {
            if attachments.count > 1 {
                attachmentFilmstrip
            }
        }
        .task(id: selectedAttachment?.id ?? message.id) { await load() }
        .onDisappear { invalidateLoadAndCleanUp() }
    }

    private var attachments: [ChatMediaAttachment] {
        mediaViewerAttachments(for: message)
    }

    private var selectedAttachment: ChatMediaAttachment? {
        guard !attachments.isEmpty else { return nil }
        let index = clampedMediaAttachmentIndex(
            selectedAttachmentIndex,
            count: attachments.count
        )
        return attachments[index]
    }

    private var attachmentFilmstrip: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(Array(attachments.enumerated()), id: \.element.id) { index, attachment in
                    Button {
                        selectedAttachmentIndex = index
                    } label: {
                        VStack(spacing: 3) {
                            Image(systemName: attachment.contentType == "photo"
                                ? "photo"
                                : attachment.contentType == "video"
                                    ? "video"
                                    : "doc")
                            Text(attachment.fileName ?? "附件 \(index + 1)")
                                .font(.caption2)
                                .lineLimit(1)
                        }
                        .frame(width: 88, height: 52)
                        .background(
                            index == clampedMediaAttachmentIndex(
                                selectedAttachmentIndex,
                                count: attachments.count
                            )
                                ? Color.accentColor.opacity(0.28)
                                : Color.white.opacity(0.08),
                            in: RoundedRectangle(cornerRadius: 8)
                        )
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(.white)
                    .accessibilityLabel(
                        "附件 \(index + 1)，\(attachment.fileName ?? attachment.contentType)"
                    )
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .background(.ultraThinMaterial)
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            ProgressView("正在载入…").tint(.white).foregroundStyle(.white)
        } else if let errorMessage {
            ContentUnavailableView("无法打开媒体", systemImage: "exclamationmark.triangle", description: Text(errorMessage))
                .foregroundStyle(.white)
        } else if selectedAttachment?.contentType == "photo", let data, let image = UIImage(data: data) {
            GeometryReader { proxy in
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
                    .frame(width: proxy.size.width, height: proxy.size.height)
                    .scaleEffect(effectiveImageScale)
                    .offset(effectiveImageOffset)
                    .contentShape(Rectangle())
                    .simultaneousGesture(
                        MagnificationGesture()
                            .updating($imageMagnification) { value, state, _ in
                                state = value
                            }
                            .onEnded { value in
                                imageTransform = imageTransform.zoomed(by: value)
                            }
                    )
                    .simultaneousGesture(
                        DragGesture(minimumDistance: 4)
                            .updating($imageDragTranslation) { value, state, _ in
                                guard effectiveImageScale > NativeMediaImageTransform.minimumScale else {
                                    state = .zero
                                    return
                                }
                                state = value.translation
                            }
                            .onEnded { value in
                                imageTransform = imageTransform.panned(by: value.translation)
                            }
                    )
                    .onTapGesture(count: 2) {
                        resetImageTransform()
                    }
                    .accessibilityLabel(selectedAttachment?.fileName ?? "Image preview")
                    .accessibilityHint("Pinch to zoom, drag to pan, double tap to fit")
            }
        } else if selectedAttachment?.contentType == "video", let localURL {
            VideoPlayer(player: AVPlayer(url: localURL)).ignoresSafeArea(edges: .bottom)
        } else if isNativePdfAttachment(mimeType: selectedAttachment?.mimeType, fileName: selectedAttachment?.fileName),
                  let localURL {
            NativePDFPreview(url: localURL)
                .ignoresSafeArea(edges: .bottom)
                .accessibilityLabel(selectedAttachment?.fileName ?? "PDF document")
        } else if isNativeSpreadsheetAttachment(
            mimeType: selectedAttachment?.mimeType,
            fileName: selectedAttachment?.fileName
        ), let data, let localURL {
            NativeSpreadsheetPreview(
                data: data,
                url: localURL,
                fileName: selectedAttachment?.fileName,
                mimeType: selectedAttachment?.mimeType
            )
        } else if let localURL {
            VStack(spacing: 18) {
                Image(systemName: "doc.fill").font(.system(size: 64)).foregroundStyle(.orange)
                Text(selectedAttachment?.fileName ?? "文件").font(.title3.bold()).foregroundStyle(.white)
                if let mime = selectedAttachment?.mimeType { Text(mime).font(.caption).foregroundStyle(.secondary) }
                Text(ByteCountFormatter.string(fromByteCount: Int64(selectedAttachment?.sizeBytes ?? 0), countStyle: .file)).foregroundStyle(.secondary)
                ShareLink(item: localURL) { Label("导出或用其他 App 打开", systemImage: "square.and.arrow.up") }
                    .buttonStyle(.borderedProminent)
            }.padding()
        }
    }

    private var effectiveImageScale: CGFloat {
        NativeMediaImageTransform.clampedScale(imageTransform.scale * imageMagnification)
    }

    private var effectiveImageOffset: CGSize {
        guard effectiveImageScale > NativeMediaImageTransform.minimumScale else { return .zero }
        return .init(
            width: imageTransform.offset.width + imageDragTranslation.width,
            height: imageTransform.offset.height + imageDragTranslation.height
        )
    }

    @MainActor
    private func resetImageTransform() {
        imageTransform = .init()
    }

    private var mediaTitle: String {
        switch selectedAttachment?.contentType ?? message.contentType {
        case "photo": "图片"
        case "video": "视频"
        default: "文件"
        }
    }

    @MainActor
    private func load() async {
        loadGeneration = loadGeneration == Int.max ? 1 : loadGeneration + 1
        let generation = loadGeneration
        resetImageTransform()
        if let staleURL = localURL { try? FileManager.default.removeItem(at: staleURL) }
        data = nil
        localURL = nil
        loading = true
        errorMessage = nil
        defer {
            if generation == loadGeneration { loading = false }
        }
        guard let attachment = selectedAttachment,
              let blobId = attachment.blobId,
              attachment.sizeBytes > 0
        else {
            errorMessage = "媒体文件不可用"
            return
        }
        if isNativePdfAttachment(mimeType: attachment.mimeType, fileName: attachment.fileName),
           attachment.sizeBytes > nativePdfPreviewByteCap {
            errorMessage = "PDF 超过 25 MB，无法在 Fabushi 内预览"
            return
        }
        if isNativeSpreadsheetAttachment(
            mimeType: attachment.mimeType,
            fileName: attachment.fileName
        ), attachment.sizeBytes > nativeSpreadsheetPreviewByteCap {
            errorMessage = "Spreadsheet 超过 25 MB，无法在 Fabushi 内预览；可导出后打开。"
            return
        }
        do {
            let bytes = try await messaging.loadBlob(blobId: blobId, sizeBytes: attachment.sizeBytes)
            guard generation == loadGeneration, !Task.isCancelled else { return }
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("fabushi-media", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let safeName = (attachment.fileName ?? "media-\(attachment.messageId)").replacingOccurrences(of: "/", with: "-")
            let url = directory.appendingPathComponent("\(generation)-\(safeName)")
            try bytes.write(to: url, options: .atomic)
            guard generation == loadGeneration, !Task.isCancelled else {
                try? FileManager.default.removeItem(at: url)
                return
            }
            data = bytes
            localURL = url
        } catch is CancellationError {
            return
        } catch {
            guard generation == loadGeneration else { return }
            errorMessage = error.localizedDescription
        }
    }

    @MainActor
    private func invalidateLoadAndCleanUp() {
        loadGeneration = loadGeneration == Int.max ? 1 : loadGeneration + 1
        resetImageTransform()
        if let localURL { try? FileManager.default.removeItem(at: localURL) }
        data = nil
        localURL = nil
        loading = false
    }
}

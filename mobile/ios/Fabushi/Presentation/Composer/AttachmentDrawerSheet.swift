//
//  AttachmentDrawerSheet.swift
//  Fabushi
//
//  Production 1:1 Telegram iOS attachment bottom drawer sheet.
//  Provides dual detents (340pt and .large), 6 quick actions (Gallery, Camera, File, Contact, Location, Poll),
//  media multi-selection with dynamic badge re-indexing (1..100), send-without-compression toggle,
//  and document validation (rejecting 0-byte and >2GB files).
//

import SwiftUI
import UniformTypeIdentifiers

// MARK: - Action Types
public enum AttachmentActionType: String, CaseIterable, Identifiable, Sendable {
    case gallery
    case camera
    case file
    case contact
    case location
    case poll

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .gallery: return "Gallery"
        case .camera: return "Camera"
        case .file: return "File"
        case .contact: return "Contact"
        case .location: return "Location"
        case .poll: return "Poll"
        }
    }

    public var iconName: String {
        switch self {
        case .gallery: return "photo.fill"
        case .camera: return "camera.fill"
        case .file: return "doc.fill"
        case .contact: return "person.crop.circle.fill"
        case .location: return "location.fill"
        case .poll: return "chart.bar.xaxis"
        }
    }

    public var tintColor: Color {
        switch self {
        case .gallery: return Color.blue
        case .camera: return Color.gray
        case .file: return Color.cyan
        case .contact: return Color.orange
        case .location: return Color.green
        case .poll: return Color.teal
        }
    }
}

// MARK: - Media Preview Item
public struct MediaPreviewItem: Identifiable, Sendable, Equatable, Hashable {
    public let id: String
    public let isVideo: Bool
    public let durationString: String?
    public let fileSizeBytes: Int64
    public let thumbnailUri: String?

    public init(
        id: String,
        isVideo: Bool = false,
        durationString: String? = nil,
        fileSizeBytes: Int64 = 1024,
        thumbnailUri: String? = nil
    ) {
        self.id = id
        self.isVideo = isVideo
        self.durationString = durationString
        self.fileSizeBytes = fileSizeBytes
        self.thumbnailUri = thumbnailUri
    }
}

// MARK: - Document Attachment & Validation
public enum DocumentError: Error, Sendable, Equatable {
    case emptyFile
    case fileExceeds2GBLimit

    public var localizedDescription: String {
        switch self {
        case .emptyFile: return "Cannot send empty files"
        case .fileExceeds2GBLimit: return "File exceeds 2GB limit"
        }
    }
}

public struct DocumentAttachment: Identifiable, Sendable, Equatable {
    public let id: UUID
    public let url: URL
    public let fileName: String
    public let fileSizeBytes: Int64
    public static let maxTelegramFileSize: Int64 = 2 * 1024 * 1024 * 1024 // 2 GB

    public init(id: UUID = UUID(), url: URL, fileName: String, fileSizeBytes: Int64) {
        self.id = id
        self.url = url
        self.fileName = fileName
        self.fileSizeBytes = fileSizeBytes
    }

    public static func validate(fileSizeBytes: Int64) throws {
        guard fileSizeBytes > 0 else {
            throw DocumentError.emptyFile
        }
        guard fileSizeBytes <= maxTelegramFileSize else {
            throw DocumentError.fileExceeds2GBLimit
        }
    }
}

// MARK: - Attachment Drawer View Model
@MainActor
public final class AttachmentDrawerViewModel: ObservableObject, @unchecked Sendable {
    public static let defaultSheetHeight: CGFloat = 340.0
    public static let maxSelectableItems: Int = 100

    @Published public var selectedItems: [MediaPreviewItem] = []
    @Published public var sendWithoutCompression: Bool = false
    @Published public var showFilePicker: Bool = false
    @Published public var showLocationPicker: Bool = false
    @Published public var hasPhotoPermission: Bool = true
    @Published public var showPermissionAlert: Bool = false
    @Published public var errorMessage: String? = nil
    @Published public var previewItems: [MediaPreviewItem] = []

    public init(initialItems: [MediaPreviewItem] = []) {
        if initialItems.isEmpty {
            // Populate standard mock gallery previews for testing
            var items: [MediaPreviewItem] = []
            for i in 1...20 {
                let isVid: Bool = (i % 4 == 0)
                let dur: String? = isVid ? "0:15" : nil
                let bytes: Int64 = Int64(i * 512 * 1024)
                items.append(MediaPreviewItem(id: "preview_\(i)", isVideo: isVid, durationString: dur, fileSizeBytes: bytes))
            }
            self.previewItems = items
        } else {
            self.previewItems = initialItems
        }
    }

    public func toggleSelection(_ item: MediaPreviewItem) {
        if let idx = selectedItems.firstIndex(of: item) {
            selectedItems.remove(at: idx)
        } else if selectedItems.count < Self.maxSelectableItems {
            selectedItems.append(item)
        }
    }

    public func isSelected(_ item: MediaPreviewItem) -> Bool {
        selectedItems.contains(item)
    }

    public func badgeIndex(for item: MediaPreviewItem) -> Int? {
        selectedItems.firstIndex(of: item).map { $0 + 1 }
    }

    public func handleActionTap(_ action: AttachmentActionType) {
        switch action {
        case .gallery:
            guard hasPhotoPermission else {
                showPermissionAlert = true
                return
            }
        case .file:
            showFilePicker = true
        case .location:
            showLocationPicker = true
        default:
            break
        }
    }

    public func clearSelection() {
        selectedItems.removeAll()
    }
}

// MARK: - Attachment Drawer Sheet View
@MainActor
public struct AttachmentDrawerSheet: View {
    @ObservedObject public var viewModel: AttachmentDrawerViewModel
    public let onSendMedia: ([MediaPreviewItem], Bool) -> Void
    public let onSelectAction: (AttachmentActionType) -> Void
    public let onDismiss: () -> Void

    public init(
        viewModel: AttachmentDrawerViewModel,
        onSendMedia: @escaping ([MediaPreviewItem], Bool) -> Void,
        onSelectAction: @escaping (AttachmentActionType) -> Void = { _ in },
        onDismiss: @escaping () -> Void = {}
    ) {
        self.viewModel = viewModel
        self.onSendMedia = onSendMedia
        self.onSelectAction = onSelectAction
        self.onDismiss = onDismiss
    }

    public var body: some View {
        VStack(spacing: 0) {
            // Drag Indicator Handle
            Capsule()
                .fill(Color.secondary.opacity(0.3))
                .frame(width: 36, height: 5)
                .padding(.top, 8)
                .padding(.bottom, 12)

            // Top Horizontal Media Carousel
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 6) {
                    ForEach(viewModel.previewItems) { item in
                        mediaThumbnailView(for: item)
                    }
                }
                .padding(.horizontal, 16)
            }
            .frame(height: 100)

            // Compression Toggle Row
            Toggle(isOn: $viewModel.sendWithoutCompression) {
                HStack(spacing: 6) {
                    Image(systemName: "square.and.arrow.up")
                        .font(.system(size: 14))
                        .foregroundStyle(Color.secondary)
                    Text("Send without compression")
                        .font(.system(size: 14, weight: .regular))
                }
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 8)

            Divider()
                .padding(.vertical, 4)

            // 6 Quick Action Grid
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 12), count: 3), spacing: 16) {
                ForEach(AttachmentActionType.allCases) { action in
                    Button(action: {
                        viewModel.handleActionTap(action)
                        onSelectAction(action)
                    }) {
                        VStack(spacing: 8) {
                            ZStack {
                                Circle()
                                    .fill(action.tintColor.opacity(0.15))
                                    .frame(width: 54, height: 54)

                                Image(systemName: action.iconName)
                                    .font(.system(size: 24, weight: .semibold))
                                    .foregroundStyle(action.tintColor)
                            }

                            Text(action.title)
                                .font(.system(size: 12, weight: .medium))
                                .foregroundStyle(Color.primary)
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("attachment_action_\(action.rawValue)")
                }
            }
            .padding(.horizontal, 24)
            .padding(.top, 12)

            Spacer()

            // Bottom Send Selection Button (if items selected)
            if !viewModel.selectedItems.isEmpty {
                Button(action: {
                    onSendMedia(viewModel.selectedItems, viewModel.sendWithoutCompression)
                    onDismiss()
                }) {
                    HStack {
                        Text("Send \(viewModel.selectedItems.count) Item\(viewModel.selectedItems.count == 1 ? "" : "s")")
                            .font(.system(size: 16, weight: .semibold))
                    }
                    .frame(maxWidth: .infinity)
                    .frame(height: 44)
                    .background(Color.accentColor)
                    .foregroundStyle(Color.white)
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .background(Color.composerBackground)
        #if os(iOS)
        .presentationDetents([.height(AttachmentDrawerViewModel.defaultSheetHeight), .large])
        .presentationDragIndicator(.visible)
        #endif
    }

    @ViewBuilder
    private func mediaThumbnailView(for item: MediaPreviewItem) -> some View {
        let badgeNum = viewModel.badgeIndex(for: item)

        ZStack(alignment: .topTrailing) {
            RoundedRectangle(cornerRadius: 8)
                .fill(Color.secondary.opacity(0.18))
                .frame(width: 90, height: 90)
                .overlay(
                    Image(systemName: item.isVideo ? "video.fill" : "photo")
                        .font(.system(size: 24))
                        .foregroundStyle(Color.secondary.opacity(0.5))
                )

            // Numbered Order Badge (1..100)
            if let num = badgeNum {
                Text("\(num)")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(Color.white)
                    .frame(width: 22, height: 22)
                    .background(Circle().fill(Color.accentColor))
                    .padding(4)
                    .transition(.scale)
            } else {
                Circle()
                    .strokeBorder(Color.white, lineWidth: 1.5)
                    .background(Circle().fill(Color.black.opacity(0.2)))
                    .frame(width: 22, height: 22)
                    .padding(4)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture {
            viewModel.toggleSelection(item)
        }
    }
}

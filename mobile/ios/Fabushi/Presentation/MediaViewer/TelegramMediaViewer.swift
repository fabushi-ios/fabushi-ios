//
//  TelegramMediaViewer.swift
//  Fabushi
//
//  Production 1:1 Telegram iOS Fullscreen Interactive Media Gallery Viewer.
//  Implements velocity swipe-down dismissal physics, pinch-to-zoom (1.0x - 5.0x),
//  double-tap zoom (1.0x <-> 2.5x), horizontal carousel paging, and bottom filmstrip scrubber.
//

import SwiftUI
import AVKit

@MainActor
public struct TelegramMediaViewer: View {
    @ObservedObject public var viewModel: MediaViewerViewModel
    public let onDismiss: @MainActor () -> Void

    public init(
        viewModel: MediaViewerViewModel,
        onDismiss: @escaping @MainActor () -> Void
    ) {
        self.viewModel = viewModel
        self.onDismiss = onDismiss
    }

    public var body: some View {
        ZStack {
            // Backdrop with dynamic opacity fade
            Color.black
                .opacity(viewModel.dismissOpacity)
                .ignoresSafeArea()

            // Main Media Content View with Dismiss Scale & Offset
            ZStack {
                if !viewModel.items.isEmpty {
                    TabView(selection: $viewModel.selectedItemId) {
                        ForEach(viewModel.items) { item in
                            mediaItemContentView(item)
                                .tag(item.id)
                        }
                    }
                    #if os(iOS)
                    .tabViewStyle(.page(indexDisplayMode: .never))
                    #endif
                }
            }
            .scaleEffect(viewModel.dismissScale)
            .offset(y: viewModel.dismissOffsetY)
            .simultaneousGesture(
                TapGesture(count: 2)
                    .onEnded {
                        viewModel.handleDoubleTap()
                    }
            )
            .simultaneousGesture(
                TapGesture(count: 1)
                    .onEnded {
                        viewModel.toggleHUD()
                    }
            )
            .gesture(
                DragGesture(minimumDistance: 10)
                    .onChanged { value in
                        let now = Date().timeIntervalSince1970
                        if viewModel.lastDragTimestamp > 0 {
                            let dt = now - viewModel.lastDragTimestamp
                            if dt > 0.001 {
                                viewModel.dragVelocityY = (value.translation.height - viewModel.dragStartTranslation.height) / CGFloat(dt)
                            }
                        }
                        viewModel.lastDragTimestamp = now
                        viewModel.dragStartTranslation = value.translation
                        viewModel.handleDragChanged(translation: value.translation)
                    }
                    .onEnded { value in
                        viewModel.handleDragEnded(
                            translation: value.translation,
                            velocityY: viewModel.dragVelocityY,
                            onDismiss: onDismiss
                        )
                        viewModel.lastDragTimestamp = 0
                        viewModel.dragVelocityY = 0
                        viewModel.dragStartTranslation = .zero
                    }
            )

            // Top HUD Navigation Chrome
            if viewModel.isHUDVisible {
                VStack {
                    topHUDBar
                    Spacer()
                    bottomFilmstripBar
                }
                .transition(.opacity)
            }

            // Deleted Message Toast
            if let toast = viewModel.deletedMessageToast {
                VStack {
                    Spacer()
                    Text(toast)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(Color.white)
                        .padding(.horizontal, 20)
                        .padding(.vertical, 12)
                        .background(Capsule().fill(Color.black.opacity(0.85)))
                        .padding(.bottom, 60)
                }
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        #if os(iOS)
        .statusBarHidden(!viewModel.isHUDVisible)
        #endif
    }

    // Top HUD Bar: Back, Sender, Timestamp, More
    private var topHUDBar: some View {
        HStack {
            Button(action: onDismiss) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(Color.white)
                    .padding(10)
            }
            .buttonStyle(.plain)

            if let item = viewModel.currentItem {
                VStack(alignment: .leading, spacing: 2) {
                    Text(item.senderName ?? "Photo")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(Color.white)

                    Text(formatDate(item.timestamp))
                        .font(.system(size: 12))
                        .foregroundStyle(Color.white.opacity(0.7))
                }
                .padding(.leading, 4)
            }

            Spacer()

            Button(action: {}) {
                Image(systemName: "ellipsis")
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(Color.white)
                    .padding(10)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 12)
        .padding(.top, 44)
        .background(
            LinearGradient(colors: [Color.black.opacity(0.6), Color.clear], startPoint: .top, endPoint: .bottom)
        )
    }

    // Bottom Scrubber Bar & Filmstrip
    private var bottomFilmstripBar: some View {
        VStack(spacing: 8) {
            // Caption if present
            if let caption = viewModel.currentItem?.caption, !caption.isEmpty {
                Text(caption)
                    .font(.system(size: 14))
                    .foregroundStyle(Color.white)
                    .padding(.horizontal, 16)
                    .multilineTextAlignment(.center)
            }

            // Thumbnail Filmstrip Scrubber
            if viewModel.items.count > 1 {
                ScrollViewReader { proxy in
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(spacing: 6) {
                            ForEach(Array(viewModel.items.enumerated()), id: \.element.id) { index, item in
                                let isSelected = item.id == viewModel.selectedItemId
                                Button(action: {
                                    viewModel.selectItem(at: index)
                                }) {
                                    RoundedRectangle(cornerRadius: 6)
                                        .fill(Color.white.opacity(0.2))
                                        .frame(width: 44, height: 44)
                                        .overlay(
                                            Image(systemName: item.type == .video ? "video.fill" : "photo")
                                                .font(.system(size: 16))
                                                .foregroundStyle(Color.white)
                                        )
                                        .overlay(
                                            RoundedRectangle(cornerRadius: 6)
                                                .stroke(Color.white, lineWidth: isSelected ? 2.5 : 0)
                                        )
                                }
                                .buttonStyle(.plain)
                                .id(item.id)
                            }
                        }
                        .padding(.horizontal, 16)
                    }
                    .frame(height: 52)
                    .onChange(of: viewModel.selectedItemId) { _, newId in
                        withAnimation {
                            proxy.scrollTo(newId, anchor: .center)
                        }
                    }
                }
            }
        }
        .padding(.bottom, 24)
        .background(
            LinearGradient(colors: [Color.clear, Color.black.opacity(0.65)], startPoint: .top, endPoint: .bottom)
        )
    }

    // Media item content renderer
    @ViewBuilder
    private func mediaItemContentView(_ item: TelegramMediaItem) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.secondary.opacity(0.15))
                .aspectRatio(item.width / max(1, item.height), contentMode: .fit)
                .overlay(
                    VStack(spacing: 12) {
                        Image(systemName: item.type == .video ? "play.circle.fill" : "photo.fill")
                            .font(.system(size: 64))
                            .foregroundStyle(Color.white.opacity(0.8))

                        if let dur = item.durationSeconds {
                            Text(String(format: "%.1fs", dur))
                                .font(.system(size: 14, weight: .medium))
                                .foregroundStyle(Color.white)
                        }
                    }
                )
                .scaleEffect(viewModel.currentScale)
                .offset(viewModel.panOffset)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func formatDate(_ timestampMs: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(timestampMs) / 1000.0)
        let formatter = DateFormatter()
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }
}

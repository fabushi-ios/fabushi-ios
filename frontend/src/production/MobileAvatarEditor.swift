import PhotosUI
import SwiftUI
import UIKit

internal struct MobileAvatarEditorSheet: View {
    let agent: MobileBotSummary
    let bridge: IOSPreloadBridge
    let onSaved: ([MobileBotSummary]) -> Void
    let onClose: () -> Void

    @State private var selectedPhoto: PhotosPickerItem?
    @State private var generateDescription = ""
    @State private var sourceImage: UIImage?
    @State private var crop: MobileAvatarCrop?
    @State private var selectedShape: String?
    @State private var selectedColor: String?
    @State private var busy = false
    @State private var failure: String?
    @State private var generation = 0
    @State private var lastDrag: CGSize = .zero
    @State private var operationTask: Task<Void, Never>?

    init(
        agent: MobileBotSummary,
        bridge: IOSPreloadBridge,
        onSaved: @escaping ([MobileBotSummary]) -> Void,
        onClose: @escaping () -> Void
    ) {
        self.agent = agent
        self.bridge = bridge
        self.onSaved = onSaved
        self.onClose = onClose
        _selectedShape = State(initialValue: agent.avatarShape)
        _selectedColor = State(initialValue: agent.avatarColor)
    }

    var body: some View {
        NavigationStack {
            Form {
                uploadSection
                if !agent.isGroup {
                    characterSection
                }
                if let failure {
                    Section {
                        Text(failure)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("mobile-avatar-error")
                    }
                }
            }
            .navigationTitle("头像与角色")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("完成", action: onClose).disabled(busy)
                }
            }
        }
        .accessibilityIdentifier("mobile-avatar-editor")
        .onChange(of: selectedPhoto) { _, item in
            loadPhoto(item)
        }
        .onDisappear {
            generation += 1
            operationTask?.cancel()
            operationTask = nil
        }
    }

    private var uploadSection: some View {
        Section {
            if !agent.isGroup {
                TextField("描述要生成的头像", text: $generateDescription, axis: .vertical)
                    .lineLimit(2...4)
                    .disabled(busy)
                    .accessibilityIdentifier("mobile-avatar-generate-description")
                Button(busy ? "生成中…" : "生成头像") { generateImage() }
                    .disabled(
                        busy || generateDescription
                            .trimmingCharacters(in: .whitespacesAndNewlines)
                            .isEmpty
                    )
                    .accessibilityIdentifier("mobile-avatar-generate")
            }

            if let sourceImage, let crop {
                avatarCropPreview(image: sourceImage, crop: crop)
                Slider(
                    value: Binding(
                        get: { self.crop?.zoom ?? AvatarImagePolicy.minZoom },
                        set: { value in
                            guard let image = self.sourceImage, var next = self.crop else { return }
                            next.zoom = value
                            self.crop = AvatarImagePolicy.clampCrop(
                                width: image.size.width,
                                height: image.size.height,
                                crop: next
                            )
                        }
                    ),
                    in: AvatarImagePolicy.minZoom...AvatarImagePolicy.maxZoom
                )
                .disabled(busy)
                .accessibilityLabel("Avatar zoom")

                HStack {
                    Button("重新选择") { selectedPhoto = nil }
                    Spacer()
                    Button(busy ? "保存中…" : "设为头像") { saveImage() }
                        .buttonStyle(.borderedProminent)
                        .disabled(busy)
                }
            } else {
                PhotosPicker(selection: $selectedPhoto, matching: .images) {
                    Label("从照片选择", systemImage: "photo")
                }
                .disabled(busy)
                .accessibilityIdentifier("mobile-avatar-photo-picker")
            }

            if agent.avatarDataURL != nil {
                Button("移除自定义头像", role: .destructive) {
                    clearImage()
                }
                .disabled(busy)
                .accessibilityIdentifier("mobile-avatar-clear")
            }
        } header: {
            Text("头像")
        } footer: {
            Text("图片最大 25 MB；编辑源最长边归一化到 1024，保存为 256×256 PNG。拖动可重新定位，缩放范围 1×–5×。")
        }
    }

    private var characterSection: some View {
        Section {
            Text("形状").font(.caption).foregroundStyle(.secondary)
            LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 4), spacing: 10) {
                ForEach(AvatarImagePolicy.shapes, id: \.self) { shape in
                    Button {
                        selectedShape = shape
                    } label: {
                        Text(shape.prefix(1).uppercased())
                            .frame(width: 38, height: 38)
                            .background(
                                selectedShape == shape ? Color.accentColor.opacity(0.2) : Color.secondary.opacity(0.12),
                                in: RoundedRectangle(cornerRadius: 10)
                            )
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("\(shape) character shape")
                }
            }

            Text("颜色").font(.caption).foregroundStyle(.secondary)
            LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 6), spacing: 10) {
                ForEach(AvatarImagePolicy.colors, id: \.id) { color in
                    Button {
                        selectedColor = color.id
                    } label: {
                        Circle()
                            .fill(Color(hexAvatar: color.value))
                            .frame(width: 28, height: 28)
                            .overlay(
                                Circle().stroke(
                                    selectedColor == color.id ? Color.primary : Color.clear,
                                    lineWidth: 2
                                )
                            )
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("\(color.label) character color")
                }
            }

            HStack {
                Button("恢复默认角色") {
                    commitCharacter(shape: "", color: "")
                }
                .disabled(busy)

                Spacer()

                Button(busy ? "保存中…" : "设为角色") {
                    commitCharacter(
                        shape: selectedShape ?? "",
                        color: selectedColor ?? ""
                    )
                }
                .buttonStyle(.borderedProminent)
                .disabled(busy || (selectedShape == nil && selectedColor == nil))
            }
        } header: {
            Text("Agent 角色")
        } footer: {
            Text("保存角色会移除自定义图片，与 Desktop 的 staged character commit 语义一致。")
        }
    }

    @ViewBuilder
    private func avatarCropPreview(image: UIImage, crop: MobileAvatarCrop) -> some View {
        let baseScale = AvatarImagePolicy.stageSize / min(image.size.width, image.size.height)
        let width = image.size.width * baseScale
        let height = image.size.height * baseScale
        let offsetX = (image.size.width / 2 - crop.centerX) * baseScale * crop.zoom
        let offsetY = (image.size.height / 2 - crop.centerY) * baseScale * crop.zoom

        Image(uiImage: image)
            .resizable()
            .frame(width: width, height: height)
            .scaleEffect(crop.zoom)
            .offset(x: offsetX, y: offsetY)
            .frame(width: AvatarImagePolicy.stageSize, height: AvatarImagePolicy.stageSize)
            .clipShape(Circle())
            .overlay(Circle().stroke(Color.secondary.opacity(0.35)))
            .contentShape(Circle())
            .gesture(
                DragGesture()
                    .onChanged { value in
                        let incremental = CGSize(
                            width: value.translation.width - lastDrag.width,
                            height: value.translation.height - lastDrag.height
                        )
                        lastDrag = value.translation
                        self.crop = AvatarImagePolicy.pan(
                            width: image.size.width,
                            height: image.size.height,
                            crop: self.crop ?? crop,
                            deltaX: incremental.width,
                            deltaY: incremental.height
                        )
                    }
                    .onEnded { _ in lastDrag = .zero }
            )
            .frame(maxWidth: .infinity)
            .accessibilityLabel("Drag to reposition avatar")
    }

    @MainActor
    private func loadPhoto(_ item: PhotosPickerItem?) {
        guard let item else { return }
        generation += 1
        let token = generation
        busy = true
        failure = nil
        operationTask?.cancel()
        operationTask = Task { @MainActor in
            defer {
                if token == generation {
                    busy = false
                    operationTask = nil
                }
            }
            do {
                guard let data = try await item.loadTransferable(type: Data.self) else {
                    throw AvatarImagePolicyError.invalidImage
                }
                try Task.checkCancellation()
                let image = try AvatarImagePolicy.normalizeSource(data: data)
                guard token == generation else { return }
                sourceImage = image
                crop = AvatarImagePolicy.initialCrop(
                    width: image.size.width,
                    height: image.size.height
                )
                selectedShape = nil
                selectedColor = nil
            } catch is CancellationError {
                return
            } catch {
                guard token == generation else { return }
                sourceImage = nil
                crop = nil
                failure = error.localizedDescription
            }
        }
    }

    @MainActor
    private func generateImage() {
        let description = generateDescription.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !description.isEmpty, !busy else { return }
        let token = beginMutation()
        operationTask = Task { @MainActor in
            defer { finishMutation(token) }
            do {
                let result = try await bridge.request(
                    method: "generateAgentAvatarImage",
                    params: ["description": description]
                )
                try Task.checkCancellation()
                guard token == generation,
                      let dataURL = result.value as? String,
                      let data = AvatarImagePolicy.data(fromImageDataURL: dataURL)
                else { throw AvatarImagePolicyError.invalidImage }
                let image = try AvatarImagePolicy.normalizeSource(data: data)
                guard token == generation else { return }
                sourceImage = image
                crop = AvatarImagePolicy.initialCrop(
                    width: image.size.width,
                    height: image.size.height
                )
                selectedShape = nil
                selectedColor = nil
            } catch is CancellationError {
                return
            } catch {
                guard token == generation else { return }
                failure = error.localizedDescription
            }
        }
    }

    @MainActor
    private func saveImage() {
        guard let sourceImage, let crop, !busy else { return }
        let token = beginMutation()
        operationTask = Task { @MainActor in
            defer { finishMutation(token) }
            do {
                let dataURL = try AvatarImagePolicy.pngDataURL(image: sourceImage, crop: crop)
                let updated = try await GrokMobileBotService(bridge: bridge).updateAgentAvatar(
                    id: agent.id,
                    isGroup: agent.isGroup,
                    avatarDataURL: dataURL
                )
                try Task.checkCancellation()
                guard token == generation else { return }
                onSaved(updated)
                onClose()
            } catch is CancellationError {
                return
            } catch {
                guard token == generation else { return }
                failure = error.localizedDescription
            }
        }
    }

    @MainActor
    private func clearImage() {
        guard !busy else { return }
        let token = beginMutation()
        operationTask = Task { @MainActor in
            defer { finishMutation(token) }
            do {
                let updated = try await GrokMobileBotService(bridge: bridge).updateAgentAvatar(
                    id: agent.id,
                    isGroup: agent.isGroup,
                    clearAvatar: true
                )
                try Task.checkCancellation()
                guard token == generation else { return }
                onSaved(updated)
                onClose()
            } catch is CancellationError {
                return
            } catch {
                guard token == generation else { return }
                failure = error.localizedDescription
            }
        }
    }

    @MainActor
    private func commitCharacter(shape: String, color: String) {
        guard !busy else { return }
        let token = beginMutation()
        operationTask = Task { @MainActor in
            defer { finishMutation(token) }
            do {
                let updated = try await GrokMobileBotService(bridge: bridge).updateAgentAvatar(
                    id: agent.id,
                    isGroup: agent.isGroup,
                    clearAvatar: true,
                    avatarShape: shape,
                    avatarColor: color
                )
                try Task.checkCancellation()
                guard token == generation else { return }
                onSaved(updated)
                onClose()
            } catch is CancellationError {
                return
            } catch {
                guard token == generation else { return }
                failure = error.localizedDescription
            }
        }
    }

    @MainActor
    private func beginMutation() -> Int {
        generation += 1
        let token = generation
        operationTask?.cancel()
        busy = true
        failure = nil
        return token
    }

    @MainActor
    private func finishMutation(_ token: Int) {
        guard token == generation else { return }
        busy = false
        operationTask = nil
    }
}

private extension Color {
    init(hexAvatar value: String) {
        let cleaned = value.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        var parsed: UInt64 = 0
        Scanner(string: cleaned).scanHexInt64(&parsed)
        let red = Double((parsed >> 16) & 0xff) / 255
        let green = Double((parsed >> 8) & 0xff) / 255
        let blue = Double(parsed & 0xff) / 255
        self.init(red: red, green: green, blue: blue)
    }
}

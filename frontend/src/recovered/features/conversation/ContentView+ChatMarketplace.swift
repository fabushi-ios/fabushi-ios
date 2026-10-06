import SwiftUI
import UniformTypeIdentifiers
import UIKit

extension ContentView {
    func chatView(_ conversation: ConversationSummary) -> some View {
        NavigationStack {
            ZStack {
                Color(red: 0.055, green: 0.06, blue: 0.07).ignoresSafeArea()
                VStack(spacing: 0) {
                    if chatSearchPresented {
                        HStack(spacing: 8) {
                            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                            TextField("搜索此聊天", text: $chatSearchQuery).textInputAutocapitalization(.never).autocorrectionDisabled()
                            if !chatSearchQuery.isEmpty { Button { chatSearchQuery = "" } label: { Image(systemName: "xmark.circle.fill") } }
                        }
                        .padding(.horizontal, 12).frame(height: 40).background(Color.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 12)).padding(8)
                    }
                    if let pinnedId = conversation.pinnedMessageIds.last, let pinned = messaging.messagesByConversation[conversation.id]?.first(where: { $0.id == pinnedId }) {
                        HStack(spacing: 9) {
                            Rectangle().fill(Color.accentColor).frame(width: 3, height: 34)
                            VStack(alignment: .leading, spacing: 2) { Text("置顶消息").font(.caption.bold()).foregroundStyle(Color.accentColor); Text(pinned.text).font(.caption).lineLimit(1) }
                            Spacer()
                            Button { Task { await messaging.setMessagePinned(conversationId: conversation.id, messageId: pinned.id, pinned: false) } } label: { Image(systemName: "xmark") }
                        }.padding(.horizontal, 12).padding(.vertical, 6).background(.ultraThinMaterial)
                    }
                    if let typingName = messaging.typingActorByConversation[conversation.id] {
                        Text("\(typingName) 正在输入…")
                            .font(.caption).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity).padding(.vertical, 4)
                    }
                    ScrollViewReader { proxy in
                        ScrollView {
                            LazyVStack(spacing: 8) {
                                ForEach((messaging.messagesByConversation[conversation.id] ?? []).filter { message in
                                    chatSearchQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || message.text.localizedCaseInsensitiveContains(chatSearchQuery)
                                }) { message in
                                    HStack {
                                        if message.isOutgoing { Spacer(minLength: 56) }
                                        VStack(alignment: .trailing, spacing: 4) {
                                            if let origin = message.forwardOrigin {
                                                HStack(spacing: 4) { Image(systemName: "arrowshape.turn.up.right.fill"); Text("转发自 \(origin)") }
                                                    .font(.caption2.bold()).foregroundStyle(Color.accentColor).frame(maxWidth: .infinity, alignment: .leading)
                                            }
                                            if let replyId = message.replyToMessageId, let replied = messaging.messagesByConversation[conversation.id]?.first(where: { $0.id == replyId }) {
                                                VStack(alignment: .leading, spacing: 2) { Text("回复").font(.caption2.bold()); Text(replied.text).font(.caption).lineLimit(2) }
                                                    .frame(maxWidth: .infinity, alignment: .leading).padding(.leading, 8).overlay(alignment: .leading) { Rectangle().fill(Color.accentColor).frame(width: 2) }
                                            }
                                            switch message.contentType {
                                            case "contact":
                                                HStack(spacing: 10) {
                                                    ZStack { Circle().fill(Color.accentColor); Text(String((message.contactName ?? "联").prefix(1))).foregroundStyle(.white).fontWeight(.bold) }.frame(width: 40, height: 40)
                                                    VStack(alignment: .leading, spacing: 2) { Text(message.contactName ?? "联系人").fontWeight(.semibold); Text("联系人").font(.caption).foregroundStyle(.secondary) }
                                                    Spacer()
                                                }.frame(maxWidth: .infinity)
                                            case "location":
                                                VStack(alignment: .leading, spacing: 6) {
                                                    HStack { Image(systemName: "map.fill").foregroundStyle(Color.accentColor); Text("位置").fontWeight(.semibold) }
                                                    if let latitude = message.latitude, let longitude = message.longitude { Text("\(latitude, specifier: "%.6f"), \(longitude, specifier: "%.6f")").font(.caption).foregroundStyle(.secondary) }
                                                }.frame(maxWidth: .infinity, alignment: .leading)
                                            case "poll":
                                                VStack(alignment: .leading, spacing: 7) {
                                                    Text(message.pollQuestion ?? "投票").fontWeight(.semibold)
                                                    ForEach(message.pollOptions) { option in
                                                        Button {
                                                            let chosenIds = Set(message.pollOptions.filter(\.chosen).map(\.id))
                                                            let next: [String]
                                                            if message.pollMultipleAnswers {
                                                                var values = chosenIds
                                                                if option.chosen { values.remove(option.id) } else { values.insert(option.id) }
                                                                next = Array(values)
                                                            } else {
                                                                next = option.chosen ? [] : [option.id]
                                                            }
                                                            Task { await messaging.votePoll(conversationId: conversation.id, messageId: message.id, optionIds: next) }
                                                        } label: {
                                                            HStack(spacing: 7) {
                                                                Image(systemName: option.chosen ? "checkmark.circle.fill" : "circle").foregroundStyle(option.chosen ? Color.accentColor : .secondary)
                                                                Text(option.text).foregroundStyle(.primary)
                                                                Spacer()
                                                                Text("\(option.voterCount)").font(.caption2).foregroundStyle(.secondary)
                                                            }.padding(.vertical, 3)
                                                        }.buttonStyle(.plain)
                                                    }
                                                }.frame(maxWidth: .infinity, alignment: .leading)
                                            case "voice":
                                                Button { Task { await voicePlayback.toggle(message: message, messaging: messaging) } } label: {
                                                    HStack(spacing: 10) {
                                                        Image(systemName: voicePlayback.playingMessageId == message.id ? "stop.circle.fill" : "play.circle.fill").font(.title2).foregroundStyle(Color.accentColor)
                                                        VStack(alignment: .leading) { Text("语音消息").fontWeight(.medium).foregroundStyle(.primary); Text(voicePlayback.playingMessageId == message.id ? "正在播放" : (message.mediaFileName ?? "录音")).font(.caption).foregroundStyle(.secondary) }
                                                        Spacer()
                                                    }
                                                }.buttonStyle(.plain)
                                            case "audio":
                                                HStack(spacing: 10) { Image(systemName: "music.note").font(.title2).foregroundStyle(Color.accentColor); Text(message.mediaFileName ?? "音频"); Spacer() }
                                            case "photo", "video", "document":
                                                Button { mediaViewerMessage = message } label: {
                                                    HStack(spacing: 9) {
                                                        Image(systemName: message.contentType == "photo" ? "photo.fill" : message.contentType == "video" ? "video.fill" : "doc.fill").font(.title2).foregroundStyle(Color.accentColor)
                                                        VStack(alignment: .leading) { Text(message.mediaFileName ?? message.text).fontWeight(.medium).foregroundStyle(.primary); Text(message.contentType == "photo" ? "图片 · 点击查看" : message.contentType == "video" ? "视频 · 点击播放" : "文件 · 点击打开").font(.caption).foregroundStyle(.secondary) }
                                                        Spacer()
                                                    }.frame(maxWidth: .infinity)
                                                }.buttonStyle(.plain)
                                            default:
                                                Text(message.text).foregroundStyle(.primary).frame(maxWidth: .infinity, alignment: .leading)
                                            }
                                            if !message.reactions.isEmpty {
                                                HStack(spacing: 5) {
                                                    ForEach(Array(message.reactions.enumerated()), id: \.offset) { _, reaction in
                                                        Text("\(reaction.reaction) \(reaction.count)").font(.caption2).padding(.horizontal, 7).padding(.vertical, 3)
                                                            .background(reaction.chosenByMe ? Color.accentColor.opacity(0.25) : Color.white.opacity(0.08), in: Capsule())
                                                    }
                                                }.frame(maxWidth: .infinity, alignment: .leading)
                                            }
                                            HStack(spacing: 3) {
                                                if message.isEdited { Text("已编辑").font(.caption2).foregroundStyle(.secondary) }
                                                Text(message.time).font(.caption2).foregroundStyle(.secondary)
                                                if message.isOutgoing {
                                                    Image(systemName: message.deliveryState.lowercased().contains("read") ? "checkmark.checkmark" : message.deliveryState.lowercased().contains("deliver") ? "checkmark.checkmark" : "checkmark")
                                                        .font(.caption2).foregroundStyle(message.deliveryState.lowercased().contains("read") ? .blue : .secondary)
                                                }
                                            }
                                        }
                                        .padding(.horizontal, 11).padding(.vertical, 7)
                                        .background(message.isOutgoing ? Color.accentColor.opacity(0.20) : Color.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 16))
                                        .simultaneousGesture(
                                            DragGesture(minimumDistance: 18)
                                                .onEnded { value in
                                                    guard value.translation.width > 58, abs(value.translation.height) < 70 else { return }
                                                    replyTarget = message
                                                    editingMessage = nil
                                                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                                                }
                                        )
                                        .contextMenu {
                                            Button("回复", systemImage: "arrowshape.turn.up.left") { replyTarget = message; editingMessage = nil }
                                            Button("转发", systemImage: "arrowshape.turn.up.right") { forwardMessage = message }
                                            Button("👍", systemImage: "hand.thumbsup") { Task { await messaging.setReaction(conversationId: conversation.id, messageId: message.id, reaction: "👍", enabled: true) } }
                                            if message.isOutgoing {
                                                Button("编辑", systemImage: "pencil") { editingMessage = message; replyTarget = nil; messageDraft = message.text }
                                            }
                                            Button(message.isPinned ? "取消置顶消息" : "置顶消息", systemImage: "pin") { Task { await messaging.setMessagePinned(conversationId: conversation.id, messageId: message.id, pinned: !message.isPinned) } }
                                            Button("删除", systemImage: "trash", role: .destructive) { Task { await messaging.deleteMessage(conversationId: conversation.id, messageId: message.id) } }
                                        }
                                        if !message.isOutgoing { Spacer(minLength: 56) }
                                    }.padding(.horizontal, 10).id(message.id)
                                }
                            }.padding(.vertical, 12)
                        }
                        .onAppear {
                            guard let target = pendingInitialMessageID,
                                  messaging.messagesByConversation[conversation.id]?.contains(where: { $0.id == target }) == true
                            else { return }
                            pendingInitialMessageID = nil
                            DispatchQueue.main.async { withAnimation { proxy.scrollTo(target, anchor: .center) } }
                        }
                        .onChange(of: messaging.messagesByConversation[conversation.id]?.count ?? 0) { _, _ in
                            if let target = pendingInitialMessageID,
                               messaging.messagesByConversation[conversation.id]?.contains(where: { $0.id == target }) == true {
                                pendingInitialMessageID = nil
                                withAnimation { proxy.scrollTo(target, anchor: .center) }
                            } else if pendingInitialMessageID == nil,
                                      let id = messaging.messagesByConversation[conversation.id]?.last?.id {
                                withAnimation { proxy.scrollTo(id, anchor: .bottom) }
                            }
                        }
                    }
                    if let editingMessage {
                        HStack {
                            Image(systemName: "pencil")
                            VStack(alignment: .leading, spacing: 2) { Text("编辑消息").font(.caption.bold()); Text(editingMessage.text).font(.caption).lineLimit(1) }
                            Spacer()
                            Button { self.editingMessage = nil; messageDraft = "" } label: { Image(systemName: "xmark.circle.fill") }
                        }.padding(.horizontal, 12).padding(.vertical, 6).background(.ultraThinMaterial)
                    } else if let replyTarget {
                        HStack {
                            Image(systemName: "arrowshape.turn.up.left")
                            VStack(alignment: .leading, spacing: 2) { Text("回复").font(.caption.bold()); Text(replyTarget.text).font(.caption).lineLimit(1) }
                            Spacer()
                            Button { self.replyTarget = nil } label: { Image(systemName: "xmark.circle.fill") }
                        }.padding(.horizontal, 12).padding(.vertical, 6).background(.ultraThinMaterial)
                    }
                    if voiceRecorder.isRecording {
                        HStack(spacing: 10) {
                            Circle().fill(Color.red).frame(width: 9, height: 9)
                            Text("正在录音 \(voiceRecorder.elapsedSeconds / 60):\(String(format: "%02d", voiceRecorder.elapsedSeconds % 60))").font(.subheadline).fontWeight(.semibold)
                            Spacer()
                            Button("取消", role: .destructive) { voiceRecorder.cancel() }
                        }.padding(.horizontal, 12).padding(.vertical, 7).background(.ultraThinMaterial)
                    }
                    HStack(alignment: .bottom, spacing: 8) {
                        Menu {
                            Button("照片或视频", systemImage: "photo") { attachmentPickerPresented = true }
                            Button("文件", systemImage: "doc") { attachmentPickerPresented = true }
                            Button("位置", systemImage: "location") { locationSharePresented = true; locationService.requestLocation() }
                            Button("联系人", systemImage: "person.crop.circle") { contactSharePresented = true }
                            Button("投票", systemImage: "chart.bar.fill") { pollQuestion = ""; pollOption1 = ""; pollOption2 = ""; pollOption3 = ""; pollComposerPresented = true }
                        } label: { Image(systemName: "paperclip").font(.title3).frame(width: 36, height: 36) }
                        TextField("消息", text: $messageDraft, axis: .vertical).lineLimit(1...5)
                            .onChange(of: messageDraft) { _, value in
                                Task {
                                    if value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { await messaging.stopTyping(conversation.id) }
                                    else { await messaging.startTyping(conversation.id) }
                                }
                                scheduleDraftSync(conversationId: conversation.id)
                            }
                            .onChange(of: replyTarget?.id) { _, _ in scheduleDraftSync(conversationId: conversation.id) }
                            .padding(.horizontal, 12).padding(.vertical, 9).background(Color.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 18))
                        Button {
                            if messageDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                if voiceRecorder.isRecording {
                                    if let recording = voiceRecorder.stop() {
                                        Task {
                                            do { try await messaging.sendVoice(conversationId: conversation.id, fileName: recording.url.lastPathComponent, mimeType: "audio/mp4", data: recording.data) }
                                            catch { model.message = "语音发送失败：\(error.localizedDescription)" }
                                        }
                                    }
                                } else {
                                    Task { await voiceRecorder.start() }
                                }
                            } else {
                                sendMessage(in: conversation)
                            }
                        } label: {
                            Image(systemName: messageDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? (voiceRecorder.isRecording ? "stop.fill" : "mic.fill") : "arrow.up")
                                .font(.system(size: 18, weight: .bold)).foregroundStyle(.white).frame(width: 38, height: 38).background(voiceRecorder.isRecording ? Color.red : Color.accentColor, in: Circle())
                        }
                        .contextMenu {
                            if !messageDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                Button("静默发送", systemImage: "bell.slash.fill") { sendMessage(in: conversation, silent: true) }
                                Button("1 小时后发送", systemImage: "clock.fill") { sendMessage(in: conversation, scheduledAtMs: Int64(Date().addingTimeInterval(3600).timeIntervalSince1970 * 1000)) }
                                Button("明天上午 9:00", systemImage: "calendar") {
                                    let calendar = Calendar.current
                                    let tomorrow = calendar.date(byAdding: .day, value: 1, to: Date()) ?? Date().addingTimeInterval(86400)
                                    let scheduled = calendar.date(bySettingHour: 9, minute: 0, second: 0, of: tomorrow) ?? tomorrow
                                    sendMessage(in: conversation, scheduledAtMs: Int64(scheduled.timeIntervalSince1970 * 1000))
                                }
                            }
                        }
                    }.padding(.horizontal, 8).padding(.vertical, 7).background(.ultraThinMaterial)
                }
            }
            .navigationTitle("")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Button { conversationInfoPresented = true } label: {
                        VStack(spacing: 1) { Text(conversation.title).font(.headline); Text("\(conversation.participants.count) 位成员").font(.caption2).foregroundStyle(.secondary) }
                    }.buttonStyle(.plain)
                }
                ToolbarItem(placement: .topBarLeading) { Button { chatSearchPresented = false; chatSearchQuery = ""; selectedConversation = nil } label: { Image(systemName: "chevron.left") } }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button(chatSearchPresented ? "关闭搜索" : "搜索", systemImage: "magnifyingglass") { chatSearchPresented.toggle(); if !chatSearchPresented { chatSearchQuery = "" } }
                        Button(conversation.isMuted ? "取消静音" : "静音", systemImage: "speaker.slash") { Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) } }
                        Button(conversation.isPinned ? "取消置顶" : "置顶", systemImage: "pin") { Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) } }
                        Button("标为未读", systemImage: "circle.fill") { Task { await messaging.setMarkedUnread(conversation.id, markedUnread: true) }; selectedConversation = nil }
                        Button("归档", systemImage: "archivebox") { Task { await messaging.setArchived(conversation.id, archived: true) }; selectedConversation = nil }
                    } label: { Image(systemName: "ellipsis.circle") }
                }
            }
        }
        .task(id: conversation.id) {
            let draft = messaging.draftsByConversation[conversation.id]
            messageDraft = draft?.text ?? ""
            replyTarget = draft?.replyToMessageId.flatMap { replyId in messaging.messagesByConversation[conversation.id]?.first(where: { $0.id == replyId }) }
        }
        .sheet(isPresented: $conversationInfoPresented) {
            ConversationInfoView(conversationId: conversation.id, messaging: messaging) { conversationInfoPresented = false }
        }
        .fullScreenCover(item: $mediaViewerMessage) { message in
            MediaViewer(message: message, messaging: messaging) { mediaViewerMessage = nil }
        }
        .sheet(item: $forwardMessage) { message in
            NavigationStack {
                List(messaging.conversations.filter { $0.id != conversation.id && !$0.isArchived }) { destination in
                    Button {
                        Task { await messaging.forwardMessage(sourceConversationId: conversation.id, messageId: message.id, destinationConversationId: destination.id) }
                        forwardMessage = nil
                    } label: { Text(destination.title) }
                }
                .navigationTitle("转发到")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("取消") { forwardMessage = nil } } }
            }
        }
        .fileImporter(isPresented: $attachmentPickerPresented, allowedContentTypes: [.item], allowsMultipleSelection: false) { result in
            guard case let .success(urls) = result, let url = urls.first else { return }
            let accessed = url.startAccessingSecurityScopedResource()
            defer { if accessed { url.stopAccessingSecurityScopedResource() } }
            do {
                let data = try Data(contentsOf: url)
                let values = try? url.resourceValues(forKeys: [.contentTypeKey])
                let mime = values?.contentType?.preferredMIMEType ?? "application/octet-stream"
                Task {
                    do { try await messaging.sendAttachment(conversationId: conversation.id, fileName: url.lastPathComponent, mimeType: mime, data: data) }
                    catch { model.message = "附件发送失败：\(error.localizedDescription)" }
                }
            } catch { model.message = "读取附件失败：\(error.localizedDescription)" }
        }
        .sheet(isPresented: $locationSharePresented) {
            NavigationStack {
                VStack(spacing: 18) {
                    if locationService.loading { ProgressView("正在获取位置…") }
                    else if let coordinate = locationService.coordinate {
                        Image(systemName: "location.circle.fill").font(.system(size: 52)).foregroundStyle(Color.accentColor)
                        Text("纬度 \(coordinate.latitude, specifier: "%.6f")")
                        Text("经度 \(coordinate.longitude, specifier: "%.6f")")
                        Button("发送此位置") {
                            Task {
                                do { try await messaging.sendLocation(conversationId: conversation.id, latitude: coordinate.latitude, longitude: coordinate.longitude); locationSharePresented = false }
                                catch { model.message = "位置发送失败：\(error.localizedDescription)" }
                            }
                        }.buttonStyle(.borderedProminent)
                    } else {
                        ContentUnavailableView("无法获取位置", systemImage: "location.slash", description: Text(locationService.errorMessage ?? "请检查位置权限"))
                        Button("重试") { locationService.requestLocation() }
                    }
                }.padding().navigationTitle("发送位置")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("取消") { locationSharePresented = false } } }
            }
        }
        .sheet(isPresented: $contactSharePresented) {
            NavigationStack {
                List(messaging.contacts) { contact in
                    Button {
                        Task {
                            do { try await messaging.sendContact(conversationId: conversation.id, contact: contact); contactSharePresented = false }
                            catch { model.message = "联系人发送失败：\(error.localizedDescription)" }
                        }
                    } label: { Text(contact.displayName) }
                }
                .navigationTitle("发送联系人")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("取消") { contactSharePresented = false } } }
            }
        }
        .sheet(isPresented: $pollComposerPresented) {
            NavigationStack {
                Form {
                    Section("问题") { TextField("输入问题", text: $pollQuestion) }
                    Section("选项") {
                        TextField("选项 1", text: $pollOption1)
                        TextField("选项 2", text: $pollOption2)
                        TextField("选项 3（可选）", text: $pollOption3)
                    }
                }
                .navigationTitle("新建投票")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("取消") { pollComposerPresented = false } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("发送") {
                            let question = pollQuestion; let options = [pollOption1, pollOption2, pollOption3]
                            Task {
                                do { try await messaging.sendPoll(conversationId: conversation.id, question: question, options: options); pollComposerPresented = false }
                                catch { model.message = "投票发送失败：\(error.localizedDescription)" }
                            }
                        }.disabled(pollQuestion.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || pollOption1.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || pollOption2.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
            }
        }
    }

    func sendMessage(in conversation: ConversationSummary, silent: Bool = false, scheduledAtMs: Int64? = nil) {
        let text = messageDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let edit = editingMessage
        let reply = replyTarget
        draftSyncTask?.cancel()
        messageDraft = ""
        Task { await messaging.stopTyping(conversation.id); await messaging.setDraft(conversationId: conversation.id, text: "", replyToMessageId: nil) }
        editingMessage = nil
        replyTarget = nil
        Task {
            do {
                if let edit { try await messaging.editText(conversationId: conversation.id, messageId: edit.id, text: text) }
                else { try await messaging.sendText(conversationId: conversation.id, text: text, replyToMessageId: reply?.id, silent: silent, scheduledAtMs: scheduledAtMs) }
            } catch { model.message = "发送失败：\(error.localizedDescription)" }
        }
    }

    func scheduleDraftSync(conversationId: String) {
        guard editingMessage == nil else { return }
        draftSyncTask?.cancel()
        let text = messageDraft
        let replyId = replyTarget?.id
        draftSyncTask = Task {
            try? await Task.sleep(for: .milliseconds(350))
            guard !Task.isCancelled else { return }
            await messaging.setDraft(conversationId: conversationId, text: text, replyToMessageId: replyId)
        }
    }

    var avatar: some View {
        ZStack {
            Circle().fill(Color.white.opacity(0.08))
            Text("✦").font(.system(size: 22, weight: .bold)).foregroundStyle(.orange)
        }
    }

    var marketplaceView: some View {
        NavigationStack {
            List {
                Section {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("MAHAYANA RUST HOST")
                            .font(.caption.bold())
                            .foregroundStyle(.secondary)
                        HStack {
                            Text("全球法布施").font(.largeTitle.bold())
                            Spacer()
                            Text("SwiftUI · Rust")
                                .font(.caption.bold())
                                .accessibilityIdentifier("runtime-badge")
                        }
                        .accessibilityElement(children: .contain)
                    }
                    .accessibilityElement(children: .contain)
                }

                Section("本地插件市场") {
                    Text("iOS 主壳使用 SwiftUI；MiniApp 使用受控 WebMCP Surface；代码从 GitHub 固定版本拉取并由共享 Mahayana Rust Host 校验、安装、更新。")
                    TextField("搜索插件", text: $model.query)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("marketplace-search")
                    Button("搜索") {
                        Task { await model.refresh() }
                    }
                    .disabled(model.loading)
                    .accessibilityIdentifier("marketplace-search-submit")
                }

                Section("Host 状态") {
                    HStack(spacing: 12) {
                        if model.loading { ProgressView() }
                        Text(model.message)
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("host-status")

                    if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                        Text(featureHostSmokeStatus)
                            .accessibilityIdentifier("feature-host-smoke")
                    }
                }

                if let permission = model.permissionRequest {
                    Section("插件权限") {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(permission.pluginId)
                                .font(.headline)
                            Text("此插件请求以下权限。只有明确批准后，Fabushi 才会把这些权限授予已安装插件。")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            ForEach(permission.permissions, id: \.self) { capability in
                                Label(capability, systemImage: "lock.shield")
                                    .font(.caption)
                            }
                            HStack(spacing: 8) {
                                Button("拒绝") {
                                    model.denyPermissions()
                                }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier("plugin-permission-deny-\(permission.pluginId)")

                                Button("批准并继续") {
                                    Task { await model.approvePermissions() }
                                }
                                .buttonStyle(.borderedProminent)
                                .accessibilityIdentifier("plugin-permission-approve-\(permission.pluginId)")
                            }
                        }
                        .accessibilityElement(children: .contain)
                        .accessibilityIdentifier("plugin-permission-request-\(permission.pluginId)")
                    }
                }

                Section("MCP 连接器") {
                    if !model.loggedIn {
                        Text("登录 Fabushi 后可管理当前账号的 MCP 连接器与工具。")
                            .foregroundStyle(.secondary)
                    } else {
                        HStack {
                            Text("服务器与工具状态由 Coordinator / Rust Host 管理。")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Spacer()
                            if model.mcpLoading { ProgressView() }
                            Button("刷新") {
                                Task { await model.refreshMcpServers() }
                            }
                            .disabled(model.mcpLoading)
                            .accessibilityIdentifier("mcp-refresh")
                        }

                        if let error = model.mcpError {
                            Text(error)
                                .font(.caption)
                                .foregroundStyle(.red)
                                .accessibilityIdentifier("mcp-error")
                        }

                        if model.mcpServers.isEmpty && !model.mcpLoading {
                            Text("当前账号没有可管理的 MCP 服务器。")
                                .foregroundStyle(.secondary)
                        }

                        ForEach(model.mcpServers) { server in
                            VStack(alignment: .leading, spacing: 8) {
                                HStack {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(server.name).font(.headline)
                                        Text("\(server.transport.uppercased()) · \(server.status) · \(server.toolCount) 个启用工具")
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                        if let detail = server.statusDetail, !detail.isEmpty {
                                            Text(detail).font(.caption2).foregroundStyle(.secondary)
                                        }
                                    }
                                    Spacer()
                                    Button(model.mcpLoadingServerId == server.id ? "读取中…" : "工具") {
                                        Task { await model.loadMcpTools(serverId: server.id) }
                                    }
                                    .disabled(model.mcpLoadingServerId == server.id)
                                }
                                .accessibilityElement(children: .contain)
                                .accessibilityIdentifier("mcp-server-\(server.id)")

                                if let tools = model.mcpToolsByServerId[server.id] {
                                    if tools.isEmpty {
                                        Text("此服务器没有报告可管理工具。")
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                    ForEach(tools) { tool in
                                        HStack(alignment: .top, spacing: 10) {
                                            VStack(alignment: .leading, spacing: 2) {
                                                Text(tool.title ?? tool.name)
                                                    .font(.subheadline.weight(.medium))
                                                if let description = tool.description, !description.isEmpty {
                                                    Text(description)
                                                        .font(.caption)
                                                        .foregroundStyle(.secondary)
                                                }
                                            }
                                            Spacer()
                                            Toggle(
                                                "启用",
                                                isOn: Binding(
                                                    get: { !tool.isDisabled },
                                                    set: { enabled in
                                                        Task {
                                                            await model.setMcpToolEnabled(
                                                                serverId: server.id,
                                                                toolName: tool.name,
                                                                enabled: enabled
                                                            )
                                                        }
                                                    }
                                                )
                                            )
                                            .labelsHidden()
                                            .disabled(model.mcpMutatingToolKey == "\(server.id):\(tool.name)")
                                            .accessibilityIdentifier("mcp-tool-\(server.id)-\(tool.name)")
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Section("插件") {
                    if model.plugins.isEmpty && !model.loading {
                        Text("没有匹配的 iOS 插件。")
                    }
                    ForEach(model.plugins) { plugin in
                        VStack(alignment: .leading, spacing: 8) {
                            Group {
                                Text(plugin.pluginId).font(.caption.monospaced()).foregroundStyle(.secondary)
                                Text(plugin.displayName).font(.headline)
                                Text(plugin.description).foregroundStyle(.secondary)
                                if let version = plugin.latestVersion {
                                    Text("\(version) · GitHub \(plugin.sourceRef?.prefix(9) ?? "待确认")")
                                        .font(.caption)
                                }
                            }
                            .accessibilityElement(children: .combine)
                            .accessibilityIdentifier("plugin-\(plugin.pluginId)")

                            HStack(spacing: 8) {
                                Button("打开 WebMCP") {
                                    openedMiniApp = plugin
                                }
                                .accessibilityIdentifier("open-\(plugin.pluginId)")

                                Button(model.installingPluginId == plugin.pluginId ? "处理中…" : "安装 / 更新") {
                                    Task { await model.install(plugin) }
                                }
                                .disabled(plugin.latestVersion == nil || model.installingPluginId != nil)
                                .accessibilityIdentifier("install-\(plugin.pluginId)")
                            }
                        }
                    }
                }
            }
            .navigationTitle("法布施")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("消息") { destination = .home }
                }
            }
            .refreshable {
                await model.refresh()
                if model.loggedIn {
                    await model.refreshMcpServers()
                }
            }
            .task {
                if model.plugins.isEmpty {
                    await model.refresh()
                }
                if model.loggedIn && model.mcpServers.isEmpty {
                    await model.refreshMcpServers()
                }
            }
        }
    }
}

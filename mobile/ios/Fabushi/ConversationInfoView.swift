import SwiftUI

struct ConversationInfoView: View {
    let conversationId: String
    @Bindable var messaging: MessagingModel
    let onClose: () -> Void

    @State private var editing = false
    @State private var titleDraft = ""
    @State private var descriptionDraft = ""

    private var conversation: ConversationSummary? {
        messaging.conversations.first { $0.id == conversationId }
    }

    var body: some View {
        NavigationStack {
            Group {
                if let conversation {
                    Form {
                        Section {
                            HStack(spacing: 14) {
                                ZStack {
                                    Circle().fill(Color.accentColor)
                                    Text(conversation.badge).font(.title2.bold()).foregroundStyle(.white)
                                }.frame(width: 64, height: 64)
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(conversation.title).font(.title3.bold())
                                    Text("\(conversation.participants.count) 位成员 · \(conversation.kind.label)").font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }

                        if canManage(conversation) {
                            Section("资料") {
                                if editing {
                                    TextField("名称", text: $titleDraft)
                                    TextField("描述", text: $descriptionDraft, axis: .vertical).lineLimit(2...5)
                                    Button("保存") {
                                        let title = titleDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                                        guard !title.isEmpty else { return }
                                        Task { await messaging.updateConversationInfo(conversationId: conversation.id, title: title, description: descriptionDraft) }
                                        editing = false
                                    }
                                } else {
                                    LabeledContent("名称", value: conversation.title)
                                    if !conversation.description.isEmpty { Text(conversation.description).foregroundStyle(.secondary) }
                                    Button("编辑资料") {
                                        titleDraft = conversation.title
                                        descriptionDraft = conversation.description
                                        editing = true
                                    }
                                }
                            }
                        } else if !conversation.description.isEmpty {
                            Section("简介") { Text(conversation.description) }
                        }

                        Section("成员") {
                            ForEach(conversation.participants) { participant in
                                HStack(spacing: 12) {
                                    ZStack {
                                        Circle().fill(Color.accentColor.opacity(0.8))
                                        Text(String(displayName(participant.actorId).prefix(1)).uppercased()).foregroundStyle(.white).fontWeight(.bold)
                                    }.frame(width: 40, height: 40)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(displayName(participant.actorId))
                                        Text(roleLabel(participant.role)).font(.caption).foregroundStyle(.secondary)
                                    }
                                    Spacer()
                                    if canManageParticipant(conversation, participant: participant) {
                                        Menu {
                                            if currentRole(conversation) == "owner" && participant.actorId != conversation.ownerId {
                                                Button(participant.role == "admin" ? "设为普通成员" : "设为管理员") {
                                                    Task { await messaging.setConversationParticipant(conversation: conversation, actorId: participant.actorId, role: participant.role == "admin" ? "member" : "admin") }
                                                }
                                            }
                                            Button("移除成员", role: .destructive) {
                                                Task { await messaging.removeConversationParticipant(conversationId: conversation.id, actorId: participant.actorId) }
                                            }
                                        } label: { Image(systemName: "ellipsis.circle") }
                                    }
                                }
                            }
                        }

                        if canManage(conversation) {
                            let available = messaging.contacts.filter { contact in !conversation.participants.contains(where: { $0.actorId == contact.id }) }
                            if !available.isEmpty {
                                Section("添加成员") {
                                    ForEach(available) { contact in
                                        Button {
                                            Task { await messaging.setConversationParticipant(conversation: conversation, actorId: contact.id, role: "member") }
                                        } label: {
                                            Label(contact.displayName, systemImage: "person.badge.plus")
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    ContentUnavailableView("会话不可用", systemImage: "person.3.fill", description: Text("你可能已不再是该会话成员。"))
                }
            }
            .navigationTitle("详情")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成", action: onClose) } }
        }
    }

    private func currentRole(_ conversation: ConversationSummary) -> String? {
        conversation.participants.first(where: { $0.actorId == messaging.currentActorId })?.role
    }

    private func canManage(_ conversation: ConversationSummary) -> Bool {
        guard conversation.kind == .group || conversation.kind == .channel else { return false }
        guard let role = currentRole(conversation) else { return false }
        return role == "owner" || role == "admin"
    }

    private func canManageParticipant(_ conversation: ConversationSummary, participant: ConversationParticipant) -> Bool {
        guard participant.actorId != messaging.currentActorId, participant.actorId != conversation.ownerId else { return false }
        switch currentRole(conversation) {
        case "owner": return true
        case "admin": return participant.role == "member" || participant.role == "restricted"
        default: return false
        }
    }

    private func displayName(_ actorId: String) -> String {
        if actorId == messaging.currentActorId { return "我" }
        return messaging.contacts.first(where: { $0.id == actorId })?.displayName ?? actorId
    }

    private func roleLabel(_ role: String) -> String {
        switch role {
        case "owner": "群主"
        case "admin": "管理员"
        case "restricted": "受限成员"
        default: "成员"
        }
    }
}

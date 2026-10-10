import SwiftUI
import UniformTypeIdentifiers
import UIKit

extension ContentView {
    var agentChatView: some View {
        NavigationStack {
            ZStack {
                Color(red: 0.055, green: 0.06, blue: 0.07).ignoresSafeArea()
                VStack(spacing: 0) {
                    ScrollViewReader { proxy in
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 10) {
                                if model.chatMessages.isEmpty {
                                    VStack(spacing: 10) {
                                        avatar.frame(width: 68, height: 68)
                                        Text("大乘助手").font(.title2.bold()).foregroundStyle(.white)
                                        Text("这是 Mahayana 多步骤智能体。真实的模型路由、工具调用和每一步工作会逐条显示在这里。")
                                            .multilineTextAlignment(.center).font(.subheadline).foregroundStyle(.white.opacity(0.65))
                                    }
                                    .frame(maxWidth: .infinity).padding(.top, 90).padding(.horizontal, 26)
                                }
                                ForEach(model.chatMessages) { entry in
                                    agentChatEntry(entry)
                                        .id(entry.id)
                                }
                            }
                            .padding(.horizontal, 14).padding(.vertical, 18)
                        }
                        .onChange(of: model.chatMessages.count) { _, _ in
                            if let last = model.chatMessages.last { withAnimation(.easeOut(duration: 0.18)) { proxy.scrollTo(last.id, anchor: .bottom) } }
                        }
                    }

                    HStack(spacing: 9) {
                        TextField("消息大乘助手", text: $model.chatDraft, axis: .vertical)
                            .lineLimit(1...5).textFieldStyle(.plain).foregroundStyle(.white)
                            .padding(.horizontal, 13).padding(.vertical, 10)
                            .background(Color.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 18))
                            .onSubmit { if !model.chatBusy { Task { await model.sendChat() } } }
                        if model.chatBusy {
                            Button { Task { await model.stopChat() } } label: {
                                Image(systemName: "stop.fill").font(.system(size: 15, weight: .bold)).foregroundStyle(.white)
                                    .frame(width: 38, height: 38).background(Color.red, in: Circle())
                            }.accessibilityIdentifier("mahayana-stop")
                        } else {
                            Button { Task { await model.sendChat() } } label: {
                                Image(systemName: "arrow.up").font(.system(size: 18, weight: .bold)).foregroundStyle(.white)
                                    .frame(width: 38, height: 38).background(Color.accentColor, in: Circle())
                            }.disabled(model.chatDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                                .accessibilityIdentifier("mahayana-send")
                        }
                    }
                    .padding(.horizontal, 9).padding(.vertical, 8).background(.ultraThinMaterial)
                }
            }
            .navigationTitle("大乘助手")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("关闭") { agentChatPresented = false }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Text(model.chatBusy ? "正在工作" : "Mahayana").font(.caption).foregroundStyle(model.chatBusy ? .orange : .secondary)
                }
            }
        }
        .accessibilityIdentifier("mahayana-agent-chat")
    }

    @ViewBuilder
    func agentChatEntry(_ entry: MobileChatMessage) -> some View {
        if entry.kind == .thinking {
            HStack(spacing: 9) {
                avatar.frame(width: 30, height: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.actionTitle ?? "正在思考").font(.subheadline.weight(.semibold)).foregroundStyle(.white)
                    HStack(spacing: 5) { ProgressView().controlSize(.mini).tint(.orange); Text("Mahayana 正在处理…").font(.caption).foregroundStyle(.secondary) }
                }
                Spacer()
            }
            .padding(10).background(Color.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 13))
            .overlay(RoundedRectangle(cornerRadius: 13).stroke(Color.orange.opacity(0.24)))
            .accessibilityIdentifier("mahayana-thinking")
        } else if entry.kind == .handoff {
            VStack(alignment: .leading, spacing: 9) {
                HStack(spacing: 8) {
                    avatar.frame(width: 28, height: 28)
                    Text("需要你完成一步").font(.subheadline.weight(.semibold)).foregroundStyle(.white)
                }
                Text(entry.text).font(.subheadline).foregroundStyle(.white)
                if entry.actionStatus == "pending" {
                    HStack(spacing: 8) {
                        Button("已完成，继续") { Task { await model.resolveBoxHandoff(entry, resolution: "completed") } }
                            .buttonStyle(.borderedProminent)
                            .accessibilityIdentifier("mahayana-handoff-complete")
                        Button("无法完成") { Task { await model.resolveBoxHandoff(entry, resolution: "dismissed") } }
                            .buttonStyle(.bordered)
                            .accessibilityIdentifier("mahayana-handoff-dismiss")
                    }
                } else {
                    Text(entry.actionStatus == "completed" ? "已归还控制" : "接管已结束").font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(10).background(Color.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 13))
            .overlay(RoundedRectangle(cornerRadius: 13).stroke(Color.orange.opacity(0.24)))
            .accessibilityIdentifier("mahayana-handoff")
        } else if entry.kind == .action {
            HStack(spacing: 8) {
                avatar.frame(width: 25, height: 25)
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.actionTitle ?? "助手动作").font(.caption.weight(.semibold)).foregroundStyle(.white)
                    if let detail = entry.actionDetail, !detail.isEmpty { Text(detail).font(.caption2).foregroundStyle(.secondary).lineLimit(2) }
                }
                Spacer()
                Text(entry.actionStatus == "failed" ? "失败" : entry.actionStatus == "running" ? "进行中" : "完成")
                    .font(.caption2).foregroundStyle(entry.actionStatus == "failed" ? .red : entry.actionStatus == "running" ? .orange : .green)
            }
            .padding(.horizontal, 10).padding(.vertical, 8).background(Color.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 11))
            .accessibilityIdentifier("mahayana-step")
        } else if entry.role == .user {
            HStack { Spacer(minLength: 44); Text(entry.text).foregroundStyle(.white).padding(.horizontal, 13).padding(.vertical, 10).background(Color.black, in: RoundedRectangle(cornerRadius: 16)) }
        } else {
            HStack(alignment: .top, spacing: 8) {
                avatar.frame(width: 28, height: 28)
                Text(entry.text).foregroundStyle(.white).padding(.horizontal, 13).padding(.vertical, 10).background(Color.white.opacity(0.12), in: RoundedRectangle(cornerRadius: 16))
                Spacer(minLength: 28)
            }
        }
    }
}

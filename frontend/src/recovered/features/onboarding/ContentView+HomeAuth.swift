import SwiftUI
import UniformTypeIdentifiers
import UIKit

extension ContentView {
    var homeView: some View {
        NavigationStack {
            ZStack(alignment: .bottomTrailing) {
                Color(red: 0.043, green: 0.043, blue: 0.047).ignoresSafeArea()
                ScrollView {
                    LazyVStack(spacing: 0) {
                        if isSearching {
                            HStack(spacing: 8) {
                                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                                TextField("搜索", text: $homeQuery)
                                    .textInputAutocapitalization(.never)
                                    .autocorrectionDisabled()
                                    .foregroundStyle(.white)
                                if !homeQuery.isEmpty {
                                    Button { homeQuery = "" } label: { Image(systemName: "xmark.circle.fill") }
                                        .foregroundStyle(.secondary)
                                }
                            }
                            .padding(.horizontal, 12)
                            .frame(height: 40)
                            .background(Color.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .accessibilityIdentifier("home-search-field")
                        }

                        if !archivedConversations.isEmpty && homeQuery.isEmpty {
                            Button { activeSection = .archive } label: {
                                HStack(spacing: 12) {
                                    Image(systemName: "archivebox.fill").foregroundStyle(.secondary)
                                    Text("已归档").font(.headline).foregroundStyle(.primary)
                                    Spacer()
                                    Text("\(archivedConversations.count)").foregroundStyle(.secondary)
                                }
                                .padding(.horizontal, 16).frame(height: 48)
                            }.buttonStyle(.plain)
                        }

                        if homeQuery.isEmpty {
                            Button { agentChatPresented = true } label: {
                                HStack(spacing: 12) {
                                    avatar.frame(width: 54, height: 54)
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text("大乘助手").font(.system(size: 17, weight: .semibold)).foregroundStyle(.primary)
                                        Text("Mahayana 多步骤智能体 · 实时工作流").font(.system(size: 15)).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                    Spacer()
                                    Image(systemName: "chevron.right").font(.caption).foregroundStyle(.secondary)
                                }
                                .padding(.horizontal, 14).padding(.vertical, 8)
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("mahayana-agent-entry")
                        }

                        ForEach(filteredConversations) { conversation in
                            conversationRow(conversation)
                                .swipeActions(edge: .leading, allowsFullSwipe: true) {
                                    Button {
                                        Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) }
                                    } label: { Label(conversation.isPinned ? "取消置顶" : "置顶", systemImage: "pin.fill") }
                                    .tint(.orange)
                                }
                                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                    Button {
                                        Task { await messaging.setArchived(conversation.id, archived: true) }
                                    } label: { Label("归档", systemImage: "archivebox.fill") }
                                    .tint(.blue)
                                    Button {
                                        Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) }
                                    } label: { Label(conversation.isMuted ? "取消静音" : "静音", systemImage: "speaker.slash.fill") }
                                    .tint(.gray)
                                }
                        }

                        if filteredConversations.isEmpty {
                            VStack(spacing: 10) {
                                Image(systemName: homeQuery.isEmpty ? "bubble.left.and.bubble.right" : "magnifyingglass")
                                    .font(.system(size: 34))
                                Text(homeQuery.isEmpty ? "还没有对话" : "没有找到结果").font(.headline)
                                Text(homeQuery.isEmpty ? "点击写消息按钮开始聊天" : "尝试其他关键词").font(.subheadline)
                            }
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 90)
                        }

                        if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                            Text(featureHostSmokeStatus).font(.caption2).foregroundStyle(.clear)
                                .accessibilityIdentifier("feature-host-smoke")
                        }
                    }
                }
                .accessibilityIdentifier("conversation-list")

            }
            .navigationTitle("聊天")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { profileMenuPresented = true } label: {
                        avatar.frame(width: 34, height: 34)
                    }
                    .accessibilityLabel("个人菜单")
                    .accessibilityIdentifier("profile-avatar")
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button {
                        withAnimation(.easeInOut(duration: 0.18)) { isSearching.toggle() }
                        if !isSearching { homeQuery = "" }
                    } label: { Image(systemName: isSearching ? "xmark" : "magnifyingglass") }
                    .accessibilityIdentifier("home-search-button")
                    Button { composeMenuPresented = true } label: { Image(systemName: "square.and.pencil") }
                        .accessibilityIdentifier("home-add-button")
                }
            }
        }
        .confirmationDialog("新建", isPresented: $composeMenuPresented, titleVisibility: .visible) {
            Button("新消息") { activeSection = .contacts }
            Button("新建群组") { startCompose(.group) }
            Button("新建频道") { startCompose(.channel) }
            Button("联系人分组") { contactGroupsPresented = true }
            Button("取消", role: .cancel) {}
        }
        .sheet(isPresented: $profileMenuPresented) {
            AccountMenuView(
                model: model,
                avatar: AnyView(avatar),
                conversationId: selectedConversation?.id,
                onClose: { profileMenuPresented = false },
                onRequestSignOut: { signOutConfirmationPresented = true },
                onOpenRemoteComputer: {
                    profileMenuPresented = false
                    destination = .remoteComputer
                },
                onOpenMarketplace: {
                    profileMenuPresented = false
                    destination = .marketplace
                },
                onOpenSection: { section in
                    profileMenuPresented = false
                    handleSection(section)
                }
            )
            .sheet(isPresented: $signOutConfirmationPresented) {
                AccountSignOutDialogView(
                    model: model,
                    onCancel: { signOutConfirmationPresented = false },
                    onSignedOut: {
                        signOutConfirmationPresented = false
                        profileMenuPresented = false
                    }
                )
            }
        }
        .sheet(item: $composeKind) { kind in composeSheet(kind) }
        .sheet(isPresented: $contactGroupsPresented) { simpleSectionSheet(title: "联系人分组", symbol: "folder.fill") }
        .sheet(item: $activeSection) { section in sectionSheet(section) }
        .fullScreenCover(item: $selectedConversation) { conversation in chatView(conversation) }
        .fullScreenCover(isPresented: $agentChatPresented) { agentChatView }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("app-shell")
        .task { await messaging.refresh() }
    }

    var onboardingView: some View {
        ZStack {
            Color(red: 0.043, green: 0.043, blue: 0.047).ignoresSafeArea()
            VStack(spacing: 22) {
                Spacer()
                avatar.frame(width: 92, height: 92)
                Text("欢迎来到法布施").font(.largeTitle.bold()).foregroundStyle(.white)
                Text("统一的聊天、插件和 Mahayana 多步骤智能体工作台。每一步工作都会像消息一样实时出现。")
                    .multilineTextAlignment(.center).foregroundStyle(.white.opacity(0.72)).padding(.horizontal, 32)
                HStack(spacing: 7) {
                    ForEach(0..<3, id: \.self) { index in
                        Capsule().fill(index <= model.onboardingStep ? Color.accentColor : Color.white.opacity(0.18)).frame(width: 24, height: 5)
                    }
                }
                Spacer()
                Button(model.onboardingStep >= 2 ? "开始使用" : "继续") { model.advanceOnboarding() }
                    .buttonStyle(.borderedProminent).controlSize(.large).accessibilityIdentifier("mobile-onboarding-continue")
                Button("跳过介绍") { model.onboardingStep = 3; UserDefaults.standard.set(true, forKey: "fabushi.mobile.onboarding-complete.v1") }
                    .font(.footnote).foregroundStyle(.secondary).accessibilityIdentifier("mobile-onboarding-skip")
                if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                    Text(featureHostSmokeStatus).font(.caption2).foregroundStyle(.clear).accessibilityIdentifier("feature-host-smoke")
                }
            }
            .padding(24)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-onboarding")
    }

    var authLoadingView: some View {
        ZStack {
            Color(red: 0.985, green: 0.985, blue: 0.978).ignoresSafeArea()
            VStack(spacing: 16) {
                avatar.frame(width: 70, height: 70)
                ProgressView().tint(.black)
                Text("正在连接 Fabushi…").foregroundStyle(Color.black.opacity(0.68))
                if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                    Text(featureHostSmokeStatus).font(.caption2).foregroundStyle(.clear).accessibilityIdentifier("feature-host-smoke")
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-auth-loading")
    }

    var loginView: some View {
        ZStack {
            Color(red: 0.985, green: 0.985, blue: 0.978).ignoresSafeArea()
            GeometryReader { proxy in
                let width = proxy.size.width
                let height = proxy.size.height
                Group {
                    LoginBlob(color: Color(red: 1.00, green: 0.57, blue: 0.04), width: 92, height: 92, rotation: -8)
                        .position(x: width * 0.25, y: height * 0.16)
                    LoginBlob(color: Color(red: 0.53, green: 0.30, blue: 1.00), width: 58, height: 66, rotation: 12)
                        .position(x: width * 0.69, y: height * 0.17)
                    LoginBlob(color: Color(red: 0.00, green: 0.78, blue: 0.45), width: 82, height: 68, rotation: 5)
                        .position(x: width * 1.00, y: height * 0.32)
                    LoginBlob(color: Color(red: 0.08, green: 0.49, blue: 0.98), width: 84, height: 66, rotation: 7)
                        .position(x: width * 0.00, y: height * 0.37)
                    LoginBlob(color: Color(red: 1.00, green: 0.14, blue: 0.26), width: 92, height: 82, rotation: 8)
                        .position(x: width * 0.56, y: height * 0.79)
                    LoginBlob(color: Color(red: 0.00, green: 0.72, blue: 0.65), width: 70, height: 70, rotation: -9)
                        .position(x: width * 0.18, y: height * 0.76)
                    LoginBlob(color: Color(red: 0.64, green: 0.40, blue: 0.20), width: 62, height: 62, rotation: 17)
                        .position(x: width * 0.91, y: height * 0.68)
                }
            }
            .ignoresSafeArea()

            VStack(spacing: 0) {
                Spacer()
                VStack(spacing: 17) {
                    Text("Fabushi")
                        .font(.system(size: 48, weight: .bold, design: .rounded))
                        .tracking(-1.5)
                        .foregroundStyle(.black)
                    Text("你的常驻智能体团队，持续完成工作。")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(Color.black.opacity(0.42))
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 40)
                }
                .padding(.bottom, 74)
                Spacer()

                AccountSignInStatusView(model: model)

                if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                    Text(featureHostSmokeStatus).font(.caption2).foregroundStyle(.clear).accessibilityIdentifier("feature-host-smoke")
                }
            }
            .padding(.horizontal, 28)
            .padding(.bottom, 18)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-login")
    }
}

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
            Color(red: 0.985, green: 0.985, blue: 0.978).ignoresSafeArea()
            VStack(spacing: 20) {
                onboardingStepContent
                if model.signedInOnboardingStep != .handOff {
                    onboardingFooter
                }
                if model.signedInOnboardingStep != .completed {
                    Button("跳过介绍") {
                        cancelOnboardingOperation()
                        model.skipSignedInOnboarding()
                    }
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .disabled(onboardingCreateBusy)
                    .accessibilityIdentifier("mobile-onboarding-skip")
                }
                if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                    Text(featureHostSmokeStatus)
                        .font(.caption2)
                        .foregroundStyle(.clear)
                        .accessibilityIdentifier("feature-host-smoke")
                }
            }
            .padding(24)
        }
        .task(id: model.settingsNoticeAccountKey) {
            synchronizeOnboardingAccountScope()
        }
        .onDisappear {
            if model.signedInOnboardingStep != .completed {
                cancelOnboardingOperation()
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-onboarding")
    }

    @ViewBuilder
    var onboardingStepContent: some View {
        switch model.signedInOnboardingStep {
        case .meet:
            Spacer()
            avatar.frame(width: 92, height: 92)
            Text("Meet Fabushi")
                .font(.largeTitle.bold())
                .accessibilityIdentifier("mobile-onboarding-meet-title")
            Text(MobileSignedInOnboardingContract.meetText)
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 24)
            Spacer()
        case .computerDemo:
            Spacer()
            Text("Fabushi has its own computer and works just like you")
                .font(.title2.bold())
                .multilineTextAlignment(.center)
                .accessibilityIdentifier("mobile-onboarding-computer-title")
            ZStack(alignment: .topLeading) {
                RoundedRectangle(cornerRadius: 22)
                    .fill(Color.black.opacity(0.88))
                VStack(alignment: .leading, spacing: 10) {
                    HStack(spacing: 6) {
                        Circle().fill(.red).frame(width: 9, height: 9)
                        Circle().fill(.yellow).frame(width: 9, height: 9)
                        Circle().fill(.green).frame(width: 9, height: 9)
                    }
                    RoundedRectangle(cornerRadius: 12)
                        .fill(Color.white.opacity(0.10))
                        .overlay {
                            VStack(spacing: 9) {
                                Label("Research", systemImage: "magnifyingglass")
                                Label("Draft", systemImage: "doc.text")
                                Label("Send", systemImage: "paperplane")
                            }
                            .foregroundStyle(.white)
                        }
                }
                .padding(18)
            }
            .frame(maxWidth: 420, maxHeight: 250)
            .accessibilityLabel("Fabushi computer demo")
            .accessibilityIdentifier("mobile-onboarding-computer-demo")
            Spacer()
        case .jobs:
            Spacer()
            Text("Give each Bot a job")
                .font(.largeTitle.bold())
                .multilineTextAlignment(.center)
                .accessibilityIdentifier("mobile-onboarding-jobs-title")
            VStack(spacing: 12) {
                ForEach(MobileSignedInOnboardingContract.jobs, id: \.self) { job in
                    Label(job, systemImage: "sparkles")
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding()
                        .background(Color.secondary.opacity(0.10), in: RoundedRectangle(cornerRadius: 16))
                }
            }
            .frame(maxWidth: 440)
            Spacer()
        case .tools:
            Text("What do you use every day?")
                .font(.largeTitle.bold())
                .multilineTextAlignment(.center)
                .accessibilityIdentifier("mobile-onboarding-tools-title")
            TextField("Search", text: $onboardingToolQuery)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("mobile-onboarding-tool-search")
            ScrollView {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 120), spacing: 10)], spacing: 10) {
                    ForEach(MobileOnboardingTool.filtered(onboardingToolQuery)) { tool in
                        let selected = onboardingDailyTools.contains(tool.label)
                        Button {
                            if selected {
                                onboardingDailyTools.removeAll { $0 == tool.label }
                            } else {
                                onboardingDailyTools.append(tool.label)
                            }
                        } label: {
                            HStack {
                                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                                Text(tool.label).lineLimit(1)
                            }
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 10)
                        }
                        .buttonStyle(.bordered)
                        .accessibilityValue(selected ? "Selected" : "Not selected")
                        .accessibilityIdentifier("mobile-onboarding-tool-\(tool.id)")
                    }
                }
            }
        case .create:
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    Text("New Bot")
                        .font(.largeTitle.bold())
                        .accessibilityIdentifier("mobile-onboarding-create-title")
                    HStack(spacing: 16) {
                        LoginBlob(
                            color: onboardingColor(onboardingDraft.normalized.color),
                            width: 64,
                            height: 64,
                            rotation: 0
                        )
                        VStack(alignment: .leading) {
                            Text(onboardingDraft.normalized.shape.capitalized)
                                .font(.headline)
                            Text(onboardingDraft.normalized.color.capitalized)
                                .foregroundStyle(.secondary)
                        }
                    }

                    TextField("Name", text: $onboardingDraft.name)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("mobile-onboarding-bot-name")

                    Text("Character color").font(.caption).foregroundStyle(.secondary)
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 5), spacing: 10) {
                        ForEach(MobileOnboardingCharacterCatalog.colors, id: \.id) { color in
                            Button {
                                onboardingDraft.color = color.id
                                onboardingDraft.pickedTemplateId = nil
                            } label: {
                                Circle()
                                    .fill(onboardingColor(color.id))
                                    .frame(width: 30, height: 30)
                                    .overlay(
                                        Circle().stroke(
                                            onboardingDraft.normalized.color == color.id ? Color.primary : Color.clear,
                                            lineWidth: 2
                                        )
                                    )
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("\(color.label) color")
                        }
                    }

                    Text("Character shape").font(.caption).foregroundStyle(.secondary)
                    LazyVGrid(columns: Array(repeating: GridItem(.adaptive(minimum: 72)), count: 1), spacing: 8) {
                        ForEach(MobileOnboardingCharacterCatalog.shapeIds, id: \.self) { shape in
                            Button(shape.capitalized) {
                                onboardingDraft.shape = shape
                                onboardingDraft.pickedTemplateId = nil
                            }
                            .buttonStyle(.bordered)
                            .tint(onboardingDraft.normalized.shape == shape ? Color.accentColor : Color.gray)
                            .accessibilityLabel("\(shape) shape")
                            .accessibilityValue(onboardingDraft.normalized.shape == shape ? "Selected" : "Not selected")
                        }
                    }

                    Text("Suggestions").font(.headline)
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 10) {
                            let suggestions = MobileOnboardingSuggestion.selected(for: onboardingDailyTools)
                            let identities = MobileOnboardingSuggestion.identities(for: suggestions)
                            ForEach(Array(suggestions.enumerated()), id: \.element.id) { index, choice in
                                let identity = identities[index]
                                Button {
                                    onboardingDraft.name = choice.suggestion.name
                                    onboardingDraft.description = choice.renderedDescription
                                    onboardingDraft.color = identity.color
                                    onboardingDraft.shape = identity.shape
                                    onboardingDraft.pickedTemplateId = choice.suggestion.id
                                } label: {
                                    HStack(alignment: .top, spacing: 10) {
                                        LoginBlob(
                                            color: onboardingColor(identity.color),
                                            width: 40,
                                            height: 40,
                                            rotation: 0
                                        )
                                        VStack(alignment: .leading, spacing: 6) {
                                            Text(choice.suggestion.name).font(.headline)
                                            Text(choice.renderedDescription)
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                                .multilineTextAlignment(.leading)
                                        }
                                    }
                                    .frame(width: 220, alignment: .leading)
                                    .padding()
                                }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier("mobile-onboarding-suggestion-\(choice.id)")
                            }
                        }
                    }

                    Button(onboardingCreateBusy ? "Getting started…" : "Get started") {
                        submitSignedInOnboarding()
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .disabled(!onboardingDraft.canSubmit || onboardingCreateBusy)
                    .accessibilityIdentifier("mobile-onboarding-create")
                }
            }
        case .handOff:
            Spacer()
            avatar.frame(width: 76, height: 76)
            if let onboardingCreateError {
                Text("Fabushi couldn’t finish setting up")
                    .font(.title2.bold())
                    .multilineTextAlignment(.center)
                Text(onboardingCreateError)
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("mobile-onboarding-create-error")
                Button("Try again") {
                    submitSignedInOnboarding()
                }
                .buttonStyle(.borderedProminent)
                .disabled(onboardingCreateBusy)
                .accessibilityIdentifier("mobile-onboarding-create-retry")
            } else {
                ProgressView()
                Text("Getting your team ready…")
                    .font(.title3.weight(.semibold))
                    .accessibilityIdentifier("mobile-onboarding-hand-off-status")
            }
            Spacer()
        case .completed:
            EmptyView()
        }
    }

    @ViewBuilder
    var onboardingFooter: some View {
        let step = model.signedInOnboardingStep
        if step != .create && step != .completed {
            HStack {
                if let previous = step.previous {
                    Button("Back") {
                        model.signedInOnboardingStep = previous
                    }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("mobile-onboarding-back")
                }
                Spacer()
                Button("Next") {
                    model.advanceOnboarding()
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("mobile-onboarding-continue")
            }
        }
    }

    func onboardingColor(_ id: String) -> Color {
        guard let value = AvatarImagePolicy.colors.first(where: { $0.id == id })?.value else {
            return .blue
        }
        let cleaned = value.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        var parsed: UInt64 = 0
        Scanner(string: cleaned).scanHexInt64(&parsed)
        return Color(
            red: Double((parsed >> 16) & 0xff) / 255,
            green: Double((parsed >> 8) & 0xff) / 255,
            blue: Double(parsed & 0xff) / 255
        )
    }

    @MainActor
    func synchronizeOnboardingAccountScope() {
        let scope = model.settingsNoticeAccountKey
        guard onboardingAccountScope != scope else { return }
        onboardingAccountScope = scope
        cancelOnboardingOperation()
        onboardingDraft = .init()
        onboardingDailyTools = []
        onboardingToolQuery = ""
        onboardingCreateError = nil
        onboardingCreatedAgentId = nil
        onboardingCreateRequestId = "ios-signed-in-onboarding-\(UUID().uuidString.lowercased())"
    }

    @MainActor
    func cancelOnboardingOperation() {
        onboardingOperationGeneration &+= 1
        onboardingOperationTask?.cancel()
        onboardingOperationTask = nil
        onboardingCreateBusy = false
    }

    @MainActor
    func submitSignedInOnboarding() {
        guard let bridge,
              model.loggedIn,
              onboardingDraft.canSubmit,
              !onboardingCreateBusy
        else { return }

        onboardingOperationGeneration &+= 1
        let generation = onboardingOperationGeneration
        let accountScope = model.settingsNoticeAccountKey
        let draft = onboardingDraft.normalized
        let dailyTools = onboardingDailyTools
        let requestId = onboardingCreateRequestId
        onboardingCreateBusy = true
        onboardingCreateError = nil
        model.beginOnboardingHandOff()

        onboardingOperationTask?.cancel()
        onboardingOperationTask = Task { @MainActor in
            defer {
                if generation == onboardingOperationGeneration {
                    onboardingCreateBusy = false
                    onboardingOperationTask = nil
                }
            }
            do {
                let service = GrokMobileBotService(bridge: bridge)
                let existing = try await service.waitForOnboardingComputer()
                try Task.checkCancellation()
                guard generation == onboardingOperationGeneration,
                      model.loggedIn,
                      model.settingsNoticeAccountKey == accountScope
                else { return }

                if !existing.isEmpty {
                    onboardingCreatedAgentId = nil
                    try await Task.sleep(nanoseconds: MobileSignedInOnboardingContract.handOffDwellNanoseconds)
                    try Task.checkCancellation()
                    guard generation == onboardingOperationGeneration,
                          model.settingsNoticeAccountKey == accountScope
                    else { return }
                    onboardingOperationTask = nil
                    model.completeSignedInOnboarding()
                    return
                }

                let description = MobileSignedInOnboardingContract.descriptionWithDailyTools(
                    draft.description,
                    tools: dailyTools
                )
                let roster = try await service.createOnboardingBot(
                    name: draft.name,
                    description: description,
                    avatarShape: draft.shape,
                    avatarColor: draft.color,
                    requestId: requestId
                )
                try Task.checkCancellation()
                guard generation == onboardingOperationGeneration,
                      model.loggedIn,
                      model.settingsNoticeAccountKey == accountScope
                else { return }

                let existingIds = Set(existing.map(\.id))
                guard let created = roster.first(where: { !existingIds.contains($0.id) })
                    ?? roster.first(where: {
                        $0.name.trimmingCharacters(in: .whitespacesAndNewlines)
                            == draft.name.trimmingCharacters(in: .whitespacesAndNewlines)
                    })
                else {
                    throw NSError(
                        domain: "Fabushi.MobileSignedInOnboarding",
                        code: 1,
                        userInfo: [NSLocalizedDescriptionKey: "Bot creation returned no matching Bot."]
                    )
                }
                onboardingCreatedAgentId = created.id
                try await Task.sleep(nanoseconds: MobileSignedInOnboardingContract.handOffDwellNanoseconds)
                try Task.checkCancellation()
                guard generation == onboardingOperationGeneration,
                      model.settingsNoticeAccountKey == accountScope
                else { return }
                onboardingOperationTask = nil
                model.completeSignedInOnboarding()
            } catch is CancellationError {
                return
            } catch {
                guard generation == onboardingOperationGeneration,
                      model.settingsNoticeAccountKey == accountScope
                else { return }
                onboardingCreateError = MobileSignedInOnboardingContract.createErrorMessage(error)
                model.signedInOnboardingStep = .handOff
            }
        }
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

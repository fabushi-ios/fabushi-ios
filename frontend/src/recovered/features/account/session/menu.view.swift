import Foundation
import SwiftUI

/// UI-safe projection of the server-authoritative Fabushi model budget.
///
/// The renderer receives only aggregate budget fields. Account credentials stay
/// inside the Rust product/Host boundary.
struct AccountUsageProjection: Equatable, Sendable {
    let windowStart: Int64
    let windowEnd: Int64
    let tokenLimit: Int64
    let usedTokens: Int64
    let reservedTokens: Int64
    let remainingTokens: Int64
    let unlimited: Bool

    init?(payload: [String: Any]) {
        guard
            let windowStart = Self.integer(payload["windowStart"]),
            let windowEnd = Self.integer(payload["windowEnd"]),
            let tokenLimit = Self.integer(payload["tokenLimit"]),
            let usedTokens = Self.integer(payload["usedTokens"]),
            let reservedTokens = Self.integer(payload["reservedTokens"]),
            let remainingTokens = Self.integer(payload["remainingTokens"]),
            let unlimited = payload["unlimited"] as? Bool,
            windowStart >= 0,
            windowEnd > windowStart,
            tokenLimit >= 0,
            usedTokens >= 0,
            reservedTokens >= 0,
            remainingTokens >= 0
        else { return nil }

        if !unlimited {
            guard tokenLimit > 0, remainingTokens <= tokenLimit else { return nil }
        }

        self.windowStart = windowStart
        self.windowEnd = windowEnd
        self.tokenLimit = tokenLimit
        self.usedTokens = usedTokens
        self.reservedTokens = reservedTokens
        self.remainingTokens = remainingTokens
        self.unlimited = unlimited
    }

    var committedTokens: Int64 {
        guard !unlimited, tokenLimit > 0 else {
            return max(0, usedTokens)
        }
        return max(0, tokenLimit - min(tokenLimit, remainingTokens))
    }

    var usageFraction: Double? {
        guard !unlimited, tokenLimit > 0 else { return nil }
        return min(1, max(0, Double(committedTokens) / Double(tokenLimit)))
    }

    var usagePercent: Int? {
        usageFraction.map { Int(($0 * 100).rounded()) }
    }

    var semanticSummary: String {
        if unlimited { return "当前周期用量不限量" }
        return "当前周期用量 \(usagePercent ?? 0)% · 剩余 \(remainingTokens) tokens"
    }

    private static func integer(_ raw: Any?) -> Int64? {
        guard let raw, !(raw is Bool) else { return nil }
        if let number = raw as? NSNumber {
            return number.int64Value
        }
        if let value = raw as? Int64 { return value }
        if let value = raw as? Int { return Int64(value) }
        return nil
    }
}

/// iOS-native adaptation of Grok's account/session menu.
///
/// Floating desktop menu geometry becomes a native sheet/List. Identity and
/// usage remain Host projections; this view never creates a second auth truth.
struct AccountMenuView: View {
    @Bindable var model: MarketplaceModel
    let avatar: AnyView
    let onClose: () -> Void
    let onRequestSignOut: () -> Void
    let onOpenRemoteComputer: () -> Void
    let onOpenMarketplace: () -> Void
    let onOpenSection: (MobileSection) -> Void

    @State private var aboutPresented = false
    @State private var settingsPresented = false
    @State private var feedbackPresented = false
    @State private var actionError: String?

    var body: some View {
        NavigationStack {
            List {
                Section("账号") {
                    HStack(spacing: 12) {
                        avatar
                            .frame(width: 42, height: 42)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(model.accountName)
                                .font(.headline)
                            if !model.accountEmail.isEmpty {
                                Text(model.accountEmail)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }

                    usageSurface

                    Button("退出登录", role: .destructive) {
                        onRequestSignOut()
                    }
                    .accessibilityIdentifier("mobile-logout")
                }

                Section("工作台") {
                    Button(action: onOpenRemoteComputer) {
                        Label("我的电脑", systemImage: "desktopcomputer")
                    }
                    .accessibilityIdentifier("remote-computer-entry")

                    Button(action: onOpenMarketplace) {
                        Label("插件市场", systemImage: "puzzlepiece.extension")
                    }
                    .accessibilityIdentifier("marketplace-entry")
                }

                Section("应用") {
                    Button {
                        settingsPresented = true
                    } label: {
                        Label("设置", systemImage: "gearshape")
                    }
                    .accessibilityIdentifier("account-settings-entry")

                    Button {
                        Task {
                            do { try await model.openAccountHelp() }
                            catch { actionError = error.localizedDescription }
                        }
                    } label: {
                        Label("帮助中心", systemImage: "questionmark.circle")
                    }
                    .accessibilityIdentifier("account-help-entry")

                    Button {
                        feedbackPresented = true
                    } label: {
                        Label("发送反馈", systemImage: "exclamationmark.bubble")
                    }
                    .accessibilityIdentifier("account-feedback-entry")

                    Button {
                        aboutPresented = true
                    } label: {
                        Label("关于 Fabushi", systemImage: "info.circle")
                    }
                    .accessibilityIdentifier("about-entry")
                }

                Section("导航") {
                    ForEach(MobileSection.allCases) { section in
                        Button {
                            onOpenSection(section)
                        } label: {
                            Label(section.label, systemImage: section.symbol)
                        }
                        .accessibilityIdentifier("profile-section-\(section.rawValue)")
                    }
                }
            }
            .navigationTitle("导航")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消", action: onClose)
                        .accessibilityIdentifier("account-menu-close")
                }
            }
            .task {
                await model.refreshAccountUsage()
            }
            .refreshable {
                await model.refreshAccountUsage()
            }
        }
        .sheet(isPresented: $aboutPresented) {
            FabushiAboutOverlayView()
        }
        .sheet(isPresented: $settingsPresented) {
            AccountSettingsView(model: model) {
                settingsPresented = false
            }
        }
        .sheet(isPresented: $feedbackPresented) {
            AccountFeedbackView(model: model) {
                feedbackPresented = false
            }
        }
        .alert("操作失败", isPresented: Binding(
            get: { actionError != nil },
            set: { if !$0 { actionError = nil } }
        )) {
            Button("好") { actionError = nil }
        } message: {
            Text(actionError ?? "")
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("account-menu")
    }

    @ViewBuilder
    private var usageSurface: some View {
        if let usage = model.accountUsage {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Label("当前周期用量", systemImage: "chart.bar.fill")
                    Spacer()
                    Text(usage.unlimited ? "不限量" : "\(usage.usagePercent ?? 0)%")
                        .foregroundStyle(.secondary)
                }

                if let fraction = usage.usageFraction {
                    ProgressView(value: fraction)
                        .accessibilityIdentifier("account-usage-progress")
                    Text("\(usage.committedTokens.formatted()) / \(usage.tokenLimit.formatted()) tokens")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Text("周期结束：\(Date(timeIntervalSince1970: TimeInterval(usage.windowEnd)).formatted(date: .abbreviated, time: .shortened))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("account-usage-summary")
        } else if model.accountUsageLoading {
            HStack(spacing: 8) {
                ProgressView()
                Text("正在加载用量…")
                    .foregroundStyle(.secondary)
            }
            .accessibilityIdentifier("account-usage-loading")
        } else if model.accountUsageError != nil {
            Button {
                Task { await model.refreshAccountUsage() }
            } label: {
                Label("重新加载用量", systemImage: "arrow.clockwise")
            }
            .accessibilityIdentifier("account-usage-retry")
        }
    }
}


/// Native iOS replacement for the Desktop account-menu Settings route.
/// It keeps account identity and mutations behind MarketplaceModel's canonical
/// preload bridge rather than introducing a renderer-owned account store.
struct AccountSettingsView: View {
    @Bindable var model: MarketplaceModel
    let onDone: () -> Void

    @State private var nameDraft = ""
    @State private var nameSaving = false
    @State private var saveGeneration = 0
    @State private var feedbackPresented = false
    @State private var aboutPresented = false
    @State private var actionError: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("账号") {
                    TextField("显示名称", text: $nameDraft)
                        .textInputAutocapitalization(.words)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("account-display-name-field")

                    Button(nameSaving ? "正在保存…" : "保存显示名称") {
                        saveDisplayName()
                    }
                    .disabled(nameSaving || MarketplaceModel.normalizedAccountDisplayName(nameDraft).isEmpty)
                    .accessibilityIdentifier("account-display-name-save")

                    if !model.accountEmail.isEmpty {
                        LabeledContent("邮箱", value: model.accountEmail)
                    }
                }

                Section("用量") {
                    if let usage = model.accountUsage {
                        LabeledContent("当前周期", value: usage.unlimited ? "不限量" : "\(usage.usagePercent ?? 0)%")
                        if !usage.unlimited {
                            ProgressView(value: usage.usageFraction ?? 0)
                                .accessibilityIdentifier("settings-account-usage-progress")
                            LabeledContent("剩余", value: "\(usage.remainingTokens.formatted()) tokens")
                        }
                        LabeledContent(
                            "周期结束",
                            value: Date(timeIntervalSince1970: TimeInterval(usage.windowEnd))
                                .formatted(date: .abbreviated, time: .shortened)
                        )
                    } else if model.accountUsageLoading {
                        ProgressView("正在加载用量…")
                    } else {
                        Button("重新加载用量") {
                            Task { await model.refreshAccountUsage() }
                        }
                    }
                }

                Section("支持") {
                    Button {
                        Task {
                            do { try await model.openAccountHelp() }
                            catch { actionError = error.localizedDescription }
                        }
                    } label: {
                        Label("帮助中心", systemImage: "questionmark.circle")
                    }
                    .accessibilityIdentifier("settings-help-entry")

                    Button {
                        feedbackPresented = true
                    } label: {
                        Label("发送反馈", systemImage: "exclamationmark.bubble")
                    }
                    .accessibilityIdentifier("settings-feedback-entry")

                    Button {
                        aboutPresented = true
                    } label: {
                        Label("关于 Fabushi", systemImage: "info.circle")
                    }
                    .accessibilityIdentifier("settings-about-entry")
                }

                Section("iOS") {
                    Label("当前设备已安装 Fabushi iOS", systemImage: "checkmark.seal.fill")
                    Text("Desktop 的“下载 iOS”入口在 iOS 上是自引用项；安装和更新由当前 App 与 App Store 生命周期负责。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("settings-ios-self-reference-disposition")
                }
            }
            .navigationTitle("设置")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成", action: onDone)
                }
            }
            .onAppear {
                if nameDraft.isEmpty { nameDraft = model.accountName }
            }
            .task {
                await model.refreshAccountUsage()
            }
        }
        .sheet(isPresented: $feedbackPresented) {
            AccountFeedbackView(model: model) { feedbackPresented = false }
        }
        .sheet(isPresented: $aboutPresented) {
            FabushiAboutOverlayView()
        }
        .alert("操作失败", isPresented: Binding(
            get: { actionError != nil },
            set: { if !$0 { actionError = nil } }
        )) {
            Button("好") { actionError = nil }
        } message: {
            Text(actionError ?? "")
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("account-settings")
    }

    private func saveDisplayName() {
        let normalized = MarketplaceModel.normalizedAccountDisplayName(nameDraft)
        guard !normalized.isEmpty, normalized.count <= 200 else {
            actionError = "名称必须为 1–200 个字符。"
            return
        }
        saveGeneration = saveGeneration == Int.max ? 1 : saveGeneration + 1
        let generation = saveGeneration
        nameSaving = true
        Task {
            do {
                try await model.updateAccountDisplayName(normalized)
                guard generation == saveGeneration else { return }
                nameDraft = model.accountName
                nameSaving = false
            } catch {
                guard generation == saveGeneration else { return }
                nameSaving = false
                actionError = error.localizedDescription
            }
        }
    }
}

struct AccountFeedbackView: View {
    @Bindable var model: MarketplaceModel
    let onDone: () -> Void

    @State private var message = ""
    @State private var submitting = false
    @State private var submitGeneration = 0
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("反馈内容") {
                    TextEditor(text: $message)
                        .frame(minHeight: 180)
                        .accessibilityIdentifier("account-feedback-message")
                    Text("\(message.count) / 10,000")
                        .font(.caption)
                        .foregroundStyle(message.count > 10_000 ? Color.red : Color.secondary)
                }
                if let errorMessage {
                    Section {
                        Text(errorMessage)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("account-feedback-error")
                    }
                }
            }
            .navigationTitle("发送反馈")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消", action: onDone)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(submitting ? "正在发送…" : "发送") {
                        submit()
                    }
                    .disabled(
                        submitting
                            || message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                            || message.count > 10_000
                    )
                    .accessibilityIdentifier("account-feedback-submit")
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("account-feedback")
    }

    private func submit() {
        submitGeneration = submitGeneration == Int.max ? 1 : submitGeneration + 1
        let generation = submitGeneration
        submitting = true
        errorMessage = nil
        Task {
            do {
                try await model.submitAccountFeedback(message)
                guard generation == submitGeneration else { return }
                submitting = false
                onDone()
            } catch {
                guard generation == submitGeneration else { return }
                submitting = false
                errorMessage = error.localizedDescription
            }
        }
    }
}

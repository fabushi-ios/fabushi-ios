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
    let conversationId: String?
    let onClose: () -> Void
    let onRequestSignOut: () -> Void
    let onOpenRemoteComputer: () -> Void
    let onOpenMarketplace: () -> Void
    let onOpenSection: (MobileSection) -> Void

    @State private var aboutPresented = false
    @State private var settingsPresented = false
    @State private var feedbackPresented = false
    @State private var actionError: String?
    @Environment(\.mobileUiPreferencesStore) private var uiPreferencesStore

    private struct AdvancedCopy {
        let section: String
        let timeZone: String
        let automaticTimeZone: String
        let localExecution: String
        let localExecutionDescription: String
        let permissionAlways: String
        let permissionAsk: String
        let permissionNever: String
        let maximum: String
    }

    private var advancedCopy: AdvancedCopy {
        switch (uiPreferencesStore?.preferences ?? MobileUiPreferences()).locale {
        case .zhHans:
            return .init(section: "高级", timeZone: "时区", automaticTimeZone: "自动", localExecution: "本地执行权限", localExecutionDescription: "控制 Agent 在此设备上使用本地工具前是否需要询问。", permissionAlways: "始终允许", permissionAsk: "先询问", permissionNever: "从不允许", maximum: "管理员上限")
        case .zhHant:
            return .init(section: "進階", timeZone: "時區", automaticTimeZone: "自動", localExecution: "本機執行權限", localExecutionDescription: "控制 Agent 在此裝置使用本機工具前是否需要詢問。", permissionAlways: "一律允許", permissionAsk: "先詢問", permissionNever: "永不允許", maximum: "管理員上限")
        case .ja:
            return .init(section: "詳細設定", timeZone: "タイムゾーン", automaticTimeZone: "自動", localExecution: "ローカル実行権限", localExecutionDescription: "このデバイスでローカルツールを使う前に Agent が確認するかを制御します。", permissionAlways: "常に許可", permissionAsk: "先に確認", permissionNever: "許可しない", maximum: "管理者上限")
        case .ko:
            return .init(section: "고급", timeZone: "시간대", automaticTimeZone: "자동", localExecution: "로컬 실행 권한", localExecutionDescription: "Agent가 이 기기에서 로컬 도구를 사용하기 전에 물어볼지 제어합니다.", permissionAlways: "항상 허용", permissionAsk: "먼저 묻기", permissionNever: "허용 안 함", maximum: "관리자 한도")
        case .ar:
            return .init(section: "متقدم", timeZone: "المنطقة الزمنية", automaticTimeZone: "تلقائي", localExecution: "إذن التنفيذ المحلي", localExecutionDescription: "يتحكم فيما إذا كان على Agent طلب الإذن قبل استخدام الأدوات المحلية على هذا الجهاز.", permissionAlways: "السماح دائمًا", permissionAsk: "اسأل أولًا", permissionNever: "عدم السماح", maximum: "حد المسؤول")
        case .he:
            return .init(section: "מתקדם", timeZone: "אזור זמן", automaticTimeZone: "אוטומטי", localExecution: "הרשאת ביצוע מקומית", localExecutionDescription: "קובע אם Agent צריך לשאול לפני שימוש בכלים מקומיים במכשיר הזה.", permissionAlways: "לאפשר תמיד", permissionAsk: "לשאול קודם", permissionNever: "לא לאפשר", maximum: "מגבלת מנהל")
        case .en, .system:
            return .init(section: "Advanced", timeZone: "Time zone", automaticTimeZone: "Automatic", localExecution: "Local execution permission", localExecutionDescription: "Controls whether Agent must ask before using local tools on this device.", permissionAlways: "Always allow", permissionAsk: "Ask first", permissionNever: "Never allow", maximum: "Admin maximum")
        }
    }

    private var shellCopy: MobileSettingsShellCopy {
        (uiPreferencesStore?.preferences ?? MobileUiPreferences()).shellCopy()
    }

    var body: some View {
        NavigationStack {
            List {
                Section(shellCopy.account) {
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

                    Button(shellCopy.signOut, role: .destructive) {
                        onRequestSignOut()
                    }
                    .accessibilityIdentifier("mobile-logout")
                }

                Section(shellCopy.workspace) {
                    Button(action: onOpenRemoteComputer) {
                        Label(shellCopy.computer, systemImage: "desktopcomputer")
                    }
                    .accessibilityIdentifier("remote-computer-entry")

                    Button(action: onOpenMarketplace) {
                        Label(shellCopy.marketplace, systemImage: "puzzlepiece.extension")
                    }
                    .accessibilityIdentifier("marketplace-entry")
                }

                Section(shellCopy.app) {
                    Button {
                        settingsPresented = true
                    } label: {
                        Label(shellCopy.settings, systemImage: "gearshape")
                    }
                    .accessibilityIdentifier("account-settings-entry")

                    Button {
                        Task {
                            do { try await model.openAccountHelp() }
                            catch { actionError = error.localizedDescription }
                        }
                    } label: {
                        Label(shellCopy.helpCenter, systemImage: "questionmark.circle")
                    }
                    .accessibilityIdentifier("account-help-entry")

                    Button {
                        feedbackPresented = true
                    } label: {
                        Label(shellCopy.sendFeedback, systemImage: "exclamationmark.bubble")
                    }
                    .accessibilityIdentifier("account-feedback-entry")

                    Button {
                        aboutPresented = true
                    } label: {
                        Label(shellCopy.aboutFabushi, systemImage: "info.circle")
                    }
                    .accessibilityIdentifier("about-entry")
                }

                Section(shellCopy.navigation) {
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
            .navigationTitle(shellCopy.navigation)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(shellCopy.cancel, action: onClose)
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
            AccountSettingsView(model: model, conversationId: conversationId) {
                settingsPresented = false
            }
        }
        .sheet(isPresented: $feedbackPresented) {
            AccountFeedbackView(
                model: model,
                conversationId: conversationId,
                defaultIncludeConversationId: conversationId != nil
            ) {
                feedbackPresented = false
            }
        }
        .alert(shellCopy.operationFailed, isPresented: Binding(
            get: { actionError != nil },
            set: { if !$0 { actionError = nil } }
        )) {
            Button(shellCopy.okay) { actionError = nil }
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
                    Label(shellCopy.currentPeriod, systemImage: "chart.bar.fill")
                    Spacer()
                    Text(usage.unlimited ? shellCopy.unlimited : "\(usage.usagePercent ?? 0)%")
                        .foregroundStyle(.secondary)
                }

                if let fraction = usage.usageFraction {
                    ProgressView(value: fraction)
                        .accessibilityIdentifier("account-usage-progress")
                    Text("\(usage.committedTokens.formatted()) / \(usage.tokenLimit.formatted()) tokens")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Text("\(shellCopy.periodEnds)：\(Date(timeIntervalSince1970: TimeInterval(usage.windowEnd)).formatted(date: .abbreviated, time: .shortened))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("account-usage-summary")
        } else if model.accountUsageLoading {
            HStack(spacing: 8) {
                ProgressView()
                Text(shellCopy.loadingUsage)
                    .foregroundStyle(.secondary)
            }
            .accessibilityIdentifier("account-usage-loading")
        } else if model.accountUsageError != nil {
            Button {
                Task { await model.refreshAccountUsage() }
            } label: {
                Label(shellCopy.reloadUsage, systemImage: "arrow.clockwise")
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
    let conversationId: String?
    let onDone: () -> Void

    init(
        model: MarketplaceModel,
        conversationId: String? = nil,
        onDone: @escaping () -> Void
    ) {
        self.model = model
        self.conversationId = conversationId
        self.onDone = onDone
    }

    @State private var nameDraft = ""
    @State private var nameSaving = false
    @State private var saveGeneration = 0
    @State private var feedbackPresented = false
    @State private var aboutPresented = false
    @State private var actionError: String?
    @State private var configurationLoading = true
    @State private var configurationSaving = false
    @State private var configurationGeneration = 0
    @State private var autoReviewSettings = DEFAULT_SAND_AUTO_REVIEW_INSTRUCTIONS
    @State private var inferenceProvider: SandInferenceProvider = .fabushi
    @State private var privacyModeEnabled = true
    @State private var timeZoneState = MobileTimeZoneSettingsState(
        detectedTimeZone: nil,
        overrideTimeZone: nil
    )
    @State private var localToolPermissionState = MobileLocalToolPermissionState(
        permission: SAND_DEFAULT_LOCAL_TOOL_PERMISSION,
        ceiling: nil
    )
    @State private var ruleDraft = ""
    @State private var ruleBehavior: SandAutoReviewInstructionBehavior = .allow
    @State private var editingRule: SandAutoReviewInstructionRow?
    @Environment(\.mobileUiPreferencesStore) private var uiPreferencesStore
    @Environment(\.humanCallMediaPort) private var mediaPort

    private var localizedFeatureCopy: MobileSettingsFeatureCopy {
        (uiPreferencesStore?.preferences ?? MobileUiPreferences()).featureCopy()
    }

    private var shellCopy: MobileSettingsShellCopy {
        (uiPreferencesStore?.preferences ?? MobileUiPreferences()).shellCopy()
    }

    @State private var mediaDevices: [HumanCallMediaDevice] = []
    @State private var selectedMicrophoneId: String?
    @State private var selectedCameraId: String?
    @State private var mediaPermissions: HumanCallMediaPermissions?
    @State private var mediaBusy = false

    var body: some View {
        NavigationStack {
            Form {
                Section(shellCopy.account) {
                    TextField(shellCopy.displayName, text: $nameDraft)
                        .textInputAutocapitalization(.words)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("account-display-name-field")

                    Button(nameSaving ? shellCopy.saving : shellCopy.saveDisplayName) {
                        saveDisplayName()
                    }
                    .disabled(nameSaving || MarketplaceModel.normalizedAccountDisplayName(nameDraft).isEmpty)
                    .accessibilityIdentifier("account-display-name-save")

                    if !model.accountEmail.isEmpty {
                        LabeledContent(shellCopy.email, value: model.accountEmail)
                    }
                }

                Section(shellCopy.usage) {
                    if let usage = model.accountUsage {
                        LabeledContent(shellCopy.currentPeriod, value: usage.unlimited ? shellCopy.unlimited : "\(usage.usagePercent ?? 0)%")
                        if !usage.unlimited {
                            ProgressView(value: usage.usageFraction ?? 0)
                                .accessibilityIdentifier("settings-account-usage-progress")
                            LabeledContent(shellCopy.remaining, value: "\(usage.remainingTokens.formatted()) tokens")
                        }
                        LabeledContent(
                            shellCopy.periodEnds,
                            value: Date(timeIntervalSince1970: TimeInterval(usage.windowEnd))
                                .formatted(date: .abbreviated, time: .shortened)
                        )
                    } else if model.accountUsageLoading {
                        ProgressView(shellCopy.loadingUsage)
                    } else {
                        Button(shellCopy.reloadUsage) {
                            Task { await model.refreshAccountUsage() }
                        }
                    }
                }

                uiPreferencesSection

                mediaDevicesSection

                configurationSections

                Section(localizedFeatureCopy.privacy) {
                    LabeledContent(
                        localizedFeatureCopy.privacyMode,
                        value: privacyModeEnabled
                            ? localizedFeatureCopy.stateEnabled
                            : localizedFeatureCopy.stateDisabled
                    )
                    .accessibilityIdentifier("settings-privacy-mode-status")

                    Text(localizedFeatureCopy.privacyModeDescription)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("settings-privacy-mode-description")
                }

                Section(shellCopy.support) {
                    Button {
                        Task {
                            do { try await model.openAccountHelp() }
                            catch { actionError = error.localizedDescription }
                        }
                    } label: {
                        Label(shellCopy.helpCenter, systemImage: "questionmark.circle")
                    }
                    .accessibilityIdentifier("settings-help-entry")

                    Button {
                        feedbackPresented = true
                    } label: {
                        Label(shellCopy.sendFeedback, systemImage: "exclamationmark.bubble")
                    }
                    .accessibilityIdentifier("settings-feedback-entry")

                    Button {
                        aboutPresented = true
                    } label: {
                        Label(shellCopy.aboutFabushi, systemImage: "info.circle")
                    }
                    .accessibilityIdentifier("settings-about-entry")
                }

                Section(shellCopy.iosSection) {
                    Label(shellCopy.iosInstalled, systemImage: "checkmark.seal.fill")
                    Text(shellCopy.iosSelfReference)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("settings-ios-self-reference-disposition")
                }
            }
            .navigationTitle(shellCopy.settings)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(shellCopy.done, action: onDone)
                }
            }
            .onAppear {
                if nameDraft.isEmpty { nameDraft = model.accountName }
                bindSettingsNoticeScope()
                refreshMediaDevices()
            }
            .task {
                await model.refreshAccountUsage()
            }
            .task(id: model.settingsNoticeAccountKey) {
                bindSettingsNoticeScope()
                await refreshConfigurationSettings()
            }
            .onDisappear {
                configurationGeneration = configurationGeneration == Int.max
                    ? 1
                    : configurationGeneration + 1
                model.settingsNoticeController.updateScope(
                    accountKey: model.settingsNoticeAccountKey,
                    surface: .none
                )
            }
        }
        .sheet(isPresented: $feedbackPresented) {
            AccountFeedbackView(
                model: model,
                conversationId: conversationId,
                defaultIncludeConversationId: conversationId != nil
            ) { feedbackPresented = false }
        }
        .sheet(isPresented: $aboutPresented) {
            FabushiAboutOverlayView()
        }
        .alert(shellCopy.operationFailed, isPresented: Binding(
            get: { actionError != nil },
            set: { if !$0 { actionError = nil } }
        )) {
            Button(shellCopy.okay) { actionError = nil }
        } message: {
            Text(actionError ?? "")
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("account-settings")
    }

    @ViewBuilder
    private var uiPreferencesSection: some View {
        if let uiPreferencesStore {
            let preferences = uiPreferencesStore.preferences
            let copy = preferences.accessibilityCopy()
            Section(copy.sectionTitle) {
                Picker(
                    copy.language,
                    selection: Binding(
                        get: { preferences.locale },
                        set: { value in mutateUiPreferences { uiPreferencesStore.setLocale(value) } }
                    )
                ) {
                    ForEach(MobileSettingsLocale.allCases) { locale in
                        Text(locale == .system ? copy.systemLanguage : locale.optionLabel)
                            .tag(locale)
                    }
                }
                .accessibilityIdentifier("settings-ui-language")
    
                Text(copy.languageDescription)
                    .font(.caption)
                    .foregroundStyle(.secondary)
    
                Picker(
                    copy.readingDirection,
                    selection: Binding(
                        get: { preferences.direction },
                        set: { value in mutateUiPreferences { uiPreferencesStore.setDirection(value) } }
                    )
                ) {
                    Text(copy.directionAutomatic).tag(MobileSettingsDirection.auto)
                    Text(copy.directionLtr).tag(MobileSettingsDirection.ltr)
                    Text(copy.directionRtl).tag(MobileSettingsDirection.rtl)
                }
                .accessibilityIdentifier("settings-ui-reading-direction")
    
                Text(copy.readingDirectionDescription)
                    .font(.caption)
                    .foregroundStyle(.secondary)
    
                Picker(
                    copy.textSize,
                    selection: Binding(
                        get: { preferences.textScale },
                        set: { value in mutateUiPreferences { uiPreferencesStore.setTextScale(value) } }
                    )
                ) {
                    ForEach(MobileUiPreferences.supportedTextScales, id: \.self) { scale in
                        Text("\(Int((scale * 100).rounded()))%").tag(scale)
                    }
                }
                .accessibilityIdentifier("settings-ui-text-scale")
    
                Text(copy.textSizeDescription)
                    .font(.caption)
                    .foregroundStyle(.secondary)
    
                Toggle(
                    copy.reduceMotion,
                    isOn: Binding(
                        get: { preferences.reducedMotion },
                        set: { value in mutateUiPreferences { uiPreferencesStore.setReducedMotion(value) } }
                    )
                )
                .accessibilityIdentifier("settings-ui-reduce-motion")
    
                Text(copy.reduceMotionDescription)
                    .font(.caption)
                    .foregroundStyle(.secondary)
    
                Toggle(
                    copy.highContrast,
                    isOn: Binding(
                        get: { preferences.highContrast },
                        set: { value in mutateUiPreferences { uiPreferencesStore.setHighContrast(value) } }
                    )
                )
                .accessibilityIdentifier("settings-ui-high-contrast")
    
                Text(copy.highContrastDescription)
                    .font(.caption)
                    .foregroundStyle(.secondary)
    
                Text(
                    copy.accessibilityCount(
                        preferences.activeAccessibilityPreferenceCount,
                        locale: preferences.resolvedLocale()
                    )
                )
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("settings-ui-accessibility-count")
            }
        }
    }

    @ViewBuilder
    private var mediaDevicesSection: some View {
        let microphones = mediaDevices.filter { $0.kind == .microphone }
        let cameras = mediaDevices.filter { $0.kind == .camera }

        Section(localizedFeatureCopy.mediaDevices) {
            Picker(
                localizedFeatureCopy.microphone,
                selection: Binding(
                    get: { selectedMicrophoneId ?? "" },
                    set: { value in selectMediaDevice(value, kind: .microphone) }
                )
            ) {
                Text(localizedFeatureCopy.defaultDevice).tag("")
                ForEach(microphones) { device in
                    Text(device.name).tag(device.id)
                }
            }
            .disabled(mediaBusy)
            .accessibilityIdentifier("settings-media-microphone")

            Picker(
                localizedFeatureCopy.camera,
                selection: Binding(
                    get: { selectedCameraId ?? "" },
                    set: { value in selectMediaDevice(value, kind: .camera) }
                )
            ) {
                Text(localizedFeatureCopy.defaultDevice).tag("")
                ForEach(cameras) { device in
                    Text(device.name).tag(device.id)
                }
            }
            .disabled(mediaBusy)
            .accessibilityIdentifier("settings-media-camera")

            Button(mediaBusy ? localizedFeatureCopy.grantMediaPermissions + "…" : localizedFeatureCopy.grantMediaPermissions) {
                requestMediaPermissions()
            }
            .disabled(mediaBusy)
            .accessibilityIdentifier("settings-media-request-permissions")

            Button(localizedFeatureCopy.refreshDevices) {
                refreshMediaDevices()
            }
            .disabled(mediaBusy)
            .accessibilityIdentifier("settings-media-refresh")

            if let mediaPermissions {
                Text(
                    "\(shellCopy.microphoneState)：\(mediaPermissions.microphone.rawValue) · \(shellCopy.cameraState)：\(mediaPermissions.camera.rawValue)"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("settings-media-permission-state")
            }

            Text(localizedFeatureCopy.mediaPermissionDescription)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var configurationSections: some View {
        if configurationLoading {
            Section(shellCopy.agentConfiguration) {
                ProgressView(shellCopy.loadingConfiguration)
                    .accessibilityIdentifier("settings-configuration-loading")
            }
        } else {
            Section(shellCopy.router) {
                Picker(
                    shellCopy.routeAgentRequests,
                    selection: Binding(
                        get: { inferenceProvider },
                        set: { beginInferenceProviderUpdate($0) }
                    )
                ) {
                    ForEach(
                        SAND_INFERENCE_PROVIDER_DESCRIPTORS,
                        id: \.provider
                    ) { option in
                        Text(option.label).tag(option.provider)
                    }
                }
                .disabled(configurationSaving)
                .accessibilityIdentifier("settings-inference-provider")

                let descriptor = sandInferenceProviderDescriptor(inferenceProvider)
                Text(descriptor.description)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(descriptor.usageDescription)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("settings-inference-provider-usage")
            }

            Section(advancedCopy.section) {
                Picker(
                    advancedCopy.timeZone,
                    selection: Binding(
                        get: { timeZoneState.overrideTimeZone ?? "" },
                        set: { beginTimeZoneUpdate($0.isEmpty ? nil : $0) }
                    )
                ) {
                    let autoLabel = timeZoneState.detectedTimeZone.map {
                        "\(advancedCopy.automaticTimeZone) · \($0.replacingOccurrences(of: "_", with: " "))"
                    } ?? advancedCopy.automaticTimeZone
                    Text(autoLabel).tag("")
                    ForEach(TimeZone.knownTimeZoneIdentifiers, id: \.self) { zone in
                        Text(zone.replacingOccurrences(of: "_", with: " ")).tag(zone)
                    }
                }
                .disabled(configurationSaving)
                .accessibilityIdentifier("settings-time-zone")

                Picker(
                    advancedCopy.localExecution,
                    selection: Binding(
                        get: { localToolPermissionState.permission },
                        set: { beginLocalToolPermissionUpdate($0) }
                    )
                ) {
                    ForEach(SAND_LOCAL_TOOL_PERMISSIONS, id: \.self) { permission in
                        Text(localToolPermissionLabel(permission))
                            .tag(permission)
                            .disabled(localToolPermissionExceedsCeiling(permission))
                    }
                }
                .disabled(configurationSaving)
                .accessibilityIdentifier("settings-local-tool-permission")

                Text(advancedCopy.localExecutionDescription)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if let ceiling = localToolPermissionState.ceiling {
                    Text("\(advancedCopy.maximum): \(localToolPermissionLabel(ceiling))")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("settings-local-tool-permission-ceiling")
                }
            }

            Section(localizedFeatureCopy.autoReviewTitle) {
                Toggle(
                    localizedFeatureCopy.autoReviewDescription,
                    isOn: Binding(
                        get: { autoReviewSettings.isEnabled },
                        set: { enabled in
                            beginAutoReviewUpdate(.init(
                                isEnabled: enabled,
                                allowInstructions: autoReviewSettings.allowInstructions,
                                blockInstructions: autoReviewSettings.blockInstructions
                            ))
                        }
                    )
                )
                .disabled(configurationSaving)
                .accessibilityIdentifier("settings-auto-review-enabled")

                if autoReviewSettings.isEnabled {
                    ForEach(sandAutoReviewInstructionRows(autoReviewSettings)) { row in
                        VStack(alignment: .leading, spacing: 6) {
                            Text(row.text)
                                .font(.body)
                                .textSelection(.enabled)
                            HStack {
                                Text(
                                    row.behavior == .allow
                                        ? localizedFeatureCopy.allowAutomatically
                                        : localizedFeatureCopy.askFirst
                                )
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Spacer()
                                Button(localizedFeatureCopy.edit) { beginEditingRule(row) }
                                    .disabled(configurationSaving)
                                    .accessibilityIdentifier("settings-auto-review-edit-\(row.id)")
                                Button(localizedFeatureCopy.delete, role: .destructive) {
                                    beginAutoReviewUpdate(
                                        removeSandAutoReviewInstruction(
                                            autoReviewSettings,
                                            row: row
                                        )
                                    )
                                }
                                .disabled(configurationSaving)
                                .accessibilityIdentifier("settings-auto-review-delete-\(row.id)")
                            }
                        }
                    }

                    TextField(
                        editingRule == nil
                            ? localizedFeatureCopy.autoReviewDraftLabel
                            : localizedFeatureCopy.autoReviewDraftLabel,
                        text: Binding(
                            get: { ruleDraft },
                            set: { ruleDraft = clampSandAutoReviewInstructionDraft($0) }
                        ),
                        axis: .vertical
                    )
                    .lineLimit(1...4)
                    .disabled(configurationSaving)
                    .accessibilityIdentifier("settings-auto-review-rule-draft")

                    Picker(localizedFeatureCopy.ruleBehaviorLabel, selection: $ruleBehavior) {
                        Text(localizedFeatureCopy.allowAutomatically)
                            .tag(SandAutoReviewInstructionBehavior.allow)
                        Text(localizedFeatureCopy.askFirst)
                            .tag(SandAutoReviewInstructionBehavior.ask)
                    }
                    .disabled(configurationSaving)
                    .accessibilityIdentifier("settings-auto-review-rule-behavior")

                    HStack {
                        Button(
                            editingRule == nil
                                ? localizedFeatureCopy.addRule
                                : localizedFeatureCopy.saveRule
                        ) {
                            commitRuleDraft()
                        }
                        .disabled(
                            configurationSaving
                                || saveSandAutoReviewInstruction(
                                    autoReviewSettings,
                                    text: ruleDraft,
                                    behavior: ruleBehavior,
                                    editing: editingRule
                                ) == nil
                        )
                        .accessibilityIdentifier("settings-auto-review-rule-save")

                        if editingRule != nil {
                            Button(localizedFeatureCopy.cancel) { clearRuleEditor() }
                                .disabled(configurationSaving)
                                .accessibilityIdentifier("settings-auto-review-rule-cancel")
                        }
                    }

                    Text(localizedFeatureCopy.rulesScope)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    @MainActor
    private func bindSettingsNoticeScope() {
        model.settingsNoticeController.updateScope(
            accountKey: model.settingsNoticeAccountKey,
            surface: .settings
        )
    }

    @MainActor
    private func publishSettingsNotice(
        _ kind: SurfaceNoticeKind,
        operation: SettingsNoticeOperation,
        message: String,
        fence: SettingsNoticeFence? = nil
    ) {
        SurfaceNoticePublisher.publish(
            SettingsNoticeEventFactory.settings(
                kind,
                operation: operation,
                message: message
            ),
            controller: model.settingsNoticeController,
            fence: fence
        )
    }

    @MainActor
    private func mutateUiPreferences(_ mutation: () -> Void) {
        let fence = model.settingsNoticeController.makeFence()
        mutation()
        publishSettingsNotice(
            .success,
            operation: .uiPreferences,
            message: "UI preferences updated.",
            fence: fence
        )
    }

    @MainActor
    private func refreshMediaDevices() {
        guard let mediaPort else {
            mediaDevices = []
            selectedMicrophoneId = nil
            selectedCameraId = nil
            return
        }
        mediaDevices = mediaPort.devices()
        let stored = mediaPort.storedPreferences()
        selectedMicrophoneId = stored.microphoneId
        selectedCameraId = stored.cameraId
    }

    @MainActor
    private func selectMediaDevice(_ id: String, kind: HumanCallMediaDevice.Kind) {
        let fence = model.settingsNoticeController.makeFence()
        guard let mediaPort else {
            actionError = shellCopy.mediaRuntimeUnavailable
            publishSettingsNotice(
                .error,
                operation: .callMedia,
                message: shellCopy.mediaRuntimeUnavailable,
                fence: fence
            )
            return
        }
        do {
            if id.isEmpty {
                mediaPort.setPreferredDeviceId(nil, kind: kind)
            } else if kind == .microphone {
                _ = try mediaPort.selectMicrophone(deviceId: id)
            } else {
                _ = try mediaPort.selectCamera(deviceId: id)
            }
            refreshMediaDevices()
            publishSettingsNotice(
                .success,
                operation: .callMedia,
                message: "Call media settings updated.",
                fence: fence
            )
        } catch {
            actionError = error.localizedDescription
            refreshMediaDevices()
            publishSettingsNotice(
                .error,
                operation: .callMedia,
                message: error.localizedDescription,
                fence: fence
            )
        }
    }

    @MainActor
    private func requestMediaPermissions() {
        guard !mediaBusy else { return }
        let accountKey = model.settingsNoticeAccountKey
        let fence = model.settingsNoticeController.makeFence()
        guard let mediaPort else {
            actionError = shellCopy.mediaRuntimeUnavailable
            publishSettingsNotice(
                .error,
                operation: .callMedia,
                message: shellCopy.mediaRuntimeUnavailable,
                fence: fence
            )
            return
        }
        mediaBusy = true
        Task { @MainActor in
            let result = await mediaPort.requestPermissions(audio: true, video: true)
            guard accountKey == model.settingsNoticeAccountKey else {
                mediaBusy = false
                return
            }
            mediaPermissions = result
            mediaBusy = false
            refreshMediaDevices()
            publishSettingsNotice(
                .success,
                operation: .callMedia,
                message: "Call media permissions updated.",
                fence: fence
            )
        }
    }

    @MainActor
    private func refreshConfigurationSettings() async {
        configurationGeneration = configurationGeneration == Int.max
            ? 1
            : configurationGeneration + 1
        let generation = configurationGeneration
        let accountKey = model.settingsNoticeAccountKey
        configurationLoading = true
        configurationSaving = false
        do {
            let snapshot = try await model.loadConfigurationSettings()
            guard generation == configurationGeneration,
                  accountKey == model.settingsNoticeAccountKey
            else { return }
            autoReviewSettings = snapshot.autoReview
            inferenceProvider = snapshot.inferenceProvider
            privacyModeEnabled = snapshot.privacyModeEnabled
            timeZoneState = snapshot.timeZone
            localToolPermissionState = snapshot.localToolPermission
            editingRule = editingRule.flatMap {
                reconcileSandAutoReviewInstructionRow(
                    snapshot.autoReview,
                    row: $0
                )
            }
            configurationLoading = false
        } catch {
            guard generation == configurationGeneration,
                  accountKey == model.settingsNoticeAccountKey
            else { return }
            configurationLoading = false
            actionError = error.localizedDescription
        }
    }

    private func localToolPermissionLabel(
        _ permission: SandLocalToolPermission
    ) -> String {
        switch permission {
        case "always": advancedCopy.permissionAlways
        case "never": advancedCopy.permissionNever
        default: advancedCopy.permissionAsk
        }
    }

    private func localToolPermissionExceedsCeiling(
        _ permission: SandLocalToolPermission
    ) -> Bool {
        guard let ceiling = localToolPermissionState.ceiling,
              let requestedRank = SAND_LOCAL_TOOL_PERMISSION_RANK[permission],
              let ceilingRank = SAND_LOCAL_TOOL_PERMISSION_RANK[ceiling]
        else { return false }
        return requestedRank > ceilingRank
    }

    @MainActor
    private func beginTimeZoneUpdate(_ next: String?) {
        guard !configurationSaving, next != timeZoneState.overrideTimeZone else { return }
        configurationGeneration = configurationGeneration == Int.max
            ? 1
            : configurationGeneration + 1
        let generation = configurationGeneration
        let accountKey = model.settingsNoticeAccountKey
        configurationSaving = true
        Task { @MainActor in
            do {
                let saved = try await model.updateTimeZoneOverride(next)
                guard generation == configurationGeneration,
                      accountKey == model.settingsNoticeAccountKey
                else { return }
                timeZoneState = saved
                configurationSaving = false
            } catch {
                guard generation == configurationGeneration,
                      accountKey == model.settingsNoticeAccountKey
                else { return }
                configurationSaving = false
                actionError = error.localizedDescription
            }
        }
    }

    @MainActor
    private func beginLocalToolPermissionUpdate(
        _ next: SandLocalToolPermission
    ) {
        guard !configurationSaving,
              next != localToolPermissionState.permission,
              !localToolPermissionExceedsCeiling(next)
        else { return }
        configurationGeneration = configurationGeneration == Int.max
            ? 1
            : configurationGeneration + 1
        let generation = configurationGeneration
        let accountKey = model.settingsNoticeAccountKey
        configurationSaving = true
        Task { @MainActor in
            do {
                let saved = try await model.updateLocalToolPermission(next)
                guard generation == configurationGeneration,
                      accountKey == model.settingsNoticeAccountKey
                else { return }
                localToolPermissionState = .init(
                    permission: saved,
                    ceiling: localToolPermissionState.ceiling
                )
                configurationSaving = false
            } catch {
                guard generation == configurationGeneration,
                      accountKey == model.settingsNoticeAccountKey
                else { return }
                configurationSaving = false
                actionError = error.localizedDescription
            }
        }
    }

    @MainActor
    private func beginAutoReviewUpdate(
        _ next: SandAutoReviewInstructions,
        clearEditorAfterSuccess: Bool = false
    ) {
        guard !configurationSaving else { return }
        configurationGeneration = configurationGeneration == Int.max
            ? 1
            : configurationGeneration + 1
        let generation = configurationGeneration
        let accountKey = model.settingsNoticeAccountKey
        configurationSaving = true
        Task { @MainActor in
            do {
                let saved = try await model.updateAutoReviewSettings(next)
                guard generation == configurationGeneration,
                      accountKey == model.settingsNoticeAccountKey
                else { return }
                autoReviewSettings = saved
                configurationSaving = false
                if clearEditorAfterSuccess {
                    clearRuleEditor()
                } else if let editingRule {
                    self.editingRule = reconcileSandAutoReviewInstructionRow(
                        saved,
                        row: editingRule
                    )
                }
            } catch {
                guard generation == configurationGeneration,
                      accountKey == model.settingsNoticeAccountKey
                else { return }
                configurationSaving = false
                actionError = error.localizedDescription
            }
        }
    }

    @MainActor
    private func beginInferenceProviderUpdate(_ next: SandInferenceProvider) {
        guard !configurationSaving, next != inferenceProvider else { return }
        configurationGeneration = configurationGeneration == Int.max
            ? 1
            : configurationGeneration + 1
        let generation = configurationGeneration
        let accountKey = model.settingsNoticeAccountKey
        configurationSaving = true
        Task { @MainActor in
            do {
                let saved = try await model.updateInferenceProvider(next)
                guard generation == configurationGeneration,
                      accountKey == model.settingsNoticeAccountKey
                else { return }
                inferenceProvider = saved
                configurationSaving = false
            } catch {
                guard generation == configurationGeneration,
                      accountKey == model.settingsNoticeAccountKey
                else { return }
                configurationSaving = false
                actionError = error.localizedDescription
            }
        }
    }

    @MainActor
    private func beginEditingRule(_ row: SandAutoReviewInstructionRow) {
        editingRule = row
        ruleDraft = row.text
        ruleBehavior = row.behavior
    }

    @MainActor
    private func clearRuleEditor() {
        editingRule = nil
        ruleDraft = ""
        ruleBehavior = .allow
    }

    @MainActor
    private func commitRuleDraft() {
        guard let next = saveSandAutoReviewInstruction(
            autoReviewSettings,
            text: ruleDraft,
            behavior: ruleBehavior,
            editing: editingRule
        ) else { return }
        beginAutoReviewUpdate(
            next,
            clearEditorAfterSuccess: true
        )
    }

    private func saveDisplayName() {
        let normalized = MarketplaceModel.normalizedAccountDisplayName(nameDraft)
        guard !normalized.isEmpty, normalized.count <= 200 else {
            actionError = shellCopy.displayNameValidationError
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

enum AccountFeedbackViewState: Equatable {
    case idle
    case sending
    case sent
    case failed(AccountFeedbackCode)
}

struct AccountFeedbackView: View {
    @Bindable var model: MarketplaceModel
    let conversationId: String?
    let onDone: () -> Void

    @State private var message = ""
    @State private var includeConversationId: Bool
    @State private var state: AccountFeedbackViewState = .idle
    @State private var submitGeneration = 0

    init(
        model: MarketplaceModel,
        conversationId: String? = nil,
        defaultIncludeConversationId: Bool = false,
        onDone: @escaping () -> Void
    ) {
        self.model = model
        self.conversationId = conversationId
        self.onDone = onDone
        _includeConversationId = State(initialValue: defaultIncludeConversationId)
    }

    private var sending: Bool { state == .sending }
    private var sent: Bool { state == .sent }
    private var canSend: Bool {
        !sending
            && !sent
            && !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && message.count <= 10_000
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("反馈内容") {
                    TextEditor(text: $message)
                        .frame(minHeight: 180)
                        .disabled(sending || sent)
                        .accessibilityIdentifier("account-feedback-message")
                    Text("\(message.count) / 10,000")
                        .font(.caption)
                        .foregroundStyle(message.count > 10_000 ? Color.red : Color.secondary)
                }

                if conversationId != nil {
                    Section {
                        Toggle("附带当前会话 ID", isOn: $includeConversationId)
                            .disabled(sending || sent)
                            .accessibilityIdentifier("account-feedback-include-conversation")
                    } footer: {
                        Text("仅在你选择时，将当前会话 ID 与反馈一起发送，便于定位相关问题。")
                    }
                }

                switch state {
                case .sent:
                    Section {
                        Text("反馈已发送。")
                            .accessibilityIdentifier("account-feedback-sent")
                            .accessibilityAddTraits(.isStaticText)
                    }
                case let .failed(code):
                    Section {
                        Text(code.localizedMessage)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("account-feedback-error")
                    }
                case .idle, .sending:
                    EmptyView()
                }
            }
            .navigationTitle("发送反馈")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(sent ? "完成" : "取消", action: onDone)
                        .disabled(sending)
                        .accessibilityIdentifier(sent ? "account-feedback-done" : "account-feedback-cancel")
                }
                if !sent {
                    ToolbarItem(placement: .confirmationAction) {
                        Button(sending ? "正在发送…" : "发送") {
                            submit()
                        }
                        .disabled(!canSend)
                        .accessibilityIdentifier("account-feedback-submit")
                    }
                }
            }
        }
        .interactiveDismissDisabled(sending)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("account-feedback")
    }

    private func submit() {
        guard canSend else { return }
        submitGeneration = submitGeneration == Int.max ? 1 : submitGeneration + 1
        let generation = submitGeneration
        state = .sending
        Task {
            do {
                try await model.submitAccountFeedback(
                    message,
                    conversationId: includeConversationId ? conversationId : nil
                )
                guard generation == submitGeneration else { return }
                state = .sent
            } catch let error as AccountFeedbackError {
                guard generation == submitGeneration else { return }
                state = .failed(error.code)
            } catch {
                guard generation == submitGeneration else { return }
                state = .failed(.unavailable)
            }
        }
    }
}

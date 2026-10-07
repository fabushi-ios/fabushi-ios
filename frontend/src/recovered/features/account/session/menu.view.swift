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

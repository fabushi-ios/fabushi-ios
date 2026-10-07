import Foundation
import SwiftUI
import UIKit

enum FabushiAboutReleaseTrack: String, Equatable, Sendable {
    case development = "Development"
    case testFlight = "TestFlight"
    case appStore = "App Store"
    case archive = "Archive"
}

struct FabushiAboutInfo: Equatable, Sendable {
    let version: String
    let build: String
    let releaseTrack: FabushiAboutReleaseTrack
    let platform: String

    var displayVersion: String {
        build == "Unknown" ? version : "\(version) (\(build))"
    }

    var copyText: String {
        [
            "Version: \(version)",
            "Build: \(build)",
            "Release Track: \(releaseTrack.rawValue)",
            "OS: \(platform)",
        ].joined(separator: "\n")
    }

    static func project(
        infoDictionary: [String: Any],
        receiptLastPathComponent: String?,
        isDebug: Bool,
        platform: String = "iOS"
    ) -> Self {
        Self(
            version: string(infoDictionary["CFBundleShortVersionString"]) ?? "Unknown",
            build: string(infoDictionary["CFBundleVersion"]) ?? "Unknown",
            releaseTrack: releaseTrack(
                receiptLastPathComponent: receiptLastPathComponent,
                isDebug: isDebug
            ),
            platform: platform
        )
    }

    static func current(bundle: Bundle = .main) -> Self {
        #if DEBUG
        let isDebug = true
        #else
        let isDebug = false
        #endif
        return project(
            infoDictionary: bundle.infoDictionary ?? [:],
            receiptLastPathComponent: bundle.appStoreReceiptURL?.lastPathComponent,
            isDebug: isDebug
        )
    }

    private static func string(_ value: Any?) -> String? {
        if let value = value as? String {
            let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }
        if let value = value as? NSNumber {
            return value.stringValue
        }
        return nil
    }

    private static func releaseTrack(
        receiptLastPathComponent: String?,
        isDebug: Bool
    ) -> FabushiAboutReleaseTrack {
        if isDebug { return .development }
        guard let receiptLastPathComponent else { return .archive }
        return receiptLastPathComponent == "sandboxReceipt" ? .testFlight : .appStore
    }
}

enum FabushiAboutPresentationPolicy {
    static let copyConfirmationMilliseconds = 1_200
}

@MainActor
struct FabushiAboutOverlayView: View {
    @Environment(\.dismiss) private var dismiss

    let info: FabushiAboutInfo
    @State private var copied = false
    @State private var copyGeneration = 0

    init(info: FabushiAboutInfo = .current()) {
        self.info = info
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 18) {
                ClothGhostAvatar(botId: "fabushi-about", size: 64)
                    .accessibilityHidden(true)

                VStack(spacing: 5) {
                    Text("Fabushi")
                        .font(.title2.bold())
                    Text("Version \(info.displayVersion)")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("about-version")
                    Text("© Fabushi")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }

                VStack(alignment: .leading, spacing: 8) {
                    LabeledContent("Release Track", value: info.releaseTrack.rawValue)
                    LabeledContent("Platform", value: info.platform)
                }
                .font(.subheadline)
                .padding(14)
                .background(
                    Color(uiColor: .secondarySystemBackground),
                    in: RoundedRectangle(cornerRadius: 14, style: .continuous)
                )

                Spacer(minLength: 0)

                Button(copied ? "已复制" : "复制版本信息") {
                    UIPasteboard.general.string = info.copyText
                    copied = true
                    copyGeneration = copyGeneration == Int.max ? 1 : copyGeneration + 1
                }
                .buttonStyle(.borderedProminent)
                .accessibilityLabel(copied ? "已复制" : "复制版本信息")
                .accessibilityIdentifier("about-copy-version")
            }
            .padding(24)
            .navigationTitle("关于 Fabushi")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("关闭") { dismiss() }
                        .accessibilityIdentifier("about-close")
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("about-surface")
        .task(id: copyGeneration) {
            guard copied else { return }
            let generation = copyGeneration
            do {
                try await Task.sleep(
                    for: .milliseconds(FabushiAboutPresentationPolicy.copyConfirmationMilliseconds)
                )
            } catch {
                return
            }
            guard generation == copyGeneration else { return }
            copied = false
        }
    }
}

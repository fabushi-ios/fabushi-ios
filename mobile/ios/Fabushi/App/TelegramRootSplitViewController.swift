import SwiftUI
#if canImport(UIKit)
import UIKit

/// Native UIKit `UISplitViewController(style: .doubleColumn)` root controller (`F11`)
/// activating side-by-side master-detail when `bounds.width > 680.0pt` and registering
/// iPadOS / Simulator hardware keyboard shortcuts (`Cmd+1..4`, `Cmd+K`, `Cmd+,`, `Cmd+D`, `Cmd+L`, `Cmd+Shift+L`).
@MainActor
public final class TelegramRootSplitViewController: UISplitViewController {
    public static let regularSplitBreakpointPt: CGFloat = 680.0

    public var onSelectTabShortcut: ((RootTab) -> Void)?
    public var onFocusSearchShortcut: (() -> Void)?
    public var onCycleThemeShortcut: (() -> Void)?
    public var onToggleLanguageShortcut: (() -> Void)?
    public var onLockAppShortcut: (() -> Void)?

    private let primaryHosting: UIHostingController<AnyView>
    private let secondaryHosting: UIHostingController<AnyView>

    public init(primaryView: AnyView, secondaryView: AnyView) {
        self.primaryHosting = UIHostingController(rootView: primaryView)
        self.secondaryHosting = UIHostingController(rootView: secondaryView)
        super.init(style: .doubleColumn)

        self.minimumPrimaryColumnWidth = 320.0
        self.maximumPrimaryColumnWidth = 375.0
        self.preferredPrimaryColumnWidthFraction = 0.36
        self.preferredDisplayMode = .oneBesideSecondary
        self.preferredSplitBehavior = .tile
        self.presentsWithGesture = true

        self.primaryHosting.view.backgroundColor = .clear
        self.secondaryHosting.view.backgroundColor = .clear
        self.setViewController(primaryHosting, for: .primary)
        self.setViewController(secondaryHosting, for: .secondary)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override public func viewDidLoad() {
        super.viewDidLoad()
        view.accessibilityIdentifier = "fabushi.root.splitView"
        applySplitBreakpoint(for: view.bounds.size.width)
    }

    override public func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        applySplitBreakpoint(for: view.bounds.size.width)
    }

    public func updateContent(primaryView: AnyView, secondaryView: AnyView, showSecondaryInCompact: Bool) {
        primaryHosting.rootView = primaryView
        secondaryHosting.rootView = secondaryView
        if isCollapsed && showSecondaryInCompact {
            showDetailViewController(secondaryHosting, sender: nil)
        }
    }

    public static func isRegularSplitWidth(_ width: CGFloat) -> Bool {
        width > regularSplitBreakpointPt
    }

    private func applySplitBreakpoint(for width: CGFloat) {
        guard width > 0 else { return }
        if Self.isRegularSplitWidth(width) {
            if preferredDisplayMode != .oneBesideSecondary {
                preferredDisplayMode = .oneBesideSecondary
            }
        }
    }

    override public var keyCommands: [UIKeyCommand]? {
        [
            UIKeyCommand(title: "Contacts", action: #selector(handleTab1), input: "1", modifierFlags: .command),
            UIKeyCommand(title: "Calls", action: #selector(handleTab2), input: "2", modifierFlags: .command),
            UIKeyCommand(title: "Chats", action: #selector(handleTab3), input: "3", modifierFlags: .command),
            UIKeyCommand(title: "Settings", action: #selector(handleTab4), input: "4", modifierFlags: .command),
            UIKeyCommand(title: "Search", action: #selector(handleSearch), input: "k", modifierFlags: .command),
            UIKeyCommand(title: "Preferences", action: #selector(handlePreferences), input: ",", modifierFlags: .command),
            UIKeyCommand(title: "Cycle Theme", action: #selector(handleCycleTheme), input: "d", modifierFlags: .command),
            UIKeyCommand(title: "Toggle Language", action: #selector(handleToggleLanguage), input: "l", modifierFlags: .command),
            UIKeyCommand(title: "Lock App", action: #selector(handleLockApp), input: "l", modifierFlags: [.command, .shift]),
        ]
    }

    @objc private func handleTab1() { onSelectTabShortcut?(.contacts) }
    @objc private func handleTab2() { onSelectTabShortcut?(.calls) }
    @objc private func handleTab3() { onSelectTabShortcut?(.chats) }
    @objc private func handleTab4() { onSelectTabShortcut?(.settings) }
    @objc private func handleSearch() { onFocusSearchShortcut?() }
    @objc private func handlePreferences() { onSelectTabShortcut?(.settings) }
    @objc private func handleCycleTheme() { onCycleThemeShortcut?() }
    @objc private func handleToggleLanguage() { onToggleLanguageShortcut?() }
    @objc private func handleLockApp() { onLockAppShortcut?() }
}

public struct TelegramRootSplitContainer: UIViewControllerRepresentable {
    public let primaryView: AnyView
    public let secondaryView: AnyView
    public let showSecondaryInCompact: Bool
    public let onSelectTabShortcut: (RootTab) -> Void
    public let onFocusSearchShortcut: () -> Void
    public let onCycleThemeShortcut: () -> Void
    public let onToggleLanguageShortcut: () -> Void
    public let onLockAppShortcut: () -> Void

    public init(
        primaryView: AnyView,
        secondaryView: AnyView,
        showSecondaryInCompact: Bool = false,
        onSelectTabShortcut: @escaping (RootTab) -> Void = { _ in },
        onFocusSearchShortcut: @escaping () -> Void = {},
        onCycleThemeShortcut: @escaping () -> Void = {},
        onToggleLanguageShortcut: @escaping () -> Void = {},
        onLockAppShortcut: @escaping () -> Void = {}
    ) {
        self.primaryView = primaryView
        self.secondaryView = secondaryView
        self.showSecondaryInCompact = showSecondaryInCompact
        self.onSelectTabShortcut = onSelectTabShortcut
        self.onFocusSearchShortcut = onFocusSearchShortcut
        self.onCycleThemeShortcut = onCycleThemeShortcut
        self.onToggleLanguageShortcut = onToggleLanguageShortcut
        self.onLockAppShortcut = onLockAppShortcut
    }

    public func makeUIViewController(context: Context) -> TelegramRootSplitViewController {
        let vc = TelegramRootSplitViewController(primaryView: primaryView, secondaryView: secondaryView)
        vc.onSelectTabShortcut = onSelectTabShortcut
        vc.onFocusSearchShortcut = onFocusSearchShortcut
        vc.onCycleThemeShortcut = onCycleThemeShortcut
        vc.onToggleLanguageShortcut = onToggleLanguageShortcut
        vc.onLockAppShortcut = onLockAppShortcut
        return vc
    }

    public func updateUIViewController(_ uiViewController: TelegramRootSplitViewController, context: Context) {
        uiViewController.onSelectTabShortcut = onSelectTabShortcut
        uiViewController.onFocusSearchShortcut = onFocusSearchShortcut
        uiViewController.onCycleThemeShortcut = onCycleThemeShortcut
        uiViewController.onToggleLanguageShortcut = onToggleLanguageShortcut
        uiViewController.onLockAppShortcut = onLockAppShortcut
        uiViewController.updateContent(
            primaryView: primaryView,
            secondaryView: secondaryView,
            showSecondaryInCompact: showSecondaryInCompact
        )
    }
}
#endif

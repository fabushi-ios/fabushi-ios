import SwiftUI
import UIKit

@MainActor
internal final class MobileComposerUITextView: UITextView {
    var onEscapeKey: (() -> Bool)?
    var onSuggestionMove: ((MobileEditorSuggestionMove) -> Bool)?

    private let placeholderLabel: UILabel = {
        let label = UILabel()
        label.text = "Message"
        label.textColor = .placeholderText
        label.font = .preferredFont(forTextStyle: .body)
        label.adjustsFontForContentSizeCategory = true
        label.isAccessibilityElement = false
        return label
    }()

    override init(frame: CGRect, textContainer: NSTextContainer?) {
        super.init(frame: frame, textContainer: textContainer)
        backgroundColor = .clear
        font = .preferredFont(forTextStyle: .body)
        adjustsFontForContentSizeCategory = true
        textContainerInset = UIEdgeInsets(top: 11, left: 14, bottom: 11, right: 14)
        self.textContainer.lineFragmentPadding = 0
        textAlignment = .natural
        semanticContentAttribute = .unspecified
        autocorrectionType = .yes
        spellCheckingType = .yes
        smartQuotesType = .yes
        smartDashesType = .yes
        keyboardDismissMode = .interactive
        returnKeyType = .send
        accessibilityLabel = "Message"
        accessibilityIdentifier = "mobile-bot-draft"
        addSubview(placeholderLabel)
        updatePlaceholderVisibility()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        // UIKit/IME owns all hardware keys while marked text is active so
        // arrows, Escape and candidate-selection keys keep their native meaning.
        guard markedTextRange == nil,
              presses.count == 1,
              let key = presses.first?.key
        else {
            super.pressesBegan(presses, with: event)
            return
        }

        let command = key.modifierFlags.contains(.command)
        let handled: Bool
        switch key.keyCode {
        case .keyboardEscape:
            handled = onEscapeKey?() ?? false
        case .keyboardUpArrow:
            handled = onSuggestionMove?(command ? .first : .previous) ?? false
        case .keyboardDownArrow:
            handled = onSuggestionMove?(command ? .last : .next) ?? false
        default:
            handled = false
        }
        if handled { return }
        super.pressesBegan(presses, with: event)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let width = max(
            0,
            bounds.width - textContainerInset.left - textContainerInset.right
        )
        placeholderLabel.frame = CGRect(
            x: textContainerInset.left,
            y: textContainerInset.top,
            width: width,
            height: placeholderLabel.intrinsicContentSize.height
        )
    }

    func updatePlaceholderVisibility() {
        placeholderLabel.isHidden = !text.isEmpty || markedTextRange != nil
    }

}

@MainActor
internal final class MobileComposerTextCoordinator: NSObject, UITextViewDelegate {
    private struct PendingExternal {
        let scopeKey: String
        let text: String
    }

    private var binding: Binding<String> = .constant("")
    private var scopeKey = ""
    private var onSubmit: () -> Void = {}
    private var onEscape: () -> Bool = { false }
    private var onSuggestionMove: (MobileEditorSuggestionMove) -> Bool = { _ in false }

    private var compositionActive = false
    private var compositionScopeKey: String?
    private var compositionBinding: Binding<String>?
    private var pendingExternal: PendingExternal?
    private var lastFocusGeneration: Int?

    func attach(to textView: MobileComposerUITextView) {
        textView.delegate = self
        textView.onEscapeKey = { [weak self, weak textView] in
            guard let self, let textView else { return false }
            return self.handleEscape(in: textView)
        }
        textView.onSuggestionMove = { [weak self, weak textView] move in
            guard let self, let textView else { return false }
            return self.handleSuggestionMove(move, in: textView)
        }
        applyNaturalWritingDirection(to: textView)
        textView.updatePlaceholderVisibility()
    }

    func updateOwner(
        binding: Binding<String>,
        scopeKey: String,
        onSubmit: @escaping () -> Void,
        onEscape: @escaping () -> Bool,
        onSuggestionMove: @escaping (MobileEditorSuggestionMove) -> Bool = { _ in false }
    ) {
        self.binding = binding
        self.scopeKey = scopeKey
        self.onSubmit = onSubmit
        self.onEscape = onEscape
        self.onSuggestionMove = onSuggestionMove
    }

    func syncExternalText(
        _ text: String,
        scopeKey: String,
        to textView: MobileComposerUITextView
    ) {
        if isCompositionActive(in: textView) {
            beginCompositionIfNeeded(in: textView)
            pendingExternal = .init(scopeKey: scopeKey, text: text)
            textView.updatePlaceholderVisibility()
            return
        }

        pendingExternal = nil
        replaceVisibleText(text, in: textView)
    }

    func updateFocus(
        generation: Int,
        in textView: MobileComposerUITextView
    ) {
        defer { lastFocusGeneration = generation }
        if let lastFocusGeneration {
            guard lastFocusGeneration != generation else { return }
            _ = textView.becomeFirstResponder()
        } else if generation > 0 {
            _ = textView.becomeFirstResponder()
        }
    }

    func isCompositionActive(in textView: UITextView) -> Bool {
        compositionActive || textView.markedTextRange != nil
    }

    @discardableResult
    func handleEscape(in textView: UITextView) -> Bool {
        guard !isCompositionActive(in: textView) else { return false }
        return onEscape()
    }

    @discardableResult
    func handleSuggestionMove(
        _ move: MobileEditorSuggestionMove,
        in textView: UITextView
    ) -> Bool {
        guard !isCompositionActive(in: textView) else { return false }
        return onSuggestionMove(move)
    }

    func textViewDidChange(_ textView: UITextView) {
        guard let textView = textView as? MobileComposerUITextView else { return }
        textView.updatePlaceholderVisibility()
        applyNaturalWritingDirection(to: textView)

        if textView.markedTextRange != nil {
            beginCompositionIfNeeded(in: textView)
            return
        }

        if compositionActive {
            let committedText = textView.text ?? ""
            let ownedScope = compositionScopeKey
            let ownedBinding = compositionBinding
            compositionActive = false
            compositionScopeKey = nil
            compositionBinding = nil

            if ownedScope == scopeKey {
                binding.wrappedValue = committedText
                pendingExternal = nil
            } else {
                ownedBinding?.wrappedValue = committedText
                let replacement: String
                if let pendingExternal, pendingExternal.scopeKey == scopeKey {
                    replacement = pendingExternal.text
                } else {
                    replacement = binding.wrappedValue
                }
                self.pendingExternal = nil
                replaceVisibleText(replacement, in: textView)
            }
            return
        }

        binding.wrappedValue = textView.text ?? ""
    }

    func textView(
        _ textView: UITextView,
        shouldChangeTextIn range: NSRange,
        replacementText text: String
    ) -> Bool {
        guard text == "\n" else { return true }
        if isCompositionActive(in: textView) {
            return true
        }
        onSubmit()
        return false
    }

    private func beginCompositionIfNeeded(in textView: UITextView) {
        guard !compositionActive else { return }
        compositionActive = true
        compositionScopeKey = scopeKey
        compositionBinding = binding
    }

    private func replaceVisibleText(
        _ text: String,
        in textView: MobileComposerUITextView
    ) {
        guard textView.text != text else {
            applyNaturalWritingDirection(to: textView)
            textView.updatePlaceholderVisibility()
            return
        }
        textView.text = text
        applyNaturalWritingDirection(to: textView)
        textView.updatePlaceholderVisibility()
        textView.invalidateIntrinsicContentSize()
    }

    private func applyNaturalWritingDirection(to textView: UITextView) {
        let paragraph = NSMutableParagraphStyle()
        paragraph.baseWritingDirection = .natural
        paragraph.alignment = .natural

        var typingAttributes = textView.typingAttributes
        typingAttributes[.paragraphStyle] = paragraph
        textView.typingAttributes = typingAttributes
        textView.textAlignment = .natural
        textView.semanticContentAttribute = .unspecified

        if textView.textStorage.length > 0 {
            textView.textStorage.addAttribute(
                .paragraphStyle,
                value: paragraph,
                range: NSRange(location: 0, length: textView.textStorage.length)
            )
        }
    }
}

@MainActor
internal struct MobileComposerTextView: UIViewRepresentable {
    @Binding var text: String
    let scopeKey: String
    let focusGeneration: Int
    let onSubmit: () -> Void
    let onEscape: () -> Bool
    let onSuggestionMove: (MobileEditorSuggestionMove) -> Bool

    func makeCoordinator() -> MobileComposerTextCoordinator {
        MobileComposerTextCoordinator()
    }

    func makeUIView(context: Context) -> MobileComposerUITextView {
        let textView = MobileComposerUITextView()
        context.coordinator.attach(to: textView)
        context.coordinator.updateOwner(
            binding: $text,
            scopeKey: scopeKey,
            onSubmit: onSubmit,
            onEscape: onEscape,
            onSuggestionMove: onSuggestionMove
        )
        context.coordinator.syncExternalText(text, scopeKey: scopeKey, to: textView)
        context.coordinator.updateFocus(generation: focusGeneration, in: textView)
        return textView
    }

    func updateUIView(_ textView: MobileComposerUITextView, context: Context) {
        context.coordinator.updateOwner(
            binding: $text,
            scopeKey: scopeKey,
            onSubmit: onSubmit,
            onEscape: onEscape,
            onSuggestionMove: onSuggestionMove
        )
        context.coordinator.syncExternalText(text, scopeKey: scopeKey, to: textView)
        context.coordinator.updateFocus(generation: focusGeneration, in: textView)
    }

    func sizeThatFits(
        _ proposal: ProposedViewSize,
        uiView: MobileComposerUITextView,
        context: Context
    ) -> CGSize? {
        guard let width = proposal.width, width > 0 else { return nil }
        let fitting = uiView.sizeThatFits(
            CGSize(width: width, height: CGFloat.greatestFiniteMagnitude)
        )
        let lineHeight = uiView.font?.lineHeight ?? 20
        let insets = uiView.textContainerInset.top + uiView.textContainerInset.bottom
        let minimumHeight = lineHeight + insets
        let maximumHeight = (lineHeight * 5) + insets
        return CGSize(
            width: width,
            height: min(max(fitting.height, minimumHeight), maximumHeight)
        )
    }
}

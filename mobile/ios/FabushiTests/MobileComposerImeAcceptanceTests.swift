import SwiftUI
import UIKit
import XCTest
@testable import Fabushi

@MainActor
final class MobileComposerImeAcceptanceTests: XCTestCase {
    func testChineseAndJapaneseMarkedTextFenceSubmitEscapeAndExternalDraftSync() throws {
        for sample in ["中文", "かな"] {
            var draft = "seed"
            var submitCount = 0
            var escapeCount = 0
            let binding = Binding(
                get: { draft },
                set: { draft = $0 }
            )

            let coordinator = MobileComposerTextCoordinator()
            let textView = MobileComposerUITextView()
            let window = host(textView)
            _ = window

            coordinator.attach(to: textView)
            coordinator.updateOwner(
                binding: binding,
                scopeKey: "account-a|agent-a",
                onSubmit: { submitCount += 1 },
                onEscape: { escapeCount += 1 }
            )
            coordinator.syncExternalText(draft, scopeKey: "account-a|agent-a", to: textView)

            textView.selectedRange = NSRange(location: (textView.text as NSString).length, length: 0)
            textView.setMarkedText(
                sample,
                selectedRange: NSRange(location: (sample as NSString).length, length: 0)
            )
            coordinator.textViewDidChange(textView)
            XCTAssertNotNil(textView.markedTextRange, "Expected UIKit marked text for \(sample)")

            draft = "external-draft"
            coordinator.updateOwner(
                binding: binding,
                scopeKey: "account-a|agent-a",
                onSubmit: { submitCount += 1 },
                onEscape: { escapeCount += 1 }
            )
            coordinator.syncExternalText(draft, scopeKey: "account-a|agent-a", to: textView)

            XCTAssertNotEqual(textView.text, "external-draft")
            XCTAssertTrue(
                coordinator.textView(
                    textView,
                    shouldChangeTextIn: textView.selectedRange,
                    replacementText: "\n"
                )
            )
            XCTAssertEqual(submitCount, 0)
            XCTAssertFalse(coordinator.handleEscape(in: textView))
            XCTAssertEqual(escapeCount, 0)

            textView.unmarkText()
            coordinator.textViewDidChange(textView)
            XCTAssertNil(textView.markedTextRange)
            XCTAssertEqual(draft, textView.text)
            XCTAssertNotEqual(draft, "external-draft")

            XCTAssertFalse(
                coordinator.textView(
                    textView,
                    shouldChangeTextIn: textView.selectedRange,
                    replacementText: "\n"
                )
            )
            XCTAssertEqual(submitCount, 1)
            XCTAssertTrue(coordinator.handleEscape(in: textView))
            XCTAssertEqual(escapeCount, 1)
        }
    }

    func testScopeSwitchDoesNotOverwriteOrLeakActiveComposition() {
        var firstDraft = "first"
        var secondDraft = "second"
        let firstBinding = Binding(
            get: { firstDraft },
            set: { firstDraft = $0 }
        )
        let secondBinding = Binding(
            get: { secondDraft },
            set: { secondDraft = $0 }
        )

        let coordinator = MobileComposerTextCoordinator()
        let textView = MobileComposerUITextView()
        let window = host(textView)
        _ = window

        coordinator.attach(to: textView)
        coordinator.updateOwner(
            binding: firstBinding,
            scopeKey: "account-a|agent-a",
            onSubmit: {},
            onEscape: {}
        )
        coordinator.syncExternalText(firstDraft, scopeKey: "account-a|agent-a", to: textView)

        textView.selectedRange = NSRange(location: (textView.text as NSString).length, length: 0)
        textView.setMarkedText(
            "中文",
            selectedRange: NSRange(location: 2, length: 0)
        )
        coordinator.textViewDidChange(textView)
        XCTAssertNotNil(textView.markedTextRange)

        coordinator.updateOwner(
            binding: secondBinding,
            scopeKey: "account-b|agent-a",
            onSubmit: {},
            onEscape: {}
        )
        coordinator.syncExternalText(secondDraft, scopeKey: "account-b|agent-a", to: textView)

        XCTAssertNotEqual(textView.text, secondDraft)
        textView.unmarkText()
        coordinator.textViewDidChange(textView)

        XCTAssertTrue(firstDraft.contains("中文"))
        XCTAssertEqual(secondDraft, "second")
        XCTAssertEqual(textView.text, "second")
    }

    func testNaturalBidiDirectionAndAccountAgentStorageKeys() {
        let textView = MobileComposerUITextView()
        let coordinator = MobileComposerTextCoordinator()
        coordinator.attach(to: textView)
        coordinator.syncExternalText("中文 مرحبا", scopeKey: "account-a|agent-a", to: textView)

        let paragraph = textView.typingAttributes[.paragraphStyle] as? NSParagraphStyle
        XCTAssertEqual(paragraph?.baseWritingDirection, .natural)
        XCTAssertEqual(textView.textAlignment, .natural)
        XCTAssertEqual(textView.semanticContentAttribute, .unspecified)

        let a = mobileBotConversationScopeKey(accountScopeKey: "account-a", agentID: "agent")
        let b = mobileBotConversationScopeKey(accountScopeKey: "account-b", agentID: "agent")
        let c = mobileBotConversationScopeKey(accountScopeKey: "account-a", agentID: "agent-b")
        XCTAssertNotEqual(a, b)
        XCTAssertNotEqual(a, c)
        XCTAssertNotEqual(b, c)
    }

    private func host(_ textView: MobileComposerUITextView) -> UIWindow {
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 390, height: 160))
        let controller = UIViewController()
        window.rootViewController = controller
        controller.view.addSubview(textView)
        textView.frame = CGRect(x: 0, y: 0, width: 390, height: 120)
        window.makeKeyAndVisible()
        XCTAssertTrue(textView.becomeFirstResponder())
        return window
    }
}

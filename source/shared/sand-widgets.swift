import Foundation

enum WidgetActionStyle: String, Codable, Equatable, Sendable {
    case defaultStyle = "default"
    case primary
    case danger
}

struct SandWidgetChoiceOption: Codable, Equatable, Sendable {
    let label: String
    var value: String? = nil
    var description: String? = nil
    var style: WidgetActionStyle? = nil

    var normalizedValue: String { value ?? label }

    var isValid: Bool {
        !label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && (value == nil || !value!.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            && (description == nil || !description!.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }
}

struct SandWidget: Codable, Equatable, Sendable {
    let prompt: String
    var helpText: String? = nil
    let options: [SandWidgetChoiceOption]
    var allowCustom: Bool? = nil
    var dismissOnMoveOn: Bool? = nil

    var isValid: Bool {
        !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && (1...6).contains(options.count)
            && options.allSatisfy(\.isValid)
            && (helpText == nil || !helpText!.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }
}

func summarizeWidget(_ widget: SandWidget?) -> String {
    guard let widget else { return "Question" }
    let prompt = widget.prompt.isEmpty ? "Question" : widget.prompt
    let labels = widget.options.map(\.label).joined(separator: " / ")
    return labels.isEmpty ? prompt : "\(prompt) — \(labels)"
}

func getWidgetAnswerLabel(_ widget: SandWidget, answer: String) -> String {
    widget.options.first(where: { $0.normalizedValue == answer })?.label ?? answer
}

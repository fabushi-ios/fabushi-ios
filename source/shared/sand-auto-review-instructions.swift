import Foundation

let SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES = 20
let SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS = 1_000

struct SandAutoReviewInstructions: Equatable, Sendable {
    let isEnabled: Bool
    let allowInstructions: [String]
    let blockInstructions: [String]
}

let DEFAULT_SAND_AUTO_REVIEW_INSTRUCTIONS = SandAutoReviewInstructions(
    isEnabled: true,
    allowInstructions: [],
    blockInstructions: []
)

enum SandAutoReviewInstructionBehavior: String, CaseIterable, Hashable, Sendable {
    case allow
    case ask
}

struct SandAutoReviewInstructionRow: Identifiable, Equatable, Hashable, Sendable {
    let behavior: SandAutoReviewInstructionBehavior
    let text: String
    let listIndex: Int

    var id: String {
        "\(behavior.rawValue):\(listIndex):\(text)"
    }
}

func sandAutoReviewInstructionRows(
    _ instructions: SandAutoReviewInstructions
) -> [SandAutoReviewInstructionRow] {
    let allow = instructions.allowInstructions.enumerated().map {
        SandAutoReviewInstructionRow(
            behavior: .allow,
            text: $0.element,
            listIndex: $0.offset
        )
    }
    let ask = instructions.blockInstructions.enumerated().map {
        SandAutoReviewInstructionRow(
            behavior: .ask,
            text: $0.element,
            listIndex: $0.offset
        )
    }
    return allow + ask
}

func removeSandAutoReviewInstruction(
    _ instructions: SandAutoReviewInstructions,
    row: SandAutoReviewInstructionRow
) -> SandAutoReviewInstructions {
    var allow = instructions.allowInstructions
    var ask = instructions.blockInstructions
    switch row.behavior {
    case .allow:
        guard allow.indices.contains(row.listIndex) else { return instructions }
        allow.remove(at: row.listIndex)
    case .ask:
        guard ask.indices.contains(row.listIndex) else { return instructions }
        ask.remove(at: row.listIndex)
    }
    return .init(
        isEnabled: instructions.isEnabled,
        allowInstructions: allow,
        blockInstructions: ask
    )
}

func reconcileSandAutoReviewInstructionRow(
    _ instructions: SandAutoReviewInstructions,
    row: SandAutoReviewInstructionRow
) -> SandAutoReviewInstructionRow? {
    let list = row.behavior == .allow
        ? instructions.allowInstructions
        : instructions.blockInstructions
    guard let index = list.firstIndex(of: row.text) else { return nil }
    return .init(
        behavior: row.behavior,
        text: row.text,
        listIndex: index
    )
}

func saveSandAutoReviewInstruction(
    _ instructions: SandAutoReviewInstructions,
    text: String,
    behavior: SandAutoReviewInstructionBehavior,
    editing: SandAutoReviewInstructionRow?
) -> SandAutoReviewInstructions? {
    let normalized = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !normalized.isEmpty else { return nil }

    var allow = instructions.allowInstructions
    var ask = instructions.blockInstructions
    var target = behavior == .allow ? allow : ask
    let sameList = editing?.behavior == behavior
    let duplicate = target.enumerated().contains { index, value in
        value == normalized && !(sameList && index == editing?.listIndex)
    }
    guard !duplicate else { return nil }

    if editing == nil {
        guard target.count < SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES else { return nil }
        target.append(normalized)
    } else if sameList, let editing {
        guard target.indices.contains(editing.listIndex) else { return nil }
        target[editing.listIndex] = normalized
    } else if let editing {
        guard target.count < SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES else { return nil }
        let withoutPrevious = removeSandAutoReviewInstruction(
            instructions,
            row: editing
        )
        allow = withoutPrevious.allowInstructions
        ask = withoutPrevious.blockInstructions
        target = behavior == .allow ? allow : ask
        target.append(normalized)
    }

    if behavior == .allow {
        allow = target
    } else {
        ask = target
    }
    return normalizeSandAutoReviewInstructions(
        isEnabled: instructions.isEnabled,
        allowInstructions: allow,
        blockInstructions: ask
    )
}

private func clampSandInstructionUTF16(_ value: String) -> String {
    var result = ""
    var units = 0
    for character in value {
        let part = String(character)
        let next = part.utf16.count
        guard units + next <= SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS else { break }
        result.append(character)
        units += next
    }
    return result
}

private func normalizeSandInstructionList(_ raw: [Any]?) -> [String] {
    guard let raw else { return [] }
    var result: [String] = []
    var seen = Set<String>()
    for item in raw {
        guard let item = item as? String else { continue }
        let trimmed = item.trimmingCharacters(in: .whitespacesAndNewlines)
        let clamped = clampSandInstructionUTF16(trimmed)
        guard !clamped.isEmpty, seen.insert(clamped).inserted else { continue }
        result.append(clamped)
        if result.count >= SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES { break }
    }
    return result
}

func normalizeSandAutoReviewInstructions(
    isEnabled: Any? = nil,
    allowInstructions: [Any]? = nil,
    blockInstructions: [Any]? = nil
) -> SandAutoReviewInstructions {
    let enabled = (isEnabled as? Bool) != false
    return .init(
        isEnabled: enabled,
        allowInstructions: normalizeSandInstructionList(allowInstructions),
        blockInstructions: normalizeSandInstructionList(blockInstructions)
    )
}

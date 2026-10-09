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

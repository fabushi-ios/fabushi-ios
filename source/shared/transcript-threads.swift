import Foundation

struct BranchedTranscriptEntry: Equatable, Sendable {
    let id: String
    var replyTo: String? = nil
}

private func resolveBranchedTranscriptRoot(
    _ entry: BranchedTranscriptEntry,
    branchedById: [String: BranchedTranscriptEntry]
) -> String? {
    var current = entry
    var seen: Set<String> = [entry.id]
    while true {
        guard let parentId = current.replyTo else { return nil }
        guard let parent = branchedById[parentId] else { return parentId }
        if seen.contains(parentId) { return nil }
        seen.insert(parentId)
        current = parent
    }
}

func branchReplyCounts(_ branched: [BranchedTranscriptEntry]) -> [String: Int] {
    var byId: [String: BranchedTranscriptEntry] = [:]
    for entry in branched { byId[entry.id] = entry }
    var counts: [String: Int] = [:]
    for entry in branched {
        guard let root = resolveBranchedTranscriptRoot(entry, branchedById: byId) else { continue }
        counts[root, default: 0] += 1
    }
    return counts
}

func threadDescendants(_ rootId: String, branched: [BranchedTranscriptEntry]) -> [BranchedTranscriptEntry] {
    var byId: [String: BranchedTranscriptEntry] = [:]
    for entry in branched { byId[entry.id] = entry }
    return branched.filter { resolveBranchedTranscriptRoot($0, branchedById: byId) == rootId }
}

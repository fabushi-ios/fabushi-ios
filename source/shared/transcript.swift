import Foundation

let SAND_AUTO_REVIEW_STALE = "auto-review/stale"
let SAND_AUTO_REVIEW_STALE_MESSAGE = "The Auto-review request is stale, expired, or not authorized."
let SAND_REACTION_SELF = "me"
let SAND_REACTION_AGENT = "agent"

struct TranscriptApproval: Equatable, Sendable {
    let requestId: String
    var status: String
}

struct TranscriptAsk: Equatable, Sendable {
    let requestId: String
    var status: String
}

struct TranscriptMessagePayload: Equatable, Sendable {
    let type: String?
    var approval: TranscriptApproval? = nil
    var ask: TranscriptAsk? = nil
}

struct TranscriptAgentTarget: Equatable, Sendable {
    let kind: String?
}

struct TranscriptEntry: Equatable, Sendable {
    let kind: String
    var message: TranscriptMessagePayload? = nil
    var id: String? = nil
    var replyTo: String? = nil
    var branched = false
    var fromAgent: String? = nil
    var toAgent: TranscriptAgentTarget? = nil
}

func settlePendingAutoReviewApprovalEntry(
    _ entry: TranscriptEntry,
    status: String,
    requestId: String? = nil
) -> TranscriptEntry? {
    guard entry.kind == "send-message",
          entry.message?.type == "auto-review-approval",
          let approval = entry.message?.approval,
          approval.status == "pending",
          requestId == nil || approval.requestId == requestId else { return nil }
    var copy = entry
    var message = copy.message!
    var nextApproval = approval
    nextApproval.status = status
    message.approval = nextApproval
    copy.message = message
    return copy
}

func settlePendingLocalToolPermissionEntry(
    _ entry: TranscriptEntry,
    status: String,
    requestId: String? = nil
) -> TranscriptEntry? {
    guard entry.kind == "send-message",
          entry.message?.type == "local-tool-permission",
          let ask = entry.message?.ask,
          ask.status == "pending",
          requestId == nil || ask.requestId == requestId else { return nil }
    var copy = entry
    var message = copy.message!
    var nextAsk = ask
    nextAsk.status = status
    message.ask = nextAsk
    copy.message = message
    return copy
}

private func transcriptReplyTo(_ entry: TranscriptEntry) -> String? {
    ["message", "send-message", "user-attachment", "notice"].contains(entry.kind)
        ? entry.replyTo
        : nil
}

private func isBranchedTranscriptEntry(_ entry: TranscriptEntry) -> Bool {
    ["message", "send-message", "user-attachment", "notice"].contains(entry.kind) && entry.branched
}

private func resolveTranscriptBranchRoot(
    _ entry: TranscriptEntry,
    byId: [String: TranscriptEntry]
) -> String? {
    var current = entry
    var seen = Set<String>()
    if let id = entry.id { seen.insert(id) }
    while true {
        guard let parentId = transcriptReplyTo(current),
              let parent = byId[parentId] else { return nil }
        if !isBranchedTranscriptEntry(parent) { return parentId }
        if seen.contains(parentId) { return nil }
        seen.insert(parentId)
        current = parent
    }
}

func getMainTranscriptEntries(_ entries: [TranscriptEntry]) -> [TranscriptEntry] {
    var byId: [String: TranscriptEntry] = [:]
    for entry in entries {
        if let id = entry.id { byId[id] = entry }
    }
    let hasThread = entries.contains {
        isBranchedTranscriptEntry($0) && resolveTranscriptBranchRoot($0, byId: byId) != nil
    }
    guard hasThread else { return entries }
    return entries.filter {
        !(isBranchedTranscriptEntry($0) && resolveTranscriptBranchRoot($0, byId: byId) != nil)
    }
}

func getThreadTranscriptEntries(_ entries: [TranscriptEntry], rootId: String) -> [TranscriptEntry] {
    var byId: [String: TranscriptEntry] = [:]
    for entry in entries {
        if let id = entry.id { byId[id] = entry }
    }
    guard byId[rootId] != nil else { return [] }
    var children: [String: [String]] = [:]
    for candidate in entries {
        guard let parentId = transcriptReplyTo(candidate), let id = candidate.id else { continue }
        children[parentId, default: []].append(id)
    }
    var thread: Set<String> = [rootId]
    var queue = [rootId]
    while !queue.isEmpty {
        let nextId = queue.removeFirst()
        for childId in children[nextId] ?? [] {
            guard !thread.contains(childId),
                  let child = byId[childId],
                  isBranchedTranscriptEntry(child) else { continue }
            thread.insert(childId)
            queue.append(childId)
        }
    }
    return entries.filter { $0.id.map(thread.contains) == true }
}

func isAgentPeerMessageEntry(_ entry: TranscriptEntry?) -> Bool {
    guard let entry else { return false }
    return entry.kind == "message" && (entry.fromAgent != nil || entry.toAgent != nil)
}

func isOutboundAgentPeerMessageEntry(_ entry: TranscriptEntry?) -> Bool {
    guard let entry else { return false }
    return entry.kind == "message" && entry.toAgent != nil
}

func isHiddenOutboundAgentPeerMessageEntry(_ entry: TranscriptEntry?) -> Bool {
    isOutboundAgentPeerMessageEntry(entry) && entry?.toAgent?.kind != "agent"
}

func entryRaisesUserActivitySignal(_ entry: TranscriptEntry) -> Bool {
    !isAgentPeerMessageEntry(entry)
}

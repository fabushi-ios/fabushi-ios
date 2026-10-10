import Foundation

protocol AgentSummaryIdentity {
    var id: String { get }
    var updatedAt: Double { get }
}

/// Mirrors the numeric comparator used by the Grok renderer.
func compareAgentSummaries<T: AgentSummaryIdentity>(_ a: T, _ b: T) -> Double {
    b.updatedAt - a.updatedAt
}

func upsertAgentSummary<T: AgentSummaryIdentity>(
    _ summaries: [T],
    updated: T
) -> [T] {
    var found = false
    var next = summaries.map { summary in
        guard summary.id == updated.id else { return summary }
        found = true
        return updated
    }
    if !found {
        next.append(updated)
    }
    return next.enumerated()
        .sorted { left, right in
            if left.element.updatedAt == right.element.updatedAt {
                return left.offset < right.offset
            }
            return compareAgentSummaries(left.element, right.element) < 0
        }
        .map(\.element)
}

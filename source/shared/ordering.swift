import Foundation

enum ReplicaOrdering {
    static let rosterReplicaKey = "roster"
    static let orderedReplicasV1 = "orderedReplicasV1"

    static func transcriptReplicaKey(agentID: String) -> String {
        "transcript:\(agentID)"
    }
}

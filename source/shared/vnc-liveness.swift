import Foundation

struct VNCLivenessReport: Equatable, Sendable {
    let phase: String
    let stallMilliseconds: Double
    let keys: Double
    let clicks: Double
    let moves: Double
    let inBytes: Double
    var isValid: Bool {
        phase == "post_connect"
            && [stallMilliseconds, keys, clicks, moves, inBytes].allSatisfy { $0.isFinite && $0 >= 0 }
    }
}
enum VNCLivenessContract {
    static let channel = "sand:vnc-liveness"
    static let windowMilliseconds = 10_000
    static let minimumImpactfulInputs = 3
    static func isValid(_ report: VNCLivenessReport) -> Bool { report.isValid }
}

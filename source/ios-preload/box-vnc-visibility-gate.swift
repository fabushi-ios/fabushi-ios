import Foundation

struct IOSVNCViewerVisibilityGate: Equatable, Sendable {
    private(set) var visible = false

    mutating func update(_ value: Bool) -> Bool {
        let becameVisible = value && !visible
        visible = value
        return becameVisible
    }
}

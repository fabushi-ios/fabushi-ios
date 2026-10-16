import Foundation

struct CursorAccountStatus: Equatable, Sendable {
    let kind: String
    let authId: String?
    let email: String?

    init(kind: String, authId: String? = nil, email: String? = nil) {
        self.kind = kind
        self.authId = authId
        self.email = email
    }
}

func cursorAccountSlot(_ status: CursorAccountStatus) -> String? {
    guard status.kind == "logged-in" else { return nil }
    let slot = status.authId ?? status.email
    guard let slot, !slot.isEmpty else { return nil }
    return slot
}

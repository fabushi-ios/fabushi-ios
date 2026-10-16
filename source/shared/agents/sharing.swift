import Foundation

let REMOTE_AGENT_ID_PREFIX = "sand-remote:"

struct RemoteAgentReference: Codable, Equatable, Sendable {
    let ownerAuthId: String
    let agentId: String
}

private let sandURIComponentAllowed: CharacterSet = {
    var allowed = CharacterSet.alphanumerics
    allowed.formUnion(CharacterSet(charactersIn: "-_.!~*'()"))
    return allowed
}()

private func sandEncodeURIComponent(_ value: String) -> String {
    value.addingPercentEncoding(withAllowedCharacters: sandURIComponentAllowed) ?? ""
}

func formatRemoteAgentId(_ reference: RemoteAgentReference) -> String {
    REMOTE_AGENT_ID_PREFIX +
        sandEncodeURIComponent(reference.ownerAuthId) +
        "/" +
        sandEncodeURIComponent(reference.agentId)
}

func isRemoteAgentId(_ id: String) -> Bool {
    id.hasPrefix(REMOTE_AGENT_ID_PREFIX)
}

func parseRemoteAgentId(_ id: String) -> RemoteAgentReference? {
    guard id.hasPrefix(REMOTE_AGENT_ID_PREFIX) else { return nil }
    let body = String(id.dropFirst(REMOTE_AGENT_ID_PREFIX.count))
    guard let separator = body.firstIndex(of: "/"),
          separator != body.startIndex,
          body.index(after: separator) != body.endIndex
    else {
        return nil
    }

    let ownerEncoded = String(body[..<separator])
    let agentEncoded = String(body[body.index(after: separator)...])
    guard let ownerAuthId = ownerEncoded.removingPercentEncoding,
          let agentId = agentEncoded.removingPercentEncoding,
          !ownerAuthId.isEmpty,
          !agentId.isEmpty
    else {
        return nil
    }
    return .init(ownerAuthId: ownerAuthId, agentId: agentId)
}

let SAND_SHARED_ROOM_IMAGE_BYTES_MAX = 1_100_000
let SAND_SHARE_AVATAR_DATA_URL_MAX_LENGTH = 200_000

func isPublishableShareAvatarDataUrl(_ value: String) -> Bool {
    guard value.count <= SAND_SHARE_AVATAR_DATA_URL_MAX_LENGTH else { return false }
    return value.range(
        of: #"^data:image/(?:png|jpeg|webp|gif);base64,[A-Za-z0-9+/]+={0,2}$"#,
        options: .regularExpression
    ) != nil
}

struct SandSharingState: Equatable, Sendable {
    let isEnabled: Bool
    let selfAuthId: String?
    let pendingJoinRequests: [String]
    let rooms: [String]
    let typingUsers: [String]
}

let EMPTY_SAND_SHARING_STATE = SandSharingState(
    isEnabled: false,
    selfAuthId: nil,
    pendingJoinRequests: [],
    rooms: [],
    typingUsers: []
)

import Foundation

func isValidSentryConversationReport(_ value: Any?) -> Bool {
    guard let record = value as? [String: Any],
          let agentId = record["agentId"]
    else {
        return false
    }
    if agentId is NSNull {
        return true
    }
    return isSandSentryBoundedTagValue(agentId)
}

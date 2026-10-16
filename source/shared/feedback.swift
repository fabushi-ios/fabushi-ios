import Foundation

let SAND_FEEDBACK_MESSAGE_MAX_CHARS = 10_000
let SAND_FEEDBACK_CONVERSATION_ID_MAX_CHARS = 512
let SAND_FEEDBACK_ACCOUNT_SLOT_MAX_CHARS = 512
let SAND_FEEDBACK_SENTRY_EVENT_ID_MAX_COUNT = 5
let SAND_FEEDBACK_SENTRY_EVENT_ID_PATTERN = #"^[0-9a-f]{32}$"#

func isValidFeedbackSentryEventID(_ value: String) -> Bool {
    value.range(of: SAND_FEEDBACK_SENTRY_EVENT_ID_PATTERN, options: .regularExpression) != nil
}

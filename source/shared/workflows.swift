import Foundation

let SAND_SKILL_PUBLISH_REFUSED = "skill-publish/refused"
let SKILL_PUBLISH_REFUSAL_PREFIX = "\(SAND_SKILL_PUBLISH_REFUSED): "
let WORKFLOW_REFERENCE_NODE_TYPE = "workflowReference"

struct SandSkillPublishError: LocalizedError, Equatable, Sendable {
    let reason: String
    var errorDescription: String? { SKILL_PUBLISH_REFUSAL_PREFIX + reason }
}

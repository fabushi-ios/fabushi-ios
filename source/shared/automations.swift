import Foundation

let TRIGGER_ANY_SCOPE = "*"
let GITHUB_EVENT_KINDS = [
    "pr-opened","pr-pushed","pr-merged","review-requested","review-approved","review-changes-requested",
    "review-commented","pr-comment","inline-review-comment","review-thread-resolved","review-thread-unresolved",
    "issue-assigned","ci-passed","ci-failed",
]
let LINEAR_EVENT_CASES = ["issueCreated","statusChanged","endOfCycle"]
let SENTRY_EVENT_CASES = ["issueCreated","issueResolved","issueAssigned","issueArchived","issueUnresolved","issueAny"]
let PAGERDUTY_EVENT_CASES = ["incidentTriggered","incidentAcknowledged","incidentResolved","incidentEscalated","incidentAny"]
let TRIGGER_MAX_GROUP_LISTENERS = 8
let TRIGGER_MAX_REACTION_EMOJI = 8
let LISTENER_INTEGRATION_PLATFORMS = ["github","slack"]
let AUTOMATION_WAKE_CUE = "[routine]"

struct CronTrigger: Equatable, Sendable { let schedule: String }

enum SlackMatch: Equatable, Sendable {
    case mention
    case message
    case keyword(String)
    case reaction(emoji: [String], bySelf: Bool?)
}

struct SlackTrigger: Equatable, Sendable {
    let channel: String
    let match: SlackMatch
}

struct GithubTrigger: Equatable, Sendable {
    let repo: String
    let events: [String]
    var ciBranch: String? = nil
    var userAllowlist: [String]? = nil
}

struct MicrosoftTeamsTrigger: Equatable, Sendable {
    let tenantId: String
    let teamId: String
    let teamIds: [String]
    let channelIds: [String]
    let messageContains: String
    let messageContainsIsRegex: Bool
    let blockUnauthenticatedTeamsUsers: Bool
}

struct CaseTrigger: Equatable, Sendable {
    enum Platform: String, Equatable, Sendable { case linear, sentry, pagerduty }
    let platform: Platform
    let eventCase: String
    var projectIds: [String] = []
    var teamIds: [String] = []
    var serviceIds: [String] = []
    var statusIds: [String] = []
    var cycleIds: [String] = []
}

enum AutomationTriggerMember: Equatable, Sendable {
    case cron(CronTrigger)
    case slack(SlackTrigger)
    case github(GithubTrigger)
    case microsoftTeams(MicrosoftTeamsTrigger)
    case integration(CaseTrigger)
}

enum AutomationTrigger: Equatable, Sendable {
    case member(AutomationTriggerMember)
    case group([AutomationTriggerMember])
}

func isGithubCiEventKind(_ kind: String) -> Bool {
    kind == "ci-passed" || kind == "ci-failed"
}

func normalizeReactionEmoji(_ raw: String) -> String {
    var bare = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    while bare.hasPrefix(":") { bare.removeFirst() }
    while bare.hasSuffix(":") { bare.removeLast() }
    return (bare.components(separatedBy: "::").first ?? bare)
        .trimmingCharacters(in: .whitespacesAndNewlines)
        .lowercased()
}

func isValidReactionEmoji(_ value: String) -> Bool {
    !value.isEmpty && value.allSatisfy { $0.isLowercase || $0.isNumber || "_+-".contains($0) }
}

func isValidGithubRepo(_ repo: String) -> Bool {
    let parts = repo.split(separator: "/", omittingEmptySubsequences: false)
    return parts.count == 2 && parts.allSatisfy { !$0.isEmpty && !$0.contains(where: { $0.isWhitespace }) }
}

func isValidGitBranch(_ branch: String) -> Bool {
    guard !branch.isEmpty,
          !branch.hasPrefix("-"), !branch.hasPrefix("/"), !branch.hasSuffix("/"),
          !branch.contains(".."), !branch.contains("@{") else { return false }
    return !branch.contains { $0.isWhitespace || "~^:?*[\\]|".contains($0) }
}

func cronTrigger(_ schedule: String) -> CronTrigger { .init(schedule: schedule) }

func triggerList(_ trigger: AutomationTrigger) -> [AutomationTriggerMember] {
    switch trigger {
    case .member(let member): return [member]
    case .group(let listeners): return listeners
    }
}

func triggerFromList(_ members: [AutomationTriggerMember]) -> AutomationTrigger? {
    guard let first = members.first else { return nil }
    return members.count == 1 ? .member(first) : .group(members)
}

func triggerListeners(_ trigger: AutomationTrigger) -> [AutomationTriggerMember] {
    triggerList(trigger).filter {
        switch $0 {
        case .slack, .github: return true
        default: return false
        }
    }
}

func triggerEventTriggers(_ trigger: AutomationTrigger) -> [AutomationTriggerMember] {
    triggerList(trigger).filter {
        if case .cron = $0 { return false }
        return true
    }
}

func triggerCronSchedules(_ trigger: AutomationTrigger) -> [String] {
    triggerList(trigger).compactMap {
        if case .cron(let cron) = $0 { return cron.schedule }
        return nil
    }
}

func triggerSchedule(_ trigger: AutomationTrigger) -> String? {
    triggerCronSchedules(trigger).first
}

private func joinWithOr(_ parts: [String]) -> String {
    if parts.count <= 1 { return parts.first ?? "" }
    if parts.count == 2 { return "\(parts[0]) or \(parts[1])" }
    return "\(parts.dropLast().joined(separator: ", ")), or \(parts.last!)"
}

private func slackScope(_ channel: String) -> String {
    channel == TRIGGER_ANY_SCOPE ? "anywhere on Slack" : "in \(channel)"
}

func describeSlackListener(_ listener: SlackTrigger) -> String {
    let scope = slackScope(listener.channel)
    switch listener.match {
    case .mention:
        return "When @mentioned \(scope)"
    case .message:
        return "On any message \(scope)"
    case .keyword(let keyword):
        return "When \"\(keyword)\" is mentioned \(scope)"
    case .reaction(let emoji, let bySelf):
        let names = joinWithOr(emoji.map { ":\($0):" })
        if bySelf == true {
            return "When you react\(emoji.isEmpty ? "" : " \(names)") \(scope)"
        }
        return "On \(emoji.isEmpty ? "a reaction" : names) \(scope)"
    }
}

private let GITHUB_PHRASES = [
    "pr-opened":"a PR opens","pr-pushed":"a PR is updated","pr-merged":"a PR merges",
    "review-requested":"a review is requested","review-approved":"a review approves a PR",
    "review-changes-requested":"a review requests changes","review-commented":"a review comments on a PR",
    "pr-comment":"a PR comment lands","inline-review-comment":"an inline review comment lands",
    "review-thread-resolved":"a review thread is resolved","review-thread-unresolved":"a review thread is reopened",
    "issue-assigned":"an issue is assigned","ci-passed":"CI passes","ci-failed":"CI fails",
]

func describeGithubListener(_ listener: GithubTrigger) -> String {
    let phrases = listener.events.map { kind -> String in
        var phrase = GITHUB_PHRASES[kind] ?? kind
        if isGithubCiEventKind(kind), let branch = listener.ciBranch { phrase += " on \(branch)" }
        return phrase
    }
    let base = "When \(joinWithOr(phrases)) in \(listener.repo)"
    guard let allowlist = listener.userAllowlist, !allowlist.isEmpty else { return base }
    let users = allowlist.map { $0.hasPrefix("@") ? $0 : "@\($0)" }
    return "\(base) (by \(joinWithOr(users)))"
}

private let CASE_PHRASES: [CaseTrigger.Platform: [String: String]] = [
    .linear:["issueCreated":"a Linear issue is created","statusChanged":"a Linear issue changes status","endOfCycle":"a Linear cycle ends"],
    .sentry:["issueCreated":"a Sentry issue is created","issueResolved":"a Sentry issue is resolved","issueAssigned":"a Sentry issue is assigned","issueArchived":"a Sentry issue is archived","issueUnresolved":"a Sentry issue becomes unresolved","issueAny":"a Sentry issue changes"],
    .pagerduty:["incidentTriggered":"a PagerDuty incident is triggered","incidentAcknowledged":"a PagerDuty incident is acknowledged","incidentResolved":"a PagerDuty incident is resolved","incidentEscalated":"a PagerDuty incident is escalated","incidentAny":"a PagerDuty incident changes"],
]

func describeListener(_ listener: AutomationTriggerMember) -> String {
    switch listener {
    case .cron(let cron): return cron.schedule
    case .slack(let slack): return describeSlackListener(slack)
    case .github(let github): return describeGithubListener(github)
    case .microsoftTeams(let teams):
        return teams.messageContains.isEmpty ? "On a Microsoft Teams message" : "When a Microsoft Teams message matches \"\(teams.messageContains)\""
    case .integration(let integration):
        return "When \(CASE_PHRASES[integration.platform]?[integration.eventCase] ?? integration.eventCase)"
    }
}

enum RoutineTriggerForm: Equatable, Sendable {
    case schedule(String)
    case slack(channel: String, match: SlackMatch)
    case github(repo: String, events: [String], userAllowlist: String, ciBranch: String)
    case microsoftTeams(
        tenantId: String,
        teamIds: String,
        channelIds: String,
        messageContains: String,
        messageContainsIsRegex: Bool,
        blockUnauthenticatedTeamsUsers: Bool
    )
    case linear(
        eventCase: String,
        statusIds: String,
        cycleIds: String,
        projectIds: String,
        teamIds: String
    )
    case sentry(eventCase: String, projectIds: String)
    case pagerduty(eventCase: String, serviceIds: String)
}

private func routineTokens(_ value: String) -> [String] {
    value
        .split(whereSeparator: { $0.isWhitespace || $0 == "," })
        .map(String.init)
        .filter { !$0.isEmpty }
}

private func normalizedRoutineAllowlist(_ value: String) -> [String] {
    var result: [String] = []
    for token in routineTokens(value) {
        let item = token.drop(while: { $0 == "@" })
        guard !item.isEmpty else { continue }
        let normalized = String(item)
        if !result.contains(where: { $0.caseInsensitiveCompare(normalized) == .orderedSame }) {
            result.append(normalized)
        }
    }
    return result
}

private func normalizedRoutineEmoji(_ values: [String]) -> [String] {
    var result: [String] = []
    for raw in values {
        let normalized = normalizeReactionEmoji(raw)
        guard isValidReactionEmoji(normalized), !result.contains(normalized) else { continue }
        result.append(normalized)
        if result.count == TRIGGER_MAX_REACTION_EMOJI { break }
    }
    return result
}

func routineTriggerFormToMember(_ form: RoutineTriggerForm) -> AutomationTriggerMember? {
    switch form {
    case .schedule(let raw):
        let schedule = normalizeSchedule(raw)
        guard !schedule.isEmpty,
              parseEveryIntervalMs(schedule) != nil || compileCronMatcher(schedule) != nil
        else { return nil }
        return .cron(.init(schedule: schedule))

    case .slack(let channelRaw, let match):
        let channel = channelRaw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !channel.isEmpty else { return nil }
        switch match {
        case .keyword(let raw):
            let keyword = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            return keyword.isEmpty ? nil : .slack(.init(channel: channel, match: .keyword(keyword)))
        case .reaction(let emoji, let bySelf):
            return .slack(.init(
                channel: channel,
                match: .reaction(emoji: normalizedRoutineEmoji(emoji), bySelf: bySelf)
            ))
        case .mention:
            return .slack(.init(channel: channel, match: .mention))
        case .message:
            return .slack(.init(channel: channel, match: .message))
        }

    case .github(let repoRaw, let events, let allowlistRaw, let branchRaw):
        let repo = repoRaw.trimmingCharacters(in: .whitespacesAndNewlines)
        let branch = branchRaw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isValidGithubRepo(repo),
              !events.isEmpty,
              events.allSatisfy({ GITHUB_EVENT_KINDS.contains($0) })
        else { return nil }
        let needsBranch = events.contains(where: isGithubCiEventKind)
        guard !needsBranch || isValidGitBranch(branch) else { return nil }
        return .github(.init(
            repo: repo,
            events: events,
            ciBranch: needsBranch ? branch : nil,
            userAllowlist: normalizedRoutineAllowlist(allowlistRaw).nilIfEmpty
        ))

    case .microsoftTeams(
        let tenantRaw,
        let teamIdsRaw,
        let channelIdsRaw,
        let messageRaw,
        let isRegex,
        let blockUnauthenticated
    ):
        let tenant = tenantRaw.trimmingCharacters(in: .whitespacesAndNewlines)
        let teamIds = routineTokens(teamIdsRaw)
        guard !tenant.isEmpty, !teamIds.isEmpty else { return nil }
        return .microsoftTeams(.init(
            tenantId: tenant,
            teamId: "",
            teamIds: teamIds,
            channelIds: routineTokens(channelIdsRaw),
            messageContains: messageRaw.trimmingCharacters(in: .whitespacesAndNewlines),
            messageContainsIsRegex: isRegex,
            blockUnauthenticatedTeamsUsers: blockUnauthenticated
        ))

    case .linear(let eventCase, let statusIds, let cycleIds, let projectIds, let teamIds):
        guard LINEAR_EVENT_CASES.contains(eventCase) else { return nil }
        return .integration(.init(
            platform: .linear,
            eventCase: eventCase,
            projectIds: routineTokens(projectIds),
            teamIds: routineTokens(teamIds),
            statusIds: eventCase == "statusChanged" ? routineTokens(statusIds) : [],
            cycleIds: eventCase == "endOfCycle" ? routineTokens(cycleIds) : []
        ))

    case .sentry(let eventCase, let projectIds):
        guard SENTRY_EVENT_CASES.contains(eventCase) else { return nil }
        return .integration(.init(
            platform: .sentry,
            eventCase: eventCase,
            projectIds: routineTokens(projectIds)
        ))

    case .pagerduty(let eventCase, let serviceIds):
        guard PAGERDUTY_EVENT_CASES.contains(eventCase) else { return nil }
        return .integration(.init(
            platform: .pagerduty,
            eventCase: eventCase,
            serviceIds: routineTokens(serviceIds)
        ))
    }
}

func routineTriggerFormIsValid(_ form: RoutineTriggerForm) -> Bool {
    routineTriggerFormToMember(form) != nil
}

func routineTriggerFromForms(_ forms: [RoutineTriggerForm]) -> AutomationTrigger? {
    guard !forms.isEmpty, forms.count <= TRIGGER_MAX_GROUP_LISTENERS else { return nil }
    var members: [AutomationTriggerMember] = []
    for form in forms {
        guard let member = routineTriggerFormToMember(form) else { return nil }
        members.append(member)
    }
    return triggerFromList(members)
}

func routineTriggerForm(from member: AutomationTriggerMember) -> RoutineTriggerForm {
    switch member {
    case .cron(let cron):
        return .schedule(cron.schedule)
    case .slack(let slack):
        return .slack(channel: slack.channel, match: slack.match)
    case .github(let github):
        return .github(
            repo: github.repo,
            events: github.events,
            userAllowlist: (github.userAllowlist ?? []).joined(separator: ", "),
            ciBranch: github.ciBranch ?? ""
        )
    case .microsoftTeams(let teams):
        return .microsoftTeams(
            tenantId: teams.tenantId,
            teamIds: (teams.teamIds.isEmpty ? [teams.teamId] : teams.teamIds)
                .filter { !$0.isEmpty }
                .joined(separator: ", "),
            channelIds: teams.channelIds.joined(separator: ", "),
            messageContains: teams.messageContains,
            messageContainsIsRegex: teams.messageContainsIsRegex,
            blockUnauthenticatedTeamsUsers: teams.blockUnauthenticatedTeamsUsers
        )
    case .integration(let integration):
        switch integration.platform {
        case .linear:
            return .linear(
                eventCase: integration.eventCase,
                statusIds: integration.statusIds.joined(separator: ", "),
                cycleIds: integration.cycleIds.joined(separator: ", "),
                projectIds: integration.projectIds.joined(separator: ", "),
                teamIds: integration.teamIds.joined(separator: ", ")
            )
        case .sentry:
            return .sentry(
                eventCase: integration.eventCase,
                projectIds: integration.projectIds.joined(separator: ", ")
            )
        case .pagerduty:
            return .pagerduty(
                eventCase: integration.eventCase,
                serviceIds: integration.serviceIds.joined(separator: ", ")
            )
        }
    }
}

func routineTriggerForms(from trigger: AutomationTrigger) -> [RoutineTriggerForm]? {
    let members = triggerList(trigger)
    guard !members.isEmpty, members.count <= TRIGGER_MAX_GROUP_LISTENERS else { return nil }
    let forms = members.map(routineTriggerForm(from:))
    return forms.allSatisfy(routineTriggerFormIsValid) ? forms : nil
}

private extension Array {
    var nilIfEmpty: Self? { isEmpty ? nil : self }
}

private func routineEventWire(
    source: String,
    event: String,
    filters: [String: Any]
) -> [String: Any] {
    var value: [String: Any] = [
        "kind": "event",
        "source": source,
        "event": event,
    ]
    if !filters.isEmpty {
        value["filters"] = filters
    }
    return value
}

func routineTriggerMemberWireValue(_ member: AutomationTriggerMember) -> [String: Any] {
    switch member {
    case .cron(let cron):
        return [
            "kind": "schedule",
            "schedule": cron.schedule,
        ]

    case .slack(let slack):
        var filters: [String: Any] = ["channel": slack.channel]
        let event: String
        switch slack.match {
        case .mention:
            event = "mention"
        case .message:
            event = "message"
        case .keyword(let keyword):
            event = "message"
            filters["messageContains"] = keyword
        case .reaction(let emoji, let bySelf):
            event = "reaction"
            if !emoji.isEmpty {
                filters["emoji"] = emoji
            }
            if let bySelf {
                filters["bySelf"] = bySelf
            }
        }
        return routineEventWire(source: "slack", event: event, filters: filters)

    case .github(let github):
        var filters: [String: Any] = [
            "repo": github.repo,
            "events": github.events,
        ]
        if let branch = github.ciBranch, !branch.isEmpty {
            filters["ciBranch"] = branch
        }
        if let users = github.userAllowlist, !users.isEmpty {
            filters["actorAllowlist"] = users
        }
        return routineEventWire(source: "github", event: "*", filters: filters)

    case .microsoftTeams(let teams):
        var filters: [String: Any] = [
            "tenantId": teams.tenantId,
            "teamIds": teams.teamIds.isEmpty ? [teams.teamId] : teams.teamIds,
            "channelIds": teams.channelIds,
            "messageContains": teams.messageContains,
            "messageContainsIsRegex": teams.messageContainsIsRegex,
            "blockUnauthenticatedTeamsUsers": teams.blockUnauthenticatedTeamsUsers,
        ]
        filters = filters.filter { _, value in
            if let string = value as? String { return !string.isEmpty }
            if let array = value as? [String] { return !array.isEmpty }
            return true
        }
        return routineEventWire(source: "teams", event: "message", filters: filters)

    case .integration(let integration):
        var filters: [String: Any] = [:]
        if !integration.projectIds.isEmpty { filters["projectIds"] = integration.projectIds }
        if !integration.teamIds.isEmpty { filters["teamIds"] = integration.teamIds }
        if !integration.serviceIds.isEmpty { filters["serviceIds"] = integration.serviceIds }
        if !integration.statusIds.isEmpty { filters["statusIds"] = integration.statusIds }
        if !integration.cycleIds.isEmpty { filters["cycleIds"] = integration.cycleIds }
        return routineEventWire(
            source: integration.platform.rawValue,
            event: integration.eventCase,
            filters: filters
        )
    }
}

func routineTriggerWireValue(_ trigger: AutomationTrigger) -> [String: Any] {
    switch trigger {
    case .member(let member):
        return routineTriggerMemberWireValue(member)
    case .group(let listeners):
        return [
            "kind": "group",
            "listeners": listeners.map(routineTriggerMemberWireValue),
        ]
    }
}

private func routineWireStringArray(_ value: Any?) -> [String]? {
    if let values = value as? [String] { return values }
    guard let values = value as? [Any] else { return nil }
    var result: [String] = []
    for value in values {
        guard let value = value as? String else { return nil }
        result.append(value)
    }
    return result
}

private func routineTriggerMemberFromWireValue(_ value: Any) -> AutomationTriggerMember? {
    guard let row = value as? [String: Any],
          let kind = row["kind"] as? String
    else { return nil }

    if kind == "schedule" {
        guard let schedule = row["schedule"] as? String else { return nil }
        let form = RoutineTriggerForm.schedule(schedule)
        return routineTriggerFormToMember(form)
    }

    guard kind == "event",
          let source = row["source"] as? String,
          let event = row["event"] as? String
    else { return nil }

    let filters = row["filters"] as? [String: Any] ?? [:]
    switch source.lowercased() {
    case "slack":
        guard let channel = filters["channel"] as? String else { return nil }
        switch event {
        case "mention":
            return .slack(.init(channel: channel, match: .mention))
        case "message":
            if let keyword = filters["messageContains"] as? String {
                return .slack(.init(channel: channel, match: .keyword(keyword)))
            }
            return .slack(.init(channel: channel, match: .message))
        case "reaction":
            let emoji = routineWireStringArray(filters["emoji"]) ?? []
            return .slack(.init(
                channel: channel,
                match: .reaction(emoji: emoji, bySelf: filters["bySelf"] as? Bool)
            ))
        default:
            return nil
        }

    case "github":
        guard let repo = filters["repo"] as? String else { return nil }
        let events = routineWireStringArray(filters["events"])
            ?? (event == "*" ? [] : [event])
        guard !events.isEmpty else { return nil }
        return .github(.init(
            repo: repo,
            events: events,
            ciBranch: filters["ciBranch"] as? String,
            userAllowlist: routineWireStringArray(
                filters["actorAllowlist"] ?? filters["userAllowlist"]
            )
        ))

    case "teams":
        guard let tenantId = filters["tenantId"] as? String else { return nil }
        let teamIds = routineWireStringArray(filters["teamIds"]) ?? []
        return .microsoftTeams(.init(
            tenantId: tenantId,
            teamId: teamIds.first ?? "",
            teamIds: teamIds,
            channelIds: routineWireStringArray(filters["channelIds"]) ?? [],
            messageContains: filters["messageContains"] as? String ?? "",
            messageContainsIsRegex: filters["messageContainsIsRegex"] as? Bool ?? false,
            blockUnauthenticatedTeamsUsers: filters["blockUnauthenticatedTeamsUsers"] as? Bool ?? false
        ))

    case "linear":
        return .integration(.init(
            platform: .linear,
            eventCase: event,
            projectIds: routineWireStringArray(filters["projectIds"]) ?? [],
            teamIds: routineWireStringArray(filters["teamIds"]) ?? [],
            statusIds: routineWireStringArray(filters["statusIds"]) ?? [],
            cycleIds: routineWireStringArray(filters["cycleIds"]) ?? []
        ))

    case "sentry":
        return .integration(.init(
            platform: .sentry,
            eventCase: event,
            projectIds: routineWireStringArray(filters["projectIds"]) ?? []
        ))

    case "pagerduty":
        return .integration(.init(
            platform: .pagerduty,
            eventCase: event,
            serviceIds: routineWireStringArray(filters["serviceIds"]) ?? []
        ))

    default:
        return nil
    }
}

func routineTriggerFromWireValue(_ value: Any) -> AutomationTrigger? {
    guard let row = value as? [String: Any],
          let kind = row["kind"] as? String
    else { return nil }

    if kind == "group" {
        guard let rawListeners = row["listeners"] as? [Any],
              rawListeners.count >= 2,
              rawListeners.count <= TRIGGER_MAX_GROUP_LISTENERS
        else { return nil }
        let listeners = rawListeners.compactMap(routineTriggerMemberFromWireValue)
        guard listeners.count == rawListeners.count else { return nil }
        return .group(listeners)
    }

    guard let member = routineTriggerMemberFromWireValue(row) else { return nil }
    return .member(member)
}


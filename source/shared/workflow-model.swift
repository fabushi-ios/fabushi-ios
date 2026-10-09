import Foundation

let WORKFLOW_FILENAME = "SKILL.md"
let LEGACY_WORKFLOW_FILENAME = "workflow.md"
let AGENT_READABLE_SKILL_DIR_MODE = 0o755
let AGENT_READABLE_SKILL_FILE_MODE = 0o644
let WORKFLOW_MAX_NAME_LENGTH = 80
let WORKFLOW_MAX_DESCRIPTION_LENGTH = 1_536
let WORKFLOW_MAX_BODY_LENGTH = 100_000
let WORKFLOW_INJECTED_BODY_LIMIT = 8_000
let WORKFLOW_UI_LIMIT = 100
let WORKFLOW_MAX_PER_AGENT = 100

private func workflowPrefixByUTF16Units(_ value: String, maxLength: Int) -> String {
    guard maxLength > 0 else { return "" }
    var result = ""
    var units = 0
    for character in value {
        let part = String(character)
        let next = part.utf16.count
        guard units + next <= maxLength else { break }
        result.append(character)
        units += next
    }
    return result
}

indirect enum WorkflowValue: Equatable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case null
    case object([String: WorkflowValue])
    case array([WorkflowValue])

    var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    var objectValue: [String: WorkflowValue]? {
        if case .object(let value) = self { return value }
        return nil
    }

    var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }
}

struct WorkflowTrigger: Equatable, Sendable {
    let schedule: String
    let isEnabled: Bool
}

struct WorkflowSpec: Equatable, Sendable {
    let name: String
    let description: String
    let body: String
    let trigger: WorkflowTrigger?
    var sourceRef: String? = nil
}

struct ParsedWorkflow: Equatable, Sendable {
    let name: String
    let description: String
    let body: String
    let trigger: WorkflowTrigger?
    let sourceRef: String?
    let data: [String: WorkflowValue]
}

enum WorkflowSource: String, Equatable, Sendable {
    case managed, plugin, workflow, automation
}

struct WorkflowRecord: Equatable, Sendable {
    let id: String
    let name: String
    let description: String
    let body: String
    let trigger: WorkflowTrigger?
    let source: WorkflowSource
    let sourceRef: String?
    var pluginId: String? = nil
    var publishedByCurrentUser: Bool? = nil
    let isEnabledForAgent: Bool
    var disableModelInvocation: Bool? = nil
    var scheduleDescription: String? = nil
    let createdAt: Int64
    var lastRunAt: Int64? = nil
    var nextRunAt: Int64? = nil
    var helperScripts: [String] = []
    var runs: [WorkflowValue] = []
    let filePath: String
}

struct AutomationProjection: Equatable, Sendable {
    let id: String
    let name: String
    let prompt: String
    let trigger: CronTrigger
    let schedule: String
    let triggerDescription: String
    let isEnabled: Bool
    let createdAt: Int64
    let lastRunAt: Int64?
    let nextRunAt: Int64?
    let runs: [WorkflowValue]
    let filePath: String
}

func clampWorkflowName(_ value: String?) -> String {
    guard let value else { return "" }
    let collapsed = value.replacingOccurrences(of: #"[\r\n]+"#, with: " ", options: .regularExpression)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    return workflowPrefixByUTF16Units(collapsed, maxLength: WORKFLOW_MAX_NAME_LENGTH)
}

func clampWorkflowDescription(_ value: String?) -> String {
    guard let value else { return "" }
    let collapsed = value.replacingOccurrences(of: #"[\r\n]+"#, with: " ", options: .regularExpression)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    return workflowPrefixByUTF16Units(collapsed, maxLength: WORKFLOW_MAX_DESCRIPTION_LENGTH)
}

func clampWorkflowBody(_ value: String?) -> String {
    guard let value else { return "" }
    return workflowPrefixByUTF16Units(value.trimmingCharacters(in: .whitespacesAndNewlines), maxLength: WORKFLOW_MAX_BODY_LENGTH)
}

func slugifyWorkflowName(_ name: String) -> String {
    let folded = name.decomposedStringWithCompatibilityMapping.lowercased()
    let replaced = folded.replacingOccurrences(of: #"[^a-z0-9]+"#, with: "-", options: .regularExpression)
    let trimmed = replaced.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
    let limited = workflowPrefixByUTF16Units(trimmed, maxLength: 64).trimmingCharacters(in: CharacterSet(charactersIn: "-"))
    return limited.isEmpty ? "workflow" : limited
}

private func workflowScalar(_ raw: String) -> WorkflowValue {
    let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    if value == "true" { return .bool(true) }
    if value == "false" { return .bool(false) }
    if value == "null" || value == "~" { return .null }
    if let number = Double(value), value.range(of: #"^-?\d+(?:\.\d+)?$"#, options: .regularExpression) != nil {
        return .number(number)
    }
    if value.count >= 2, value.hasPrefix("\""), value.hasSuffix("\""),
       let data = value.data(using: .utf8),
       let decoded = try? JSONDecoder().decode(String.self, from: data) {
        return .string(decoded)
    }
    if value.count >= 2, value.hasPrefix("'"), value.hasSuffix("'") {
        return .string(String(value.dropFirst().dropLast()).replacingOccurrences(of: "''", with: "'"))
    }
    return .string(value)
}

private struct WorkflowYamlLine {
    let indent: Int
    let key: String
    let tail: String
}

private func workflowYamlLines(_ text: String) -> [WorkflowYamlLine] {
    text.split(separator: "\n", omittingEmptySubsequences: false).compactMap { sub in
        let raw = String(sub)
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty, !trimmed.hasPrefix("#") else { return nil }
        let indent = raw.prefix { $0 == " " }.count
        let rest = String(raw.dropFirst(indent))
        guard let colon = rest.firstIndex(of: ":"), colon != rest.startIndex else { return nil }
        let key = rest[..<colon].trimmingCharacters(in: .whitespaces)
        guard !key.isEmpty, !key.hasPrefix("#") else { return nil }
        let tail = rest[rest.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        return .init(indent: indent, key: key, tail: tail)
    }
}

private func parseWorkflowYamlObject(_ lines: [WorkflowYamlLine], index: inout Int, indent: Int) -> [String: WorkflowValue] {
    var result: [String: WorkflowValue] = [:]
    while index < lines.count {
        let line = lines[index]
        if line.indent < indent { break }
        if line.indent > indent {
            index += 1
            continue
        }
        index += 1
        if line.tail.isEmpty {
            if index < lines.count, lines[index].indent > line.indent {
                let childIndent = lines[index].indent
                result[line.key] = .object(parseWorkflowYamlObject(lines, index: &index, indent: childIndent))
            } else {
                result[line.key] = .object([:])
            }
        } else {
            result[line.key] = workflowScalar(line.tail)
        }
    }
    return result
}

private func parseWorkflowFrontmatter(_ text: String) -> [String: WorkflowValue] {
    let lines = workflowYamlLines(text)
    guard let first = lines.first else { return [:] }
    var index = 0
    return parseWorkflowYamlObject(lines, index: &index, indent: first.indent)
}

private func splitWorkflowMatter(_ raw: String) -> (data: [String: WorkflowValue], content: String) {
    guard raw.hasPrefix("---"), let firstNewline = raw.firstIndex(of: "\n") else { return ([:], raw) }
    let searchStart = raw.index(after: firstNewline)
    guard let close = raw.range(of: "\n---", range: searchStart..<raw.endIndex) else { return ([:], raw) }
    let matter = String(raw[searchStart..<close.lowerBound])
    var content = String(raw[close.upperBound...])
    if content.hasPrefix("\r\n") { content.removeFirst(2) }
    else if content.hasPrefix("\n") { content.removeFirst() }
    return (parseWorkflowFrontmatter(matter), content)
}

private func workflowSourceRef(_ data: [String: WorkflowValue]) -> String? {
    if let metadata = data["metadata"]?.objectValue,
       let source = metadata["source"]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines),
       !source.isEmpty { return source }
    if let source = data["source"]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines),
       !source.isEmpty { return source }
    return nil
}

private func workflowTrigger(_ data: [String: WorkflowValue]) -> WorkflowTrigger? {
    guard let trigger = data["trigger"]?.objectValue,
          let raw = trigger["schedule"]?.stringValue else { return nil }
    let schedule = normalizeSchedule(raw)
    guard !schedule.isEmpty else { return nil }
    return .init(schedule: schedule, isEnabled: trigger["enabled"]?.boolValue != false)
}

func parseWorkflowFile(_ raw: String) -> ParsedWorkflow? {
    let split = splitWorkflowMatter(raw)
    let body = clampWorkflowBody(split.content)
    guard !body.isEmpty || !split.data.isEmpty else { return nil }
    return .init(
        name: clampWorkflowName(split.data["name"]?.stringValue),
        description: clampWorkflowDescription(split.data["description"]?.stringValue),
        body: body,
        trigger: workflowTrigger(split.data),
        sourceRef: workflowSourceRef(split.data),
        data: split.data
    )
}

private func workflowYamlValue(_ value: WorkflowValue) -> String {
    switch value {
    case .string(let string):
        if let data = try? JSONEncoder().encode(string), let encoded = String(data: data, encoding: .utf8) { return encoded }
        return "\"\""
    case .number(let number):
        return number.rounded() == number ? String(Int(number)) : String(number)
    case .bool(let bool): return String(bool)
    case .null: return "null"
    case .array(let values):
        return "[" + values.map(workflowYamlValue).joined(separator: ", ") + "]"
    case .object:
        return "{}"
    }
}

private func emitWorkflowYaml(_ data: [String: WorkflowValue], indent: Int = 0) -> [String] {
    var lines: [String] = []
    for key in data.keys.sorted() {
        guard let value = data[key] else { continue }
        let prefix = String(repeating: " ", count: indent) + key + ":"
        if case .object(let object) = value {
            lines.append(prefix)
            lines.append(contentsOf: emitWorkflowYaml(object, indent: indent + 2))
        } else {
            lines.append(prefix + " " + workflowYamlValue(value))
        }
    }
    return lines
}

func serializeWorkflowFile(_ spec: WorkflowSpec, existingData: [String: WorkflowValue] = [:]) -> String {
    var data = existingData
    data["name"] = .string(spec.name)
    if spec.description.isEmpty { data.removeValue(forKey: "description") }
    else { data["description"] = .string(spec.description) }

    let legacySource = data["source"]?.stringValue
    data.removeValue(forKey: "source")
    var metadata = data["metadata"]?.objectValue ?? [:]
    let nextSource: String?
    if spec.sourceRef == nil {
        nextSource = metadata["source"]?.stringValue ?? legacySource
    } else {
        nextSource = spec.sourceRef
    }
    if let nextSource, !nextSource.isEmpty { metadata["source"] = .string(nextSource) }
    else { metadata.removeValue(forKey: "source") }
    if metadata.isEmpty { data.removeValue(forKey: "metadata") }
    else { data["metadata"] = .object(metadata) }

    if let trigger = spec.trigger {
        data["trigger"] = .object([
            "schedule": .string(trigger.schedule),
            "enabled": .bool(trigger.isEnabled),
        ])
    } else {
        data.removeValue(forKey: "trigger")
    }

    return "---\n" + emitWorkflowYaml(data).joined(separator: "\n") + "\n---\n" + spec.body.trimmingCharacters(in: .whitespacesAndNewlines) + "\n"
}

func workflowToAutomation(_ workflow: WorkflowRecord) -> AutomationProjection? {
    guard let trigger = workflow.trigger else { return nil }
    return .init(
        id: workflow.id,
        name: workflow.name,
        prompt: workflow.body,
        trigger: cronTrigger(trigger.schedule),
        schedule: trigger.schedule,
        triggerDescription: workflow.scheduleDescription ?? describeSchedule(trigger.schedule),
        isEnabled: trigger.isEnabled,
        createdAt: workflow.createdAt,
        lastRunAt: workflow.lastRunAt,
        nextRunAt: workflow.nextRunAt,
        runs: workflow.runs,
        filePath: workflow.filePath
    )
}

func automationToWorkflow(_ automation: AutomationProjection) -> WorkflowRecord {
    .init(
        id: automation.id,
        name: automation.name,
        description: "",
        body: automation.prompt,
        trigger: .init(schedule: automation.schedule, isEnabled: automation.isEnabled),
        source: .automation,
        sourceRef: nil,
        pluginId: nil,
        publishedByCurrentUser: false,
        isEnabledForAgent: true,
        scheduleDescription: automation.triggerDescription,
        createdAt: automation.createdAt,
        lastRunAt: automation.lastRunAt,
        nextRunAt: automation.nextRunAt,
        helperScripts: [],
        runs: automation.runs,
        filePath: automation.filePath
    )
}

func limitSurfacedWorkflows(_ workflows: [WorkflowRecord]) -> [WorkflowRecord] {
    let managed = workflows.filter { $0.source == .managed || $0.source == .plugin }
    let user = workflows.filter { $0.source != .managed && $0.source != .plugin }
    return managed + Array(user.prefix(WORKFLOW_UI_LIMIT))
}

struct AgentWorkflowSkill: Equatable, Sendable {
    let fullPath: String
    let description: String
}

func agentSkillsFromWorkflows(_ workflows: [WorkflowRecord]) -> [AgentWorkflowSkill] {
    limitSurfacedWorkflows(workflows).compactMap { workflow in
        guard workflow.trigger == nil,
              workflow.isEnabledForAgent,
              workflow.disableModelInvocation != true,
              !workflow.filePath.isEmpty else { return nil }
        return .init(fullPath: workflow.filePath, description: workflow.description)
    }
}

func deriveWorkflowNameFromMarkdown(_ body: String) -> String? {
    for raw in body.split(separator: "\n", omittingEmptySubsequences: false) {
        let line = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !line.isEmpty else { continue }
        let heading = line.replacingOccurrences(of: #"^#+\s+"#, with: "", options: .regularExpression)
        let text = heading.replacingOccurrences(of: #"[*_\x60#>]"#, with: "", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty { return workflowPrefixByUTF16Units(text, maxLength: WORKFLOW_MAX_NAME_LENGTH) }
    }
    return nil
}

func workflowSpecFromMarkdown(_ markdown: String, fallbackName: String? = nil) -> WorkflowSpec? {
    guard let parsed = parseWorkflowFile(markdown) else { return nil }
    let name = parsed.name.isEmpty
        ? clampWorkflowName(deriveWorkflowNameFromMarkdown(parsed.body) ?? fallbackName ?? "")
        : parsed.name
    guard !name.isEmpty, !parsed.body.isEmpty else { return nil }
    return .init(
        name: name,
        description: parsed.description,
        body: parsed.body,
        trigger: parsed.trigger,
        sourceRef: parsed.sourceRef
    )
}

func buildLiveSourcePointerBody(_ source: String) -> String {
    [
        "This workflow is a live reference to the skill at `\(source)`.",
        "Read that source now with your file or fetch tools and follow it as written. Do not assume its contents from this note; the source is the source of truth and may have changed since this workflow was created.",
    ].joined(separator: "\n")
}

func liveWorkflowSpecFromSource(name rawName: String, source rawSource: String, description rawDescription: String? = nil) -> WorkflowSpec {
    let source = rawSource.trimmingCharacters(in: .whitespacesAndNewlines)
    let name = clampWorkflowName(rawName)
    let explicit = clampWorkflowDescription(rawDescription ?? "")
    let generated = "Use when the \"\(name)\" skill applies; it is a live reference to \(source)."
    return .init(
        name: name,
        description: explicit.isEmpty ? clampWorkflowDescription(generated) : explicit,
        body: clampWorkflowBody(buildLiveSourcePointerBody(source)),
        trigger: nil,
        sourceRef: source
    )
}

func deriveWorkflowNameFromUrl(_ raw: String) -> String {
    let path = URL(string: raw)?.path ?? raw
    var last = path.split(separator: "/").last.map(String.init) ?? "Imported skill"
    last = last.replacingOccurrences(of: #"\.(md|markdown|mdc|txt)$"#, with: "", options: [.regularExpression, .caseInsensitive])
    last = last.replacingOccurrences(of: #"[-_]+"#, with: " ", options: .regularExpression)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    return clampWorkflowName(last.isEmpty ? "Imported skill" : last)
}

struct WorkflowReference: Equatable, Sendable {
    let id: String
    let teachQueueScope: String?
}

func collectWorkflowReferences(_ richText: String?) -> [WorkflowReference] {
    guard let richText, let data = richText.data(using: .utf8),
          let root = try? JSONSerialization.jsonObject(with: data) else { return [] }
    var result: [WorkflowReference] = []
    var seen = Set<String>()

    func visit(_ node: Any) {
        guard let object = node as? [String: Any] else { return }
        if object["type"] as? String == WORKFLOW_REFERENCE_NODE_TYPE,
           let attrs = object["attrs"] as? [String: Any],
           let id = attrs["id"] as? String,
           !id.isEmpty,
           seen.insert(id).inserted {
            result.append(.init(id: id, teachQueueScope: attrs["teachQueueScope"] as? String))
        }
        if let content = object["content"] as? [Any] {
            for child in content { visit(child) }
        }
    }

    visit(root)
    return result
}

private func workflowHandles(_ workflow: WorkflowRecord) -> [String] {
    var values = Set<String>()
    let name = workflow.name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    if !name.isEmpty {
        values.insert(name)
        values.insert(name.replacingOccurrences(of: #"\s+"#, with: "", options: .regularExpression))
    }
    let id = workflow.id.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    if !id.isEmpty { values.insert(id) }
    return Array(values)
}

func collectMentionedWorkflows(_ prompt: String, workflows: [WorkflowRecord]) -> [WorkflowRecord] {
    var found = Set<String>()
    let lower = prompt.lowercased()

    if let regex = try? NSRegularExpression(pattern: #"sand-workflow:([a-z0-9]+(?:-[a-z0-9]+)*)"#, options: [.caseInsensitive]) {
        let range = NSRange(lower.startIndex..<lower.endIndex, in: lower)
        for match in regex.matches(in: lower, range: range) {
            if let idRange = Range(match.range(at: 1), in: lower) { found.insert(String(lower[idRange])) }
        }
    }

    let candidates = workflows.flatMap { workflow in
        workflowHandles(workflow).map { (handle: $0, id: workflow.id.lowercased()) }
    }.sorted { $0.handle.utf16.count > $1.handle.utf16.count }
    var claimed: [Range<String.Index>] = []

    for candidate in candidates {
        let needle = "@" + candidate.handle
        var search = lower.startIndex..<lower.endIndex
        while let range = lower.range(of: needle, range: search) {
            let before = range.lowerBound == lower.startIndex ? nil : lower[lower.index(before: range.lowerBound)]
            let after = range.upperBound == lower.endIndex ? nil : lower[range.upperBound]
            func isWord(_ c: Character?) -> Bool {
                guard let c, c.unicodeScalars.count == 1, let scalar = c.unicodeScalars.first else { return false }
                return (scalar.value >= 97 && scalar.value <= 122) || (scalar.value >= 48 && scalar.value <= 57)
            }
            let overlaps = claimed.contains { $0.lowerBound < range.upperBound && range.lowerBound < $0.upperBound }
            if !isWord(before) && !isWord(after) && !overlaps {
                claimed.append(range)
                found.insert(candidate.id)
            }
            if range.upperBound == lower.endIndex { break }
            search = lower.index(after: range.lowerBound)..<lower.endIndex
        }
    }

    return workflows.filter { found.contains($0.id.lowercased()) }
}

func promptReferencesWorkflow(_ prompt: String, workflow: WorkflowRecord) -> Bool {
    !collectMentionedWorkflows(prompt, workflows: [workflow]).isEmpty
}

func workflowDir(_ filePath: String) -> String {
    guard let slash = filePath.lastIndex(where: { $0 == "/" || $0 == "\\" }) else { return filePath }
    return String(filePath[..<slash])
}

func renderWorkflowsSystemPrompt(_ location: String?) -> String {
    guard let location else { return "" }
    return "Workflows are a GLOBAL, shared library across all of the user's assistants. User-created skills live as files at \(location): one subfolder per workflow, each holding a SKILL.md. Prefer the update_state tool (target \"workflow\") to save, rewrite, and delete them. Reference workflows as [name](sand-workflow:<id>)."
}

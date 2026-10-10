import Foundation

enum PackagePromptAttribute: Equatable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)

    var rendered: String {
        switch self {
        case .string(let value): value
        case .number(let value): packagePromptNumber(value)
        case .bool(let value): value ? "true" : "false"
        }
    }
}

struct PackagePromptProps: Sendable {
    var children: [PackagePromptNode]?
    var name: String?
    var toolCallId: String?
    var tag: String?
    var title: String?
    var attributes: [String: PackagePromptAttribute]

    init(
        children: [PackagePromptNode]? = nil,
        name: String? = nil,
        toolCallId: String? = nil,
        tag: String? = nil,
        title: String? = nil,
        attributes: [String: PackagePromptAttribute] = [:]
    ) {
        self.children = children
        self.name = name
        self.toolCallId = toolCallId
        self.tag = tag
        self.title = title
        self.attributes = attributes
    }
}

struct PackagePromptElement: Sendable {
    let type: String
    let props: PackagePromptProps
    let children: [PackagePromptNode]?
}

indirect enum PackagePromptNode: Sendable {
    case element(PackagePromptElement)
    case array([PackagePromptNode])
    case text(String)
    case number(Double)
    case bool(Bool)
    case null
}

struct PackagePromptMessage: Equatable, Sendable {
    let role: String
    let content: String
    let name: String?
    let toolCallId: String?

    init(role: String, content: String, name: String? = nil, toolCallId: String? = nil) {
        self.role = role
        self.content = content
        self.name = name
        self.toolCallId = toolCallId
    }
}

typealias PackagePromptComponent = @Sendable (PackagePromptProps) -> PackagePromptNode

func packagePromptJSX(
    _ type: String,
    props: PackagePromptProps = .init(),
    children variadicChildren: [PackagePromptNode] = []
) -> PackagePromptNode {
    let source = props.children ?? variadicChildren
    let filtered = source.filter { node in
        switch node {
        case .null, .bool:
            false
        case .text(let value):
            !value.isEmpty
        default:
            true
        }
    }
    var normalized = props
    normalized.children = filtered.isEmpty ? nil : filtered
    return .element(.init(
        type: type,
        props: normalized,
        children: filtered.isEmpty ? nil : filtered
    ))
}

func packagePromptFragment(_ props: PackagePromptProps) -> PackagePromptNode {
    packagePromptJSX("Fragment", props: props)
}

func packagePromptSystem(_ props: PackagePromptProps) -> PackagePromptNode {
    packagePromptJSX("System", props: props)
}

func packagePromptUser(_ props: PackagePromptProps) -> PackagePromptNode {
    packagePromptJSX("User", props: props)
}

func packagePromptAssistant(_ props: PackagePromptProps) -> PackagePromptNode {
    packagePromptJSX("Assistant", props: props)
}

func packagePromptTool(_ props: PackagePromptProps) -> PackagePromptNode {
    packagePromptJSX("Tool", props: props)
}

final class PackagePromptRenderer {
    private let components: [String: PackagePromptComponent]

    init(components: [String: PackagePromptComponent] = [:]) {
        self.components = components
    }

    func renderToMessages(_ node: PackagePromptNode) throws -> [PackagePromptMessage] {
        switch node {
        case .null, .bool:
            return []
        case .text(let value):
            return [.init(role: "user", content: value)]
        case .number(let value):
            return [.init(role: "user", content: packagePromptNumber(value))]
        case .array(let nodes):
            return try nodes.flatMap(renderToMessages)
        case .element(let element):
            return try renderElement(element)
        }
    }

    func renderContent(_ node: PackagePromptNode?) throws -> String {
        guard let node else { return "" }
        switch node {
        case .null, .bool:
            return ""
        case .text(let value):
            return value
        case .number(let value):
            return packagePromptNumber(value)
        case .array(let nodes):
            return try renderParagraphAwareContent(nodes)
        case .element(let element):
            return try renderContentElement(element)
        }
    }

    private func renderElement(_ element: PackagePromptElement) throws -> [PackagePromptMessage] {
        let children = element.props.children.map(PackagePromptNode.array)
        switch element.type {
        case "System":
            return [.init(
                role: "system",
                content: try renderContent(children),
                name: element.props.name
            )]
        case "User":
            return [.init(
                role: "user",
                content: try renderContent(children),
                name: element.props.name
            )]
        case "Assistant":
            return [.init(
                role: "assistant",
                content: try renderContent(children),
                name: element.props.name
            )]
        case "Tool":
            return [.init(
                role: "tool",
                content: try renderContent(children),
                name: element.props.name,
                toolCallId: element.props.toolCallId
            )]
        case "Fragment":
            return try renderToMessages(children ?? .array([]))
        case "p", "section", "ul", "ol", "li",
             "h1", "h2", "h3", "h4", "h5", "h6",
             "x", "pre", "br":
            return []
        default:
            if let component = components[element.type] {
                return try renderToMessages(component(element.props))
            }
            throw PackagePromptRenderError.unknownComponent(element.type)
        }
    }

    private func renderContentElement(_ element: PackagePromptElement) throws -> String {
        if let component = components[element.type] {
            return try renderContent(component(element.props))
        }
        let children = element.props.children.map(PackagePromptNode.array)
        switch element.type {
        case "p", "Fragment":
            return try renderContent(children)
        case "h1", "h2", "h3", "h4", "h5", "h6":
            let level = Int(element.type.dropFirst()) ?? 1
            let content = try renderContent(children).trimmingCharacters(in: .whitespacesAndNewlines)
            return content.isEmpty ? "" : String(repeating: "#", count: level) + " " + content
        case "x":
            return try renderX(element.props)
        case "section":
            return try renderSection(element.props)
        case "br":
            return "\n"
        case "pre":
            return try renderPre(children)
        case "ul":
            return try renderList(children, ordered: false, baseIndent: 0)
        case "ol":
            return try renderList(children, ordered: true, baseIndent: 0)
        case "li":
            return "- " + (try renderContent(children))
        default:
            return try renderToMessages(.element(element)).map(\.content).joined(separator: "\n")
        }
    }

    private func renderParagraphAwareContent(_ input: [PackagePromptNode]) throws -> String {
        let nodes = flatten(input)
        var paragraphs: [(content: String, isSection: Bool)] = []
        var current: [String] = []

        func flush() {
            guard !current.isEmpty else { return }
            let joined = current.joined()
            if !joined.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                paragraphs.append((packagePromptNormalizeWhitespace(joined), false))
            } else if joined.contains("\n") {
                paragraphs.append((joined, false))
            }
            current.removeAll()
        }

        for node in nodes {
            if case .element(let element) = node,
               ["p", "section", "ul", "ol", "x", "pre"].contains(element.type)
                || element.type.range(of: #"^h[1-6]$"#, options: .regularExpression) != nil
            {
                flush()
                let content = try renderContentElement(element)
                let final = element.type == "pre"
                    ? content
                    : content.trimmingCharacters(in: .whitespacesAndNewlines)
                if !final.isEmpty || element.type == "pre" {
                    paragraphs.append((final, element.type == "section"))
                }
            } else {
                let rendered = try renderContent(node)
                if !rendered.isEmpty { current.append(rendered) }
            }
        }
        flush()

        guard let first = paragraphs.first else { return "" }
        var result = first.content
        for index in 1..<paragraphs.count {
            let previous = paragraphs[index - 1].content
            let current = paragraphs[index].content
            if current.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                result += current
            } else if !previous.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                      previous.hasSuffix("\n") {
                result += current
            } else {
                result += "\n\n" + current
            }
        }
        return result.trimmingCharacters(in: CharacterSet.newlines)
    }

    private func flatten(_ nodes: [PackagePromptNode]) -> [PackagePromptNode] {
        var result: [PackagePromptNode] = []
        for node in nodes {
            switch node {
            case .null, .bool:
                continue
            case .array(let nested):
                result.append(contentsOf: flatten(nested))
            case .element(let element) where element.type == "Fragment":
                result.append(contentsOf: flatten(element.props.children ?? []))
            default:
                result.append(node)
            }
        }
        return result
    }

    private func renderX(_ props: PackagePromptProps) throws -> String {
        let tag = props.tag ?? "x"
        let content = try renderContent(props.children.map(PackagePromptNode.array))
        let attrs = packagePromptRenderAttributes(props.attributes)
        let prefix = attrs.isEmpty ? "" : " " + attrs
        return content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? "<\(tag)\(prefix) />"
            : "<\(tag)\(prefix)>\(content)</\(tag)>"
    }

    private func renderSection(_ props: PackagePromptProps) throws -> String {
        let rawTitle = props.title ?? "undefined"
        let tag = rawTitle
            .lowercased()
            .replacingOccurrences(of: #"\s+"#, with: "-", options: .regularExpression)
        let content = try renderContent(props.children.map(PackagePromptNode.array))
        let attrs = packagePromptRenderAttributes(props.attributes)
        let prefix = attrs.isEmpty ? "" : " " + attrs
        return content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? "<\(tag)\(prefix) />"
            : "<\(tag)\(prefix)>\n\(content)\n</\(tag)>"
    }

    private func renderList(
        _ node: PackagePromptNode?,
        ordered: Bool,
        baseIndent: Int
    ) throws -> String {
        guard let node else { return "" }
        let source: [PackagePromptNode]
        switch node {
        case .array(let values): source = flattenList(values)
        default: source = [node]
        }

        var lines: [String] = []
        var itemNumber = 1
        let markerWidth = ordered ? 3 : 2
        for child in source {
            guard case .element(let element) = child, element.type == "li" else {
                let rendered = try renderContent(child).trimmingCharacters(in: .whitespacesAndNewlines)
                if !rendered.isEmpty {
                    lines.append(String(repeating: " ", count: baseIndent) + rendered)
                }
                continue
            }

            let parts = try extractListItem(
                element.props.children ?? [],
                nestedIndent: baseIndent + markerWidth
            )
            let marker = ordered ? "\(itemNumber). " : "- "
            if !parts.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                lines.append(
                    String(repeating: " ", count: baseIndent)
                    + marker
                    + parts.text.trimmingCharacters(in: .whitespacesAndNewlines)
                )
                lines.append(contentsOf: parts.nested)
            } else if ordered, let nested = parts.nested.first {
                lines.append(String(repeating: " ", count: baseIndent) + marker + nested.trimmingCharacters(in: .whitespaces))
                lines.append(contentsOf: parts.nested.dropFirst())
            } else {
                lines.append(contentsOf: parts.nested)
            }
            if ordered { itemNumber += 1 }
        }
        return lines.joined(separator: "\n")
    }

    private func flattenList(_ nodes: [PackagePromptNode]) -> [PackagePromptNode] {
        nodes.flatMap { node -> [PackagePromptNode] in
            if case .array(let nested) = node { return flattenList(nested) }
            return [node]
        }
    }

    private func extractListItem(
        _ children: [PackagePromptNode],
        nestedIndent: Int
    ) throws -> (text: String, nested: [String]) {
        var text: [String] = []
        var nested: [String] = []
        for child in flattenList(children) {
            if case .element(let element) = child, element.type == "ul" || element.type == "ol" {
                let rendered = try renderList(
                    element.props.children.map(PackagePromptNode.array),
                    ordered: element.type == "ol",
                    baseIndent: nestedIndent
                )
                if !rendered.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    nested.append(rendered)
                }
            } else {
                let rendered = try renderContent(child)
                if !rendered.isEmpty { text.append(rendered) }
            }
        }
        return (text.joined(), nested)
    }

    private func renderPre(_ node: PackagePromptNode?) throws -> String {
        guard let node else { return "" }
        switch node {
        case .null, .bool:
            return ""
        case .text(let value):
            return value
        case .number(let value):
            return packagePromptNumber(value)
        case .array(let values):
            return try values.map { try renderPre($0) }.joined()
        case .element(let element):
            if element.type == "br" { return "\n" }
            return try renderPre(element.props.children.map(PackagePromptNode.array))
        }
    }
}

enum PackagePromptRenderError: Error, Equatable {
    case unknownComponent(String)
}

private func packagePromptRenderAttributes(_ attributes: [String: PackagePromptAttribute]) -> String {
    attributes.keys.sorted().compactMap { key in
        guard let value = attributes[key] else { return nil }
        switch value {
        case .bool(false):
            return nil
        case .bool(true):
            return key
        default:
            let escaped = value.rendered.replacingOccurrences(of: "\"", with: "&quot;")
            return "\(key)=\"\(escaped)\""
        }
    }.joined(separator: " ")
}

private func packagePromptNormalizeWhitespace(_ input: String) -> String {
    let trailing = input.range(of: #"\n+$"#, options: .regularExpression)
        .map { String(input[$0]) } ?? ""
    let body = input
        .replacingOccurrences(of: #" +"#, with: " ", options: .regularExpression)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    return body + trailing
}

private func packagePromptNumber(_ value: Double) -> String {
    if value.isFinite, value.rounded() == value,
       value <= Double(Int64.max), value >= Double(Int64.min) {
        return String(Int64(value))
    }
    return String(value)
}

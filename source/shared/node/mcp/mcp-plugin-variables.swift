import Foundation

struct PluginVariableField: Equatable, Sendable {
    let key: String
    let label: String
    let placeholder: String
    let isRequired: Bool
    let isSecret: Bool
    var defaultValue: String? = nil
    var hint: String? = nil
}

private let MCP_SECRET_NAME_FRAGMENTS = ["TOKEN","SECRET","KEY","PASSWORD","CREDENTIAL"]
private let MCP_VARIABLE_ACRONYMS: Set<String> = ["url","uri","api","id","ssl","tls","http","https","db","aws","gcp"]

func humanizeVariableName(_ name: String) -> String {
    name.split(separator: "_").map { raw in
        let word = String(raw)
        if MCP_VARIABLE_ACRONYMS.contains(word.lowercased()) { return word.uppercased() }
        guard let first = word.first else { return "" }
        return first.uppercased() + word.dropFirst().lowercased()
    }.filter { !$0.isEmpty }.joined(separator: " ")
}

private func mcpDictionary(_ value: Any?) -> [String: Any]? {
    value as? [String: Any]
}

func pluginVariablesSchemaToFields(_ schema: Any) -> [PluginVariableField] {
    guard let root = mcpDictionary(schema),
          let properties = mcpDictionary(root["properties"]) else { return [] }

    let required = Set((root["required"] as? [Any] ?? []).compactMap { $0 as? String })
    return properties.map { key, raw -> PluginVariableField in
        let property = mcpDictionary(raw) ?? [:]
        let title = property["title"] as? String
        let description = property["description"] as? String
        let defaultValue = property["default"] as? String
        let isSecret = property["format"] as? String == "password"
            || property["writeOnly"] as? Bool == true
            || MCP_SECRET_NAME_FRAGMENTS.contains(where: { key.uppercased().contains($0) })
        return .init(
            key: key,
            label: title ?? humanizeVariableName(key),
            placeholder: key,
            isRequired: required.contains(key),
            isSecret: isSecret,
            defaultValue: defaultValue,
            hint: description
        )
    }
}

func findMissingRequiredCatalogFields(
    _ fields: [PluginVariableField],
    values: [String: String?]
) -> [PluginVariableField] {
    fields.filter { field in
        guard field.isRequired else { return false }
        let supplied = values[field.key] ?? nil
        let value = supplied?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let fallback = field.defaultValue?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return value.isEmpty && fallback.isEmpty
    }
}

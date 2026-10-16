import Foundation

struct SandModelCatalogValue: Codable, Equatable, Sendable {
    let value: String
    let displayName: String?

    init(value: String, displayName: String? = nil) {
        self.value = value
        self.displayName = displayName
    }
}

struct SandModelCatalogParameter: Codable, Equatable, Sendable {
    enum ParameterType: String, Codable, Equatable, Sendable {
        case boolean
        case enumeration = "enum"
    }

    let id: String
    let name: String?
    let type: ParameterType
    let values: [SandModelCatalogValue]
}

struct SandModelCatalogParameterValue: Codable, Equatable, Hashable, Sendable {
    let id: String
    let value: String
}

struct SandModelCatalogEntry: Codable, Equatable, Sendable {
    let id: String
    let displayName: String?
    let aliases: [String]
    let params: [SandModelCatalogParameter]
    let variants: [[SandModelCatalogParameterValue]]
}

struct AvailableModelWire: Equatable, Sendable {
    struct ParameterDefinition: Equatable, Sendable {
        struct ParameterType: Equatable, Sendable {
            struct Values: Equatable, Sendable {
                let values: [SandModelCatalogValue]
            }

            let booleanParameter: Values?
            let enumParameter: Values?
        }

        let id: String
        let name: String
        let parameterType: ParameterType?
    }

    struct Variant: Equatable, Sendable {
        let parameterValues: [SandModelCatalogParameterValue]
    }

    let name: String
    let clientDisplayName: String?
    let idAliases: [String]
    let parameterDefinitions: [ParameterDefinition]
    let variants: [Variant]
}

func findCatalogEntry(
    _ catalog: [SandModelCatalogEntry],
    modelId: String
) -> SandModelCatalogEntry? {
    let needle = modelId.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard !needle.isEmpty else { return nil }
    return catalog.first { entry in
        entry.id.lowercased() == needle
            || entry.aliases.contains { $0.lowercased() == needle }
    }
}

private func variantMatchesRequested(
    _ variant: [SandModelCatalogParameterValue],
    requested: [(String, String)]
) -> Bool {
    let byId = Dictionary(uniqueKeysWithValues: variant.map { ($0.id, $0.value) })
    return requested.allSatisfy { id, value in byId[id] == value }
}

func validateParamCombination(
    _ entry: SandModelCatalogEntry,
    params: [String: String]
) -> String? {
    guard !entry.variants.isEmpty, !params.isEmpty else { return nil }
    let requested = params.sorted { $0.key < $1.key }.map { ($0.key, $0.value) }
    if entry.variants.contains(where: { variantMatchesRequested($0, requested: requested) }) {
        return nil
    }

    let conflicting = requested.compactMap { dropId, _ -> String? in
        let rest = requested.filter { id, _ in id != dropId }
        return entry.variants.contains(where: { variantMatchesRequested($0, requested: rest) })
            ? dropId
            : nil
    }
    let requestedString = requested.map { "\($0.0)=\($0.1)" }.joined(separator: ", ")
    let hint = conflicting.count > 1
        ? " These parameters can't be combined on \(entry.id): \(conflicting.joined(separator: ", "))."
        : ""
    return "{\(requestedString)} is not a supported parameter combination for \(entry.id)." +
        hint +
        " Change one of the conflicting parameters and retry (for example, on some models a 1M context can't be combined with the fast tier)."
}

func validateModelParams(
    _ entry: SandModelCatalogEntry,
    params: [String: String]
) -> [String] {
    var errors: [String] = []
    for (id, value) in params.sorted(by: { $0.key < $1.key }) {
        guard let definition = entry.params.first(where: { $0.id == id }) else {
            let allowedIds = entry.params.map(\.id).joined(separator: ", ")
            errors.append(
                "'\(id)' is not a parameter of \(entry.id)." +
                    (allowedIds.isEmpty
                        ? " This model takes no parameters."
                        : " Allowed parameters: \(allowedIds).")
            )
            continue
        }
        guard definition.values.contains(where: { $0.value == value }) else {
            let allowed = definition.values.map(\.value).joined(separator: ", ")
            errors.append(
                "'\(value)' is not a valid value for '\(id)' on \(entry.id). Allowed: \(allowed)."
            )
            continue
        }
    }

    if errors.isEmpty, let combinationError = validateParamCombination(entry, params: params) {
        errors.append(combinationError)
    }
    return errors
}

func describeParamIncompatibilities(
    _ entry: SandModelCatalogEntry
) -> [String] {
    guard !entry.variants.isEmpty else { return [] }
    var present: [SandModelCatalogParameterValue] = []
    var seen = Set<SandModelCatalogParameterValue>()
    for variant in entry.variants {
        for parameter in variant where seen.insert(parameter).inserted {
            present.append(parameter)
        }
    }

    func coOccurs(
        _ a: SandModelCatalogParameterValue,
        _ b: SandModelCatalogParameterValue
    ) -> Bool {
        entry.variants.contains { variant in
            let byId = Dictionary(uniqueKeysWithValues: variant.map { ($0.id, $0.value) })
            return byId[a.id] == a.value && byId[b.id] == b.value
        }
    }

    var notes: [String] = []
    for index in present.indices {
        let a = present[index]
        for b in present.dropFirst(index + 1)
            where a.id != b.id && !coOccurs(a, b) {
            notes.append("\(a.id)=\(a.value) cannot be combined with \(b.id)=\(b.value)")
        }
    }
    return notes
}

func toSandModelCatalogEntry(
    _ model: AvailableModelWire
) -> SandModelCatalogEntry {
    var params: [SandModelCatalogParameter] = []
    for definition in model.parameterDefinitions {
        let booleanDefinition = definition.parameterType?.booleanParameter
        let enumDefinition = definition.parameterType?.enumParameter
        let rawValues = booleanDefinition?.values ?? enumDefinition?.values ?? []
        let values = rawValues.map { value in
            SandModelCatalogValue(
                value: value.value,
                displayName: value.displayName?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                    .nilIfEmpty
            )
        }
        guard !values.isEmpty else { continue }
        params.append(.init(
            id: definition.id,
            name: definition.name
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .nilIfEmpty,
            type: booleanDefinition != nil ? .boolean : .enumeration,
            values: values
        ))
    }

    return SandModelCatalogEntry(
        id: model.name,
        displayName: model.clientDisplayName?
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .nilIfEmpty,
        aliases: model.idAliases,
        params: params,
        variants: model.variants.map { variant in
            variant.parameterValues.map { .init(id: $0.id, value: $0.value) }
        }
    )
}

func mapAvailableModels(
    _ models: [AvailableModelWire]
) -> [SandModelCatalogEntry] {
    models
        .filter { !$0.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        .map(toSandModelCatalogEntry)
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}

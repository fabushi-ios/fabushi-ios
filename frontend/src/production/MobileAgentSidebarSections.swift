import Foundation

struct MobileAgentSidebarSection: Codable, Identifiable, Equatable {
    let id: String
    var name: String
    var agentIds: [String]
}

enum MobileAgentSidebarSections {
    static func canAssign(isPinned: Bool, isHidden: Bool) -> Bool {
        !isPinned && !isHidden
    }

    static func normalized(_ sections: [MobileAgentSidebarSection]) -> [MobileAgentSidebarSection] {
        var seenSections = Set<String>()
        var claimedAgents = Set<String>()
        var result: [MobileAgentSidebarSection] = []
        for section in sections {
            let id = section.id.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !id.isEmpty, seenSections.insert(id).inserted else { continue }
            var ids: [String] = []
            for agentId in section.agentIds where !agentId.isEmpty {
                if claimedAgents.insert(agentId).inserted {
                    ids.append(agentId)
                }
            }
            result.append(.init(id: id, name: section.name, agentIds: ids))
        }
        return result
    }

    static func assigning(
        agentId: String,
        to sectionId: String?,
        in sections: [MobileAgentSidebarSection]
    ) -> [MobileAgentSidebarSection] {
        normalized(sections).map { section in
            var next = section
            next.agentIds.removeAll { $0 == agentId }
            if section.id == sectionId {
                next.agentIds.append(agentId)
            }
            return next
        }
    }

    static func creating(
        name: String,
        with agentId: String,
        in sections: [MobileAgentSidebarSection],
        id: String = UUID().uuidString.lowercased()
    ) -> [MobileAgentSidebarSection]? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        let unassigned = assigning(agentId: agentId, to: nil, in: sections)
        return [
            MobileAgentSidebarSection(id: "section-\(id)", name: trimmed, agentIds: [agentId])
        ] + unassigned
    }

    static func load(
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) -> [MobileAgentSidebarSection] {
        guard let data = defaults.data(forKey: storageKey(accountScopeKey)),
              let decoded = try? JSONDecoder().decode([MobileAgentSidebarSection].self, from: data)
        else { return [] }
        return normalized(decoded)
    }

    static func persist(
        _ sections: [MobileAgentSidebarSection],
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) {
        let next = normalized(sections)
        guard let data = try? JSONEncoder().encode(next) else { return }
        defaults.set(data, forKey: storageKey(accountScopeKey))
    }

    private static func storageKey(_ accountScopeKey: String) -> String {
        let encoded = Data(accountScopeKey.utf8).base64EncodedString()
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "=", with: "")
        return "fabushi.mobile.sidebar-sections.\(encoded)"
    }
}

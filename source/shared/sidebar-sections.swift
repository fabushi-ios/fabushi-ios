import Foundation

struct SidebarSection: Equatable, Sendable {
    let id: String
    let name: String
    let agentIDs: [String]
    var isCollapsed: Bool?

    init(id: String, name: String, agentIDs: [String], isCollapsed: Bool? = nil) {
        self.id = id
        self.name = name
        self.agentIDs = agentIDs
        self.isCollapsed = isCollapsed
    }
}

enum SidebarSections {
    static let agentsSectionID = "__agents__"
    static let agentsSectionName = "Unassigned"

    static func normalize(_ sections: [SidebarSection]) -> [SidebarSection] {
        var seenSectionIDs: Set<String> = []
        var claimedAgentIDs: Set<String> = []
        var normalized: [SidebarSection] = []

        for section in sections {
            let id = section.id.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !id.isEmpty, !seenSectionIDs.contains(id) else { continue }
            seenSectionIDs.insert(id)
            guard id != agentsSectionID else { continue }

            var agents: [String] = []
            for agentID in section.agentIDs {
                guard !agentID.isEmpty, !claimedAgentIDs.contains(agentID) else { continue }
                claimedAgentIDs.insert(agentID)
                agents.append(agentID)
            }
            normalized.append(.init(id: id, name: section.name, agentIDs: agents))
        }

        guard !normalized.isEmpty else { return [] }
        normalized.append(.init(id: agentsSectionID, name: agentsSectionName, agentIDs: []))
        return normalized
    }

    static func parse(_ value: [Any]) -> [SidebarSection] {
        let records = value.compactMap { entry -> SidebarSection? in
            guard let record = entry as? [String: Any],
                  let id = record["id"] as? String else { return nil }
            let name = record["name"] as? String ?? ""
            let agentIDs = (record["agentIds"] as? [Any] ?? []).compactMap { $0 as? String }
            return SidebarSection(id: id, name: name, agentIDs: agentIDs)
        }
        return normalize(records)
    }

    static func withFolds(_ sections: [SidebarSection], collapsedSectionIDs: [String]) -> [SidebarSection] {
        let collapsed = Set(collapsedSectionIDs)
        return sections.map {
            .init(id: $0.id, name: $0.name, agentIDs: $0.agentIDs, isCollapsed: collapsed.contains($0.id))
        }
    }

    static func carryFolds(sections: [SidebarSection], stored: [SidebarSection]? = nil) -> [SidebarSection] {
        var foldByID: [String: Bool] = [:]
        for section in (stored ?? []) + sections {
            if let folded = section.isCollapsed {
                foldByID[section.id.trimmingCharacters(in: .whitespacesAndNewlines)] = folded
            }
        }
        return normalize(sections).map {
            .init(id: $0.id, name: $0.name, agentIDs: $0.agentIDs, isCollapsed: foldByID[$0.id] ?? false)
        }
    }
}

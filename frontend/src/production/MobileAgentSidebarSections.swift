import Foundation

struct MobileAgentSidebarSection: Codable, Identifiable, Equatable {
    let id: String
    var name: String
    var agentIds: [String]
    var isCollapsed: Bool = false
}

enum MobileAgentSidebarSections {
    static let pinnedSectionID = "__pinned__"
    static let unassignedSectionID = "__unassigned__"
    private static let reservedSectionIDs: Set<String> = [
        "__agents__",
        pinnedSectionID,
        unassignedSectionID,
    ]

    static func canAssign(isPinned: Bool, isHidden: Bool) -> Bool {
        !isPinned && !isHidden
    }

    static func normalized(_ sections: [MobileAgentSidebarSection]) -> [MobileAgentSidebarSection] {
        var seenSections = Set<String>()
        var claimedAgents = Set<String>()
        var result: [MobileAgentSidebarSection] = []
        for section in sections {
            let id = section.id.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !id.isEmpty,
                  !reservedSectionIDs.contains(id),
                  seenSections.insert(id).inserted
            else { continue }
            var ids: [String] = []
            for agentId in section.agentIds where !agentId.isEmpty {
                if claimedAgents.insert(agentId).inserted {
                    ids.append(agentId)
                }
            }
            result.append(.init(
                id: id,
                name: section.name,
                agentIds: ids,
                isCollapsed: section.isCollapsed
            ))
        }
        return result
    }

    static func projected(
        agentIds: [String],
        pinnedAgentIds: Set<String>,
        sections: [MobileAgentSidebarSection],
        searching: Bool
    ) -> [MobileAgentSidebarSection] {
        var seen = Set<String>()
        let orderedAgentIds = agentIds.filter { !$0.isEmpty && seen.insert($0).inserted }
        let pinned = orderedAgentIds.filter { pinnedAgentIds.contains($0) }
        var result: [MobileAgentSidebarSection] = []
        if !pinned.isEmpty {
            result.append(.init(
                id: pinnedSectionID,
                name: "置顶",
                agentIds: pinned,
                isCollapsed: false
            ))
        }

        var assigned = Set<String>()
        for section in normalized(sections) {
            let ids = orderedAgentIds.filter {
                !pinnedAgentIds.contains($0) && section.agentIds.contains($0)
            }
            guard !ids.isEmpty else { continue }
            assigned.formUnion(ids)
            result.append(.init(
                id: section.id,
                name: section.name,
                agentIds: ids,
                isCollapsed: searching ? false : section.isCollapsed
            ))
        }

        let unassigned = orderedAgentIds.filter {
            !pinnedAgentIds.contains($0) && !assigned.contains($0)
        }
        if !unassigned.isEmpty {
            result.append(.init(
                id: unassignedSectionID,
                name: "未分组",
                agentIds: unassigned,
                isCollapsed: false
            ))
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
            MobileAgentSidebarSection(
                id: "section-\(id)",
                name: trimmed,
                agentIds: [agentId],
                isCollapsed: false
            )
        ] + unassigned
    }

    static func renamed(
        _ sections: [MobileAgentSidebarSection],
        sectionId: String,
        name: String
    ) -> [MobileAgentSidebarSection]? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !reservedSectionIDs.contains(sectionId) else { return nil }
        return normalized(sections).map { section in
            var next = section
            if next.id == sectionId { next.name = trimmed }
            return next
        }
    }

    static func removing(
        _ sections: [MobileAgentSidebarSection],
        sectionId: String
    ) -> [MobileAgentSidebarSection] {
        guard !reservedSectionIDs.contains(sectionId) else { return normalized(sections) }
        return normalized(sections).filter { $0.id != sectionId }
    }

    static func togglingCollapsed(
        _ sections: [MobileAgentSidebarSection],
        sectionId: String
    ) -> [MobileAgentSidebarSection] {
        guard !reservedSectionIDs.contains(sectionId) else { return normalized(sections) }
        let rows = normalized(sections)
        guard rows.contains(where: { $0.id == sectionId }) else { return rows }
        return rows.map { section in
            var next = section
            if next.id == sectionId {
                next.isCollapsed.toggle()
            }
            return next
        }
    }

    static func moving(
        _ sections: [MobileAgentSidebarSection],
        sectionId: String,
        offset: Int
    ) -> [MobileAgentSidebarSection] {
        let rows = normalized(sections)
        guard offset != 0,
              let source = rows.firstIndex(where: { $0.id == sectionId })
        else { return rows }
        let target = source + offset
        guard rows.indices.contains(target) else { return rows }
        var next = rows
        next.swapAt(source, target)
        return next
    }

    static func canonical(from value: Any) -> [MobileAgentSidebarSection]? {
        if value is NSNull { return [] }
        guard let rows = value as? [[String: Any]] else { return nil }
        var parsed: [MobileAgentSidebarSection] = []
        for row in rows {
            guard let id = row["id"] as? String,
                  let name = row["name"] as? String,
                  let agentIds = row["agentIds"] as? [String]
            else { return nil }
            parsed.append(.init(
                id: id,
                name: name,
                agentIds: agentIds,
                isCollapsed: row["isCollapsed"] as? Bool ?? false
            ))
        }
        return normalized(parsed)
    }

    static func foundationValue(
        _ sections: [MobileAgentSidebarSection]
    ) -> [[String: Any]] {
        normalized(sections).map {
            [
                "id": $0.id,
                "name": $0.name,
                "agentIds": $0.agentIds,
                "isCollapsed": $0.isCollapsed,
            ]
        }
    }

    static func loadFallback(
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) -> [MobileAgentSidebarSection] {
        guard let data = defaults.data(forKey: storageKey(accountScopeKey)),
              let decoded = try? JSONDecoder().decode([MobileAgentSidebarSection].self, from: data)
        else { return [] }
        return normalized(decoded)
    }

    static func persistFallback(
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

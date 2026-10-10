import Foundation

struct AccountDisplay<TServer> {
    let servers: [TServer]
    var unresolvedServerIds: [String]? = nil
}

func mergeUnresolvedAccountServers<TServer>(
    display: AccountDisplay<TServer>,
    cached: AccountDisplay<TServer>?,
    id: (TServer) -> String
) -> AccountDisplay<TServer> {
    guard let cached,
          let unresolvedIds = display.unresolvedServerIds,
          !unresolvedIds.isEmpty else { return display }

    let unresolved = Set(unresolvedIds)
    let fresh = Dictionary(uniqueKeysWithValues: display.servers.map { (id($0), $0) })
    let cachedIds = Set(cached.servers.map(id))
    var merged: [TServer] = []

    for server in cached.servers {
        let serverId = id(server)
        if let replacement = fresh[serverId] {
            merged.append(replacement)
        } else if unresolved.contains(serverId) {
            merged.append(server)
        }
    }
    merged.append(contentsOf: display.servers.filter { !cachedIds.contains(id($0)) })
    return .init(servers: merged, unresolvedServerIds: display.unresolvedServerIds)
}

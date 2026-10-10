import Foundation

func isPathWithin(
    parent: String,
    child: String,
    isInclusive: Bool = false
) -> Bool {
    let parentURL = URL(fileURLWithPath: parent).standardizedFileURL
    let childURL = URL(fileURLWithPath: child).standardizedFileURL
    let parentComponents = parentURL.pathComponents
    let childComponents = childURL.pathComponents
    guard childComponents.count >= parentComponents.count,
          Array(childComponents.prefix(parentComponents.count)) == parentComponents else { return false }
    return childComponents.count > parentComponents.count || isInclusive
}

func filePathFromFileUrl(_ rawUrl: String) -> String? {
    guard let url = URL(string: rawUrl), url.isFileURL else { return nil }
    return url.path
}

func posixPathFromFileUrl(_ rawUrl: String) -> String? {
    guard let components = URLComponents(string: rawUrl),
          components.scheme?.lowercased() == "file" else { return nil }
    return components.percentEncodedPath.removingPercentEncoding
}

func realpathNearestExisting(_ path: String) throws -> String {
    let fm = FileManager.default
    var current = URL(fileURLWithPath: path).standardizedFileURL
    var missing: [String] = []

    while !fm.fileExists(atPath: current.path) {
        let parent = current.deletingLastPathComponent()
        if parent.path == current.path { return path }
        missing.append(current.lastPathComponent)
        current = parent
    }

    var resolved = current.resolvingSymlinksInPath()
    for component in missing.reversed() {
        resolved.appendPathComponent(component)
    }
    return resolved.standardizedFileURL.path
}

func containWithin(
    roots: [String],
    path: Any
) throws -> String? {
    guard let path = path as? String, !path.isEmpty, path.hasPrefix("/") else { return nil }
    let resolved = URL(fileURLWithPath: path).standardizedFileURL.path
    guard roots.contains(where: { isPathWithin(parent: $0, child: resolved) }) else { return nil }

    let realResolved = try realpathNearestExisting(resolved)
    let realRoots = try roots.map(realpathNearestExisting)
    return realRoots.contains(where: { isPathWithin(parent: $0, child: realResolved) }) ? resolved : nil
}

import Foundation

func resolveDefaultDownloadDirectory(
    configuredDirectory: String?,
    osDownloadsDirectory: String
) -> String {
    if let configured = configuredDirectory?.trimmingCharacters(in: .whitespacesAndNewlines),
       !configured.isEmpty,
       (configured as NSString).isAbsolutePath {
        return configured
    }
    return osDownloadsDirectory
}

func resolveDefaultDownloadPath(
    configuredDirectory: String?,
    osDownloadsDirectory: String,
    fileName: String
) -> String {
    let directory = resolveDefaultDownloadDirectory(
        configuredDirectory: configuredDirectory,
        osDownloadsDirectory: osDownloadsDirectory
    )
    let safeName = (fileName as NSString).lastPathComponent
    return (directory as NSString).appendingPathComponent(safeName)
}

func resolveSuggestedDownloadName(
    sourcePath: String,
    suggestedName: Any?
) -> String {
    let sourceName = (sourcePath as NSString).lastPathComponent
    let fallback = sourceName.isEmpty || sourceName == "." || sourceName == ".."
        ? "download"
        : sourceName

    guard let suggestedName = suggestedName as? String else {
        return fallback
    }
    let candidate = (suggestedName.trimmingCharacters(in: .whitespacesAndNewlines) as NSString)
        .lastPathComponent
    guard !candidate.isEmpty, candidate != ".", candidate != ".." else {
        return fallback
    }

    let sourceExtension = (sourceName as NSString).pathExtension.lowercased()
    let candidateExtension = (candidate as NSString).pathExtension.lowercased()
    if candidateExtension == sourceExtension
        || (sourceExtension == "bin" && candidateExtension.isEmpty) {
        return candidate
    }
    return fallback
}

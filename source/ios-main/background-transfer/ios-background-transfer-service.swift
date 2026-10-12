import Foundation

protocol IOSBackgroundTransferScheduling: Sendable {
    func enqueueDownload(url: URL, fileName: String?) async throws -> Int
}

final class IOSBackgroundTransferService:
    NSObject,
    URLSessionDownloadDelegate,
    URLSessionTaskDelegate,
    IOSBackgroundTransferScheduling,
    @unchecked Sendable
{
    static let shared = IOSBackgroundTransferService()
    static let sessionIdentifier = "com.ombhrum.fabushi.background-transfer.v1"
    static let completionNotification = Notification.Name(
        "com.ombhrum.fabushi.background-transfer.completed"
    )

    enum TransferError: LocalizedError, Equatable {
        case invalidURL
        case invalidFileName
        case unexpectedSessionIdentifier

        var errorDescription: String? {
            switch self {
            case .invalidURL: "background_transfer_invalid_url"
            case .invalidFileName: "background_transfer_invalid_file_name"
            case .unexpectedSessionIdentifier: "background_transfer_unexpected_session"
            }
        }
    }

    private let lock = NSLock()
    private var backgroundEventsCompletionHandler: (() -> Void)?

    private lazy var session: URLSession = {
        let configuration = URLSessionConfiguration.background(
            withIdentifier: Self.sessionIdentifier
        )
        configuration.sessionSendsLaunchEvents = true
        configuration.isDiscretionary = false
        configuration.waitsForConnectivity = true
        return URLSession(
            configuration: configuration,
            delegate: self,
            delegateQueue: nil
        )
    }()

    static func validatedRemoteURL(_ raw: String) throws -> URL {
        guard raw.utf8.count <= 4_096,
              let components = URLComponents(string: raw),
              components.scheme?.lowercased() == "https",
              let host = components.host,
              !host.isEmpty,
              components.user == nil,
              components.password == nil,
              components.fragment == nil,
              let url = components.url
        else {
            throw TransferError.invalidURL
        }
        return url
    }

    static func validatedFileName(_ raw: String?) throws -> String? {
        guard let raw else { return nil }
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty,
              value.utf8.count <= 180,
              value != ".",
              value != "..",
              !value.contains("/"),
              !value.contains("\\"),
              !value.unicodeScalars.contains(where: { $0.value < 32 })
        else {
            throw TransferError.invalidFileName
        }
        return value
    }

    func enqueueDownload(url: URL, fileName: String?) async throws -> Int {
        let task = session.downloadTask(with: url)
        task.taskDescription = fileName
        let identifier = task.taskIdentifier
        task.resume()
        return identifier
    }

    func handleEvents(
        forBackgroundURLSession identifier: String,
        completionHandler: @escaping () -> Void
    ) {
        guard identifier == Self.sessionIdentifier else {
            completionHandler()
            return
        }
        lock.lock()
        backgroundEventsCompletionHandler = completionHandler
        lock.unlock()
        _ = session
    }

    func urlSession(
        _ session: URLSession,
        downloadTask: URLSessionDownloadTask,
        didFinishDownloadingTo location: URL
    ) {
        let fileName = (try? Self.validatedFileName(downloadTask.taskDescription))
            ?? sanitizedResponseFileName(downloadTask.response?.suggestedFilename)
            ?? "download"
        let destination = transferDirectory()
            .appendingPathComponent(
                "\(downloadTask.taskIdentifier)-\(fileName)",
                isDirectory: false
            )
        do {
            try FileManager.default.createDirectory(
                at: destination.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            if FileManager.default.fileExists(atPath: destination.path) {
                try FileManager.default.removeItem(at: destination)
            }
            try FileManager.default.moveItem(at: location, to: destination)
        } catch {
            // Completion status is reported from didCompleteWithError.
        }
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        NotificationCenter.default.post(
            name: Self.completionNotification,
            object: nil,
            userInfo: [
                "taskIdentifier": task.taskIdentifier,
                "ok": error == nil,
                "error": error.map { String(describing: type(of: $0)) } ?? "",
            ]
        )
    }

    func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
        lock.lock()
        let completion = backgroundEventsCompletionHandler
        backgroundEventsCompletionHandler = nil
        lock.unlock()

        guard let completion else { return }
        DispatchQueue.main.async {
            completion()
        }
    }

    private func transferDirectory() -> URL {
        let base = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first!
        return base
            .appendingPathComponent("com.ombhrum.fabushi", isDirectory: true)
            .appendingPathComponent("background-transfers", isDirectory: true)
    }

    private func sanitizedResponseFileName(_ raw: String?) -> String? {
        try? Self.validatedFileName(raw)
    }
}

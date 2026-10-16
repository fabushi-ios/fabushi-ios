import Foundation

enum PackageContextLogLevel: String, Equatable, Sendable {
    case debug
    case info
    case warn
    case error
}

indirect enum PackageLogValue: Equatable, Sendable {
    case null
    case string(String)
    case number(Double)
    case bool(Bool)
    case array([PackageLogValue])
    case object([String: PackageLogValue])

    static func fromFoundation(_ value: Any?) -> PackageLogValue {
        guard let value else { return .null }
        if value is NSNull { return .null }
        if let value = value as? Bool { return .bool(value) }
        if let value = value as? String { return .string(value) }
        if let value = value as? Int { return .number(Double(value)) }
        if let value = value as? Int64 { return .number(Double(value)) }
        if let value = value as? Double { return .number(value) }
        if let value = value as? NSNumber { return .number(value.doubleValue) }
        if let value = value as? [Any] { return .array(value.map(Self.fromFoundation)) }
        if let value = value as? [String: Any] {
            return .object(value.mapValues(Self.fromFoundation))
        }
        return .string(String(describing: value))
    }

    fileprivate func rendered(maxStringLength: Int = 200) -> String {
        switch self {
        case .null:
            return "null"
        case .string(let value):
            let bounded = value.count <= maxStringLength
                ? value
                : String(value.prefix(maxStringLength)) + "..."
            return String(reflecting: bounded)
        case .number(let value):
            return String(value)
        case .bool(let value):
            return String(value)
        case .array(let values):
            let rendered = values.prefix(40).map {
                $0.rendered(maxStringLength: maxStringLength)
            }.joined(separator: ", ")
            return "[" + rendered + (values.count > 40 ? ", ..." : "") + "]"
        case .object(let values):
            let rendered = values.keys.sorted().prefix(50).map { key in
                "\(key): \(values[key]!.rendered(maxStringLength: maxStringLength))"
            }
            return "{" + rendered.joined(separator: ", ") + (values.count > 50 ? ", ..." : "") + "}"
        }
    }
}

typealias PackageLogMetadata = [String: PackageLogValue]

struct PackageContextLogEntry: Equatable, Sendable {
    let level: PackageContextLogLevel
    let message: String
    let timestamp: Date
    let contextPath: [String]
    let errorName: String?
    let errorMessage: String?
    let metadata: PackageLogMetadata
}

protocol PackageContextLoggerBackend: AnyObject {
    func log(context: PackageContext, entry: PackageContextLogEntry)
}

private final class PackageConsoleLoggerBackend: PackageContextLoggerBackend, @unchecked Sendable {
    static let shared = PackageConsoleLoggerBackend()
    private init() {}

    func log(context _: PackageContext, entry: PackageContextLogEntry) {
        var parts = [entry.level.rawValue.uppercased(), entry.message]
        if !entry.contextPath.isEmpty {
            parts.append("ctx=\(entry.contextPath.joined(separator: "/"))")
        }
        if !entry.metadata.isEmpty {
            let rendered = entry.metadata.keys.sorted().map { key in
                "\(key)=\(entry.metadata[key]!.rendered())"
            }
            parts.append("meta={" + rendered.joined(separator: ", ") + "}")
        }
        if let errorName = entry.errorName {
            let message = entry.errorMessage.map { ": \($0)" } ?? ""
            parts.append("error=\(errorName)\(message)")
        }
        NSLog("%@", parts.joined(separator: " "))
    }
}

private final class PackageContextLoggerMiddleware: PackageContextLoggerBackend {
    private let backend: any PackageContextLoggerBackend
    private let attributes: PackageLogMetadata

    init(backend: any PackageContextLoggerBackend, attributes: PackageLogMetadata) {
        self.backend = backend
        self.attributes = attributes
    }

    func log(context: PackageContext, entry: PackageContextLogEntry) {
        var metadata = attributes
        metadata.merge(entry.metadata) { _, new in new }
        backend.log(
            context: context,
            entry: .init(
                level: entry.level,
                message: entry.message,
                timestamp: entry.timestamp,
                contextPath: entry.contextPath,
                errorName: entry.errorName,
                errorMessage: entry.errorMessage,
                metadata: metadata
            )
        )
    }
}

let packageLoggerKey = PackageContextKey<any PackageContextLoggerBackend>(
    defaultValue: PackageConsoleLoggerBackend.shared
)

func packageLoggerBackend(_ context: PackageContext) -> any PackageContextLoggerBackend {
    context.get(packageLoggerKey)
}

struct PackageContextLogger {
    let name: String

    func debug(_ context: PackageContext, _ message: String, metadata: PackageLogMetadata = [:]) {
        log(context, level: .debug, message: message, metadata: metadata)
    }

    func info(_ context: PackageContext, _ message: String, metadata: PackageLogMetadata = [:]) {
        log(context, level: .info, message: message, metadata: metadata)
    }

    func warn(_ context: PackageContext, _ message: String, metadata: PackageLogMetadata = [:]) {
        log(context, level: .warn, message: message, metadata: metadata)
    }

    func error(
        _ context: PackageContext,
        _ message: String,
        error: (any Error)?,
        metadata: PackageLogMetadata = [:]
    ) {
        log(context, level: .error, message: message, error: error, metadata: metadata)
    }

    private func log(
        _ context: PackageContext,
        level: PackageContextLogLevel,
        message: String,
        error: (any Error)? = nil,
        metadata: PackageLogMetadata
    ) {
        let errorInfo = error.map(packageAbortReasonInfo)
        packageLoggerBackend(context).log(
            context: context,
            entry: .init(
                level: level,
                message: message,
                timestamp: Date(),
                contextPath: context.getPath(),
                errorName: errorInfo?.abortReasonName,
                errorMessage: errorInfo?.abortReasonMessage,
                metadata: metadata
            )
        )
    }
}

func packageCreateLogger(_ name: String) -> PackageContextLogger {
    .init(name: name)
}

func packageWithLogAttributes(
    _ context: PackageContext,
    attributes: PackageLogMetadata
) -> PackageContext {
    context.with(
        packageLoggerKey,
        value: PackageContextLoggerMiddleware(
            backend: packageLoggerBackend(context),
            attributes: attributes
        )
    )
}

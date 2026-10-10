import Foundation

enum PackageSpanAttributeValue: Equatable, Sendable {
    case string(String)
    case bool(Bool)
    case int(Int64)
    case double(Double)
    case strings([String])
    case bools([Bool])
    case ints([Int64])
    case doubles([Double])
}

struct PackageSpanContextData: Equatable, Sendable {
    let traceId: String
    let spanId: String
    let traceFlags: Int
    let traceState: String?

    init(traceId: String, spanId: String, traceFlags: Int, traceState: String? = nil) {
        self.traceId = traceId
        self.spanId = spanId
        self.traceFlags = traceFlags
        self.traceState = traceState
    }

    var isValid: Bool {
        parseTraceparent(
            "00-\(traceId)-\(spanId)-\(String(format: "%02x", traceFlags & 0xff))"
        ) != nil
    }
}

protocol PackageSpanHandle: AnyObject {
    var contextData: PackageSpanContextData { get }
    func setAttribute(_ key: String, value: PackageSpanAttributeValue)
    func end(at date: Date?)
}

protocol PackageTracerBackend: AnyObject {
    func startSpan(
        name: String,
        parent: PackageSpanContextData?,
        attributes: [String: PackageSpanAttributeValue]
    ) -> any PackageSpanHandle
}

private final class PackageLocalSpan: PackageSpanHandle {
    let contextData: PackageSpanContextData
    private let lock = NSLock()
    private var attributes: [String: PackageSpanAttributeValue]
    private var endedAt: Date?

    init(
        contextData: PackageSpanContextData,
        attributes: [String: PackageSpanAttributeValue] = [:]
    ) {
        self.contextData = contextData
        self.attributes = attributes
    }

    func setAttribute(_ key: String, value: PackageSpanAttributeValue) {
        lock.lock()
        attributes[key] = value
        lock.unlock()
    }

    func end(at date: Date?) {
        lock.lock()
        if endedAt == nil {
            endedAt = date ?? Date()
        }
        lock.unlock()
    }
}

private final class PackageRemoteSpan: PackageSpanHandle {
    let contextData: PackageSpanContextData

    init(contextData: PackageSpanContextData) {
        self.contextData = contextData
    }

    func setAttribute(_: String, value _: PackageSpanAttributeValue) {}
    func end(at _: Date?) {}
}

private final class PackageLocalTracerBackend: PackageTracerBackend, @unchecked Sendable {
    static let shared = PackageLocalTracerBackend()
    private init() {}

    func startSpan(
        name _: String,
        parent: PackageSpanContextData?,
        attributes: [String: PackageSpanAttributeValue]
    ) -> any PackageSpanHandle {
        let root = mintTraceparent(sampled: true)
        return PackageLocalSpan(
            contextData: .init(
                traceId: parent?.traceId ?? root.traceId,
                spanId: root.spanId,
                traceFlags: parent?.traceFlags ?? 1,
                traceState: parent?.traceState
            ),
            attributes: attributes
        )
    }
}

private let packageSpanKey = PackageContextKey<(any PackageSpanHandle)?>(defaultValue: nil)
private let packageRootSpanKey = PackageContextKey<(any PackageSpanHandle)?>(defaultValue: nil)
private let packageRootSpanStartMsKey = PackageContextKey<Int64?>(defaultValue: nil)
private let packageSuppressChildSpansKey = PackageContextKey(defaultValue: false)
let packageInheritableSpanAttributesKey = PackageContextKey<[String: PackageSpanAttributeValue]>(
    defaultValue: [:]
)
let packageTracerBackendKey = PackageContextKey<any PackageTracerBackend>(
    defaultValue: PackageLocalTracerBackend.shared
)

func packageWithSpan(_ context: PackageContext) -> PackageContext {
    let parent = context.get(packageSpanKey)
    if context.get(packageSuppressChildSpansKey), parent != nil {
        return context
    }

    let backend = context.get(packageTracerBackendKey)
    let inherited = context.get(packageInheritableSpanAttributesKey)
    let span = backend.startSpan(
        name: context.name ?? "anonymous-context",
        parent: parent?.contextData,
        attributes: inherited
    )

    let nowMs = Int64((Date().timeIntervalSince1970 * 1_000).rounded())
    var next = context.with(packageSpanKey, value: Optional(span))
    if parent == nil {
        next = next
            .with(packageRootSpanKey, value: Optional(span))
            .with(packageRootSpanStartMsKey, value: Optional(nowMs))
    }
    return next
}

func packageGetSpan(_ context: PackageContext) -> (any PackageSpanHandle)? {
    context.get(packageSpanKey)
}

func packageWithSuppressedChildSpans(_ context: PackageContext) -> PackageContext {
    context.with(packageSuppressChildSpansKey, value: true)
}

func packageWithInheritableAttribute(
    _ context: PackageContext,
    key: String,
    value: PackageSpanAttributeValue
) -> PackageContext {
    var attributes = context.get(packageInheritableSpanAttributesKey)
    attributes[key] = value
    packageGetSpan(context)?.setAttribute(key, value: value)
    return context.with(packageInheritableSpanAttributesKey, value: attributes)
}

func packageGetSpanContextData(_ context: PackageContext) -> PackageSpanContextData? {
    guard let span = packageGetSpan(context) else { return nil }
    let data = span.contextData
    guard context.get(packageSuppressChildSpansKey) else { return data }
    return .init(
        traceId: data.traceId,
        spanId: data.spanId,
        traceFlags: 0,
        traceState: data.traceState
    )
}

func packageCreateContextFromSpanContext(
    _ spanContext: PackageSpanContextData,
    name: String? = nil,
    existingContext: PackageContext? = nil
) -> PackageContext {
    let parent = existingContext ?? PackageContext.root()
    let named = name.map { parent.withName($0) } ?? parent
    let backend = named.get(packageTracerBackendKey)
    let span = backend.startSpan(
        name: name ?? "child.span",
        parent: spanContext,
        attributes: named.get(packageInheritableSpanAttributesKey)
    )
    return named.with(packageSpanKey, value: Optional(span))
}

func packageCreateContextFromRemoteSpanContext(
    _ spanContext: PackageSpanContextData,
    name: String? = nil,
    existingContext: PackageContext? = nil
) -> PackageContext {
    let parent = existingContext ?? PackageContext.root()
    let named = name.map { parent.withName($0) } ?? parent
    let span: any PackageSpanHandle = PackageRemoteSpan(contextData: spanContext)
    return named.with(packageSpanKey, value: Optional(span))
}

final class PackageDisposableSpan {
    let context: PackageContext
    let span: any PackageSpanHandle
    private let shouldEnd: Bool
    private var ended = false

    init(context: PackageContext, span: any PackageSpanHandle, shouldEnd: Bool) {
        self.context = context
        self.span = span
        self.shouldEnd = shouldEnd
    }

    func end(at date: Date? = nil) {
        guard !ended else { return }
        ended = true
        if shouldEnd {
            span.end(at: date)
        }
    }

    deinit {
        end()
    }
}

func packageCreateSpan(_ context: PackageContext) -> PackageDisposableSpan {
    let parent = packageGetSpan(context)
    let suppressed = context.get(packageSuppressChildSpansKey) && parent != nil
    let contextWithSpan = packageWithSpan(context)
    guard let span = packageGetSpan(contextWithSpan) ?? parent else {
        preconditionFailure("packageCreateSpan requires a span handle")
    }
    return .init(context: contextWithSpan, span: span, shouldEnd: !suppressed)
}

@discardableResult
func packageRecordCompletedSpanIfParented(
    _ context: PackageContext,
    endTime: Date? = nil
) -> (any PackageSpanHandle)? {
    guard packageGetSpan(context) != nil,
          !context.get(packageSuppressChildSpansKey)
    else { return nil }

    let child = packageWithSpan(context)
    let span = packageGetSpan(child)
    span?.end(at: endTime)
    return span
}

func packageReportEvent(_ context: PackageContext, name: String, now: Date = Date()) {
    guard let parent = packageGetSpan(context) else { return }

    let eventSpan = context.get(packageTracerBackendKey).startSpan(
        name: name,
        parent: parent.contextData,
        attributes: context.get(packageInheritableSpanAttributesKey)
    )
    let nowMs = Int64((now.timeIntervalSince1970 * 1_000).rounded())
    let rootStart = context.get(packageRootSpanStartMsKey)
    let delta = max(0, nowMs - (rootStart ?? nowMs))
    let key = "event.\(name)"
    let value = PackageSpanAttributeValue.int(delta)

    (context.get(packageRootSpanKey) ?? parent).setAttribute(key, value: value)
    eventSpan.setAttribute(key, value: value)
    eventSpan.end(at: now)
}

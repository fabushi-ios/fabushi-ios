import Foundation

enum PackageMetricType: String, Equatable, Sendable {
    case counter
    case gauge
    case histogram
}

struct PackageMetricHandle: Equatable, Sendable {
    let name: String
    let type: PackageMetricType
    let description: String?
    let labelNames: [String]?
}

typealias PackageMetricLabels = [String: String]

protocol PackageMetricsBackend: AnyObject {
    func record(
        context: PackageContext,
        handle: PackageMetricHandle,
        value: Double,
        labels: PackageMetricLabels?
    )
    func increment(
        context: PackageContext,
        handle: PackageMetricHandle,
        value: Double,
        labels: PackageMetricLabels?
    )
    func gauge(
        context: PackageContext,
        handle: PackageMetricHandle,
        value: Double,
        labels: PackageMetricLabels?
    )
    func histogram(
        context: PackageContext,
        handle: PackageMetricHandle,
        value: Double,
        labels: PackageMetricLabels?
    )
}

final class PackageNoopMetricsBackend: PackageMetricsBackend, @unchecked Sendable {
    static let shared = PackageNoopMetricsBackend()
    private init() {}

    func record(context _: PackageContext, handle _: PackageMetricHandle, value _: Double, labels _: PackageMetricLabels?) {}
    func increment(context _: PackageContext, handle _: PackageMetricHandle, value _: Double, labels _: PackageMetricLabels?) {}
    func gauge(context _: PackageContext, handle _: PackageMetricHandle, value _: Double, labels _: PackageMetricLabels?) {}
    func histogram(context _: PackageContext, handle _: PackageMetricHandle, value _: Double, labels _: PackageMetricLabels?) {}
}

let packageMetricsKey = PackageContextKey<any PackageMetricsBackend>(
    defaultValue: PackageNoopMetricsBackend.shared
)

func packageMetricsBackend(_ context: PackageContext) -> any PackageMetricsBackend {
    context.get(packageMetricsKey)
}

struct PackageCounter {
    let handle: PackageMetricHandle

    func increment(
        _ context: PackageContext,
        value: Double = 1,
        labels: PackageMetricLabels? = nil
    ) {
        packageMetricsBackend(context).increment(
            context: context,
            handle: handle,
            value: value,
            labels: labels
        )
    }

    func record(
        _ context: PackageContext,
        value: Double,
        labels: PackageMetricLabels? = nil
    ) {
        packageMetricsBackend(context).record(
            context: context,
            handle: handle,
            value: value,
            labels: labels
        )
    }
}

struct PackageGauge {
    let handle: PackageMetricHandle

    func gauge(
        _ context: PackageContext,
        value: Double,
        labels: PackageMetricLabels? = nil
    ) {
        packageMetricsBackend(context).gauge(
            context: context,
            handle: handle,
            value: value,
            labels: labels
        )
    }

    func record(
        _ context: PackageContext,
        value: Double,
        labels: PackageMetricLabels? = nil
    ) {
        packageMetricsBackend(context).record(
            context: context,
            handle: handle,
            value: value,
            labels: labels
        )
    }
}

struct PackageHistogram {
    let handle: PackageMetricHandle

    func histogram(
        _ context: PackageContext,
        value: Double,
        labels: PackageMetricLabels? = nil
    ) {
        packageMetricsBackend(context).histogram(
            context: context,
            handle: handle,
            value: value,
            labels: labels
        )
    }

    func record(
        _ context: PackageContext,
        value: Double,
        labels: PackageMetricLabels? = nil
    ) {
        packageMetricsBackend(context).record(
            context: context,
            handle: handle,
            value: value,
            labels: labels
        )
    }
}

func packageCreateCounter(
    _ name: String,
    description: String? = nil,
    labelNames: [String]? = nil
) -> PackageCounter {
    .init(handle: .init(
        name: name,
        type: .counter,
        description: description,
        labelNames: labelNames
    ))
}

func packageCreateGauge(
    _ name: String,
    description: String? = nil,
    labelNames: [String]? = nil
) -> PackageGauge {
    .init(handle: .init(
        name: name,
        type: .gauge,
        description: description,
        labelNames: labelNames
    ))
}

func packageCreateHistogram(
    _ name: String,
    description: String? = nil,
    labelNames: [String]? = nil
) -> PackageHistogram {
    .init(handle: .init(
        name: name,
        type: .histogram,
        description: description,
        labelNames: labelNames
    ))
}

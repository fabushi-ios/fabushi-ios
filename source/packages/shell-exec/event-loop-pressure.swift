import Foundation

enum EventLoopPressureThresholdDefault {
    static let eventLoopDelayP95Ms: Double = 50
    static let eventLoopUtilization: Double = 0.7
}

enum EventLoopPressureTrackerDefault {
    static let sampleIntervalMs: Int = 250
    static let eventLoopResolutionMs: Int = 20
}

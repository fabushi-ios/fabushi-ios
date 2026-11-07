//
//  MessageClusterCalculator.swift
//  Fabushi
//
//  5-minute (300s) same-sender clustering calculator and corner radii generator.
//  Determines .single, .top, .middle, .bottom positioning and tail placement.
//

import CoreGraphics
import Foundation

public struct MessageClusterCalculator: Sendable {
    public static let clusterThresholdMs: Int64 = 300_000 // 5 minutes in milliseconds

    /// Evaluates if two messages belong to the same cluster.
    public static func canCluster(
        _ first: TelegramStageMessage,
        _ second: TelegramStageMessage
    ) -> Bool {
        guard first.senderId == second.senderId,
              first.isOutgoing == second.isOutgoing
        else {
            return false
        }
        return abs(first.timestamp - second.timestamp) <= clusterThresholdMs
    }

    /// Calculates cluster position and corner radii given the current message and optional older/newer neighbors.
    ///
    /// - Parameters:
    ///   - current: The target message.
    ///   - older: The chronologically preceding message (sent before `current`).
    ///   - newer: The chronologically following message (sent after `current`).
    ///   - isRTL: Whether Right-to-Left layout is active (e.g. Arabic/Hebrew).
    public static func calculate(
        current: TelegramStageMessage,
        older: TelegramStageMessage?,
        newer: TelegramStageMessage?,
        isRTL: Bool = false
    ) -> (MessageClusterPosition, BubbleCornerRadii) {
        let gOlder = older.map { canCluster(current, $0) } ?? false
        let gNewer = newer.map { canCluster(current, $0) } ?? false

        let position: MessageClusterPosition
        if !gOlder && !gNewer {
            position = .single
        } else if !gOlder && gNewer {
            position = .top
        } else if gOlder && gNewer {
            position = .middle
        } else {
            position = .bottom
        }

        // Tail is drawn only on the bottom-most message of a cluster (.single or .bottom)
        let hasTail = !gNewer

        // Effective sender side: outgoing is right unless RTL is active
        let isAnchoredRight = isRTL ? !current.isOutgoing : current.isOutgoing

        let radii: BubbleCornerRadii
        if isAnchoredRight {
            // Outgoing / Right-aligned:
            // Leading (left) corners are outer: always 16pt
            // Trailing (right) corners are inner: top-right = gOlder ? 4 : 16; bottom-right = gNewer ? 4 : 16
            radii = BubbleCornerRadii(
                topLeft: 16.0,
                topRight: gOlder ? 4.0 : 16.0,
                bottomLeft: 16.0,
                bottomRight: gNewer ? 4.0 : 16.0,
                hasTail: hasTail
            )
        } else {
            // Incoming / Left-aligned:
            // Trailing (right) corners are outer: always 16pt
            // Leading (left) corners are inner: top-left = gOlder ? 4 : 16; bottom-left = gNewer ? 4 : 16
            radii = BubbleCornerRadii(
                topLeft: gOlder ? 4.0 : 16.0,
                topRight: 16.0,
                bottomLeft: gNewer ? 4.0 : 16.0,
                bottomRight: 16.0,
                hasTail: hasTail
            )
        }

        return (position, radii)
    }

    /// Convenience method for an array of messages.
    ///
    /// - Parameters:
    ///   - messages: Array of stage messages.
    ///   - index: Index of current message in the array.
    ///   - isInverted: If true, index 0 is the newest message (chat stage default).
    ///                 If false, index 0 is the oldest message (chronological).
    ///   - isRTL: Right-to-Left layout flag.
    public static func calculate(
        for messages: [TelegramStageMessage],
        at index: Int,
        isInverted: Bool = true,
        isRTL: Bool = false
    ) -> (MessageClusterPosition, BubbleCornerRadii) {
        guard index >= 0 && index < messages.count else {
            return (.single, .standardSingle)
        }

        let current = messages[index]
        let older: TelegramStageMessage?
        let newer: TelegramStageMessage?

        if isInverted {
            // Inverted: index + 1 is older, index - 1 is newer
            older = index + 1 < messages.count ? messages[index + 1] : nil
            newer = index > 0 ? messages[index - 1] : nil
        } else {
            // Chronological: index - 1 is older, index + 1 is newer
            older = index > 0 ? messages[index - 1] : nil
            newer = index + 1 < messages.count ? messages[index + 1] : nil
        }

        return calculate(current: current, older: older, newer: newer, isRTL: isRTL)
    }
}

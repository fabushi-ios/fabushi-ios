//
//  MessageDeliveryTicksView.swift
//  Fabushi
//
//  Delivery status tick marks (.pending clock, .sent single check, .read double check, .failed)
//  and inline timestamp pill for Telegram message bubbles.
//

import SwiftUI

public struct MessageDeliveryTicksView: View {
    public let timestamp: Int64 // Epoch milliseconds
    public let status: TelegramDeliveryStatus
    public let isOutgoing: Bool
    public let isEdited: Bool
    public let onRetryFailed: (() -> Void)?

    @Environment(\.colorScheme) private var colorScheme

    public init(
        timestamp: Int64,
        status: TelegramDeliveryStatus,
        isOutgoing: Bool,
        isEdited: Bool = false,
        onRetryFailed: (() -> Void)? = nil
    ) {
        self.timestamp = timestamp
        self.status = status
        self.isOutgoing = isOutgoing
        self.isEdited = isEdited
        self.onRetryFailed = onRetryFailed
    }

    private var formattedTime: String {
        let date = Date(timeIntervalSince1970: TimeInterval(timestamp) / 1000.0)
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: date)
    }

    private var tickColor: Color {
        if !isOutgoing {
            return .secondary.opacity(0.8)
        }
        return colorScheme == .dark
            ? Color(conversationHex: "#73B980")
            : Color(conversationHex: "#5EA852")
    }

    public var body: some View {
        HStack(spacing: 3) {
            if isEdited {
                Text("edited")
                    .font(.system(size: 11, weight: .regular))
                    .foregroundStyle(tickColor.opacity(0.8))
                    .accessibilityIdentifier("message-edited-label")
            }

            Text(formattedTime)
                .font(.system(size: 11, weight: .regular, design: .default))
                .foregroundStyle(tickColor)
                .accessibilityIdentifier("message-timestamp-label")

            if isOutgoing {
                switch status {
                case .pending:
                    Image(systemName: "clock")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(tickColor)
                        .accessibilityIdentifier("message-delivery-ticks-pending")

                case .sent:
                    Image(systemName: "checkmark")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(tickColor)
                        .accessibilityIdentifier("message-delivery-ticks-sent")

                case .read:
                    HStack(spacing: -6) {
                        Image(systemName: "checkmark")
                            .font(.system(size: 11, weight: .semibold))
                        Image(systemName: "checkmark")
                            .font(.system(size: 11, weight: .semibold))
                    }
                    .foregroundStyle(tickColor)
                    .accessibilityIdentifier("message-delivery-ticks-read")

                case .failed:
                    Button(action: { onRetryFailed?() }) {
                        Image(systemName: "exclamationmark.circle.fill")
                            .font(.system(size: 12, weight: .bold))
                            .foregroundStyle(Color.red)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("message-delivery-ticks-failed")
                }
            }
        }
        .padding(.horizontal, 4)
        .padding(.vertical, 2)
    }
}

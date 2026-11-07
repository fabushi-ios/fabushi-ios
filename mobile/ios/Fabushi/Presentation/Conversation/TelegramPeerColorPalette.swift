//
//  TelegramPeerColorPalette.swift
//  Fabushi
//
//  Deterministic 64-bit FNV-1a hash mod 7 Telegram sender color palette.
//  Used for sender headers in group chats and avatars.
//

import SwiftUI

public struct TelegramPeerColorPalette: Sendable {
    // 7 Canonical Telegram Sender Colors
    public static let hexValues: [String] = [
        "#E17076", // 0: Red
        "#FAA774", // 1: Orange
        "#A695E7", // 2: Violet
        "#7BC862", // 3: Green
        "#6EC9CB", // 4: Cyan
        "#65AADD", // 5: Blue
        "#EE7AAE"  // 6: Pink
    ]

    public static let colors: [Color] = hexValues.map { Color(conversationHex: $0) }

    /// Returns the deterministic 0..6 palette index using 64-bit FNV-1a hash.
    public static func colorIndex(for senderId: String) -> Int {
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in senderId.utf8 {
            hash ^= UInt64(byte)
            hash &*= 0x100000001b3
        }
        return Int(hash % 7)
    }

    /// Convenience for Int64 senderId
    public static func colorIndex(for senderId: Int64) -> Int {
        colorIndex(for: String(senderId))
    }

    /// Resolves the sender color for a given sender identifier string.
    public static func color(for senderId: String) -> Color {
        let index = colorIndex(for: senderId)
        return colors[index]
    }

    /// Resolves the sender color for a numeric sender ID.
    public static func color(for senderId: Int64) -> Color {
        color(for: String(senderId))
    }
}

// MARK: - Internal Hex Color Parser
extension Color {
    init(conversationHex hexString: String) {
        var clean = hexString.trimmingCharacters(in: .whitespacesAndNewlines)
        if clean.hasPrefix("#") {
            clean.removeFirst()
        }

        var rgbValue: UInt64 = 0
        Scanner(string: clean).scanHexInt64(&rgbValue)

        let r = Double((rgbValue >> 16) & 0xFF) / 255.0
        let g = Double((rgbValue >> 8) & 0xFF) / 255.0
        let b = Double(rgbValue & 0xFF) / 255.0

        self.init(.sRGB, red: r, green: g, blue: b, opacity: 1.0)
    }
}

//
//  TelegramReplayWindow.swift
//  Fabushi
//
//  Production 128-bit sliding replay window bridging iOS native client with Telegram Rust core C-ABI.
//

import Foundation

public final class TelegramReplayWindow: @unchecked Sendable {
    private let raw: UnsafeMutableRawPointer

    public init() {
        guard let ptr = telegram_replay_window_create() else {
            fatalError("Failed to allocate telegram_replay_window via Rust C-ABI")
        }
        self.raw = ptr
    }

    deinit {
        telegram_replay_window_destroy(raw)
    }

    @discardableResult
    public func checkAndAdd(seq: UInt64) -> Bool {
        telegram_replay_window_check_and_add(raw, seq)
    }
}

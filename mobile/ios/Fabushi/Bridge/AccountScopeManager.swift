//
//  AccountScopeManager.swift
//  Fabushi
//
//  Feature 37: Account Scope Generation Fence (1:1 Telegram Native).
//  Swift 6 actor maintaining monotonic generation counter UInt64,
//  fencing in-flight async dispatches, stale callback rejection, and logout epoch scrubbing.
//

import Foundation

public enum FenceRejectionError: Error, Equatable, LocalizedError {
    case staleGeneration(taskGen: UInt64, currentGen: UInt64)
    case accountNotFound(String)
    case generationOverflow

    public var errorDescription: String? {
        switch self {
        case .staleGeneration(let taskGen, let currentGen):
            return "Task generation \(taskGen) was invalidated by generation advance to \(currentGen)"
        case .accountNotFound(let id):
            return "Account \(id) not found in keychain"
        case .generationOverflow:
            return "Account generation counter reached UInt64 maximum"
        }
    }
}

public actor AccountScopeManager {
    public static let shared = AccountScopeManager()

    public private(set) var currentGeneration: UInt64 = 1
    public private(set) var activeAccountId: String? = "acc_primary"
    private var inMemoryCryptoKeys: [String: Data] = [:]
    private var knownAccounts: Set<String> = ["acc_primary", "acc_work"]

    public init(initialGeneration: UInt64 = 1, activeAccountId: String? = "acc_primary") {
        self.currentGeneration = initialGeneration
        self.activeAccountId = activeAccountId
        if let acc = activeAccountId {
            self.inMemoryCryptoKeys[acc] = Data(repeating: 0xAA, count: 32)
        }
    }

    public func getCurrentGeneration() -> UInt64 {
        currentGeneration
    }

    public func getActiveAccountId() -> String? {
        activeAccountId
    }

    /// Switches active account and advances generation fence monotonically.
    public func switchAccount(to newAccountId: String) async throws -> UInt64 {
        guard currentGeneration < UInt64.max else {
            throw FenceRejectionError.generationOverflow
        }

        currentGeneration += 1
        activeAccountId = newAccountId
        knownAccounts.insert(newAccountId)
        inMemoryCryptoKeys[newAccountId] = Data(repeating: 0xBB, count: 32)
        return currentGeneration
    }

    /// Logs out current active account, increments epoch, and scrubs in-memory keys.
    public func logout() async -> UInt64 {
        if currentGeneration < UInt64.max {
            currentGeneration += 1
        }
        activeAccountId = nil
        inMemoryCryptoKeys.removeAll()
        return currentGeneration
    }

    /// Validates whether a captured generation equals current active generation.
    public func validateGeneration(_ taskGen: UInt64) -> Bool {
        taskGen == currentGeneration
    }

    /// Executes an async task fenced by generation. If generation advanced while task was running,
    /// discards output and throws `FenceRejectionError.staleGeneration`.
    public func executeFenced<T: Sendable>(_ task: @Sendable (UInt64) async throws -> T) async throws -> T {
        let capturedGen = currentGeneration
        let result = try await task(capturedGen)

        guard currentGeneration == capturedGen else {
            throw FenceRejectionError.staleGeneration(taskGen: capturedGen, currentGen: currentGeneration)
        }

        return result
    }

    /// Fast-forwards generation counter for testing.
    public func setGenerationForTesting(_ gen: UInt64) {
        self.currentGeneration = gen
    }

    public func hasActiveKey(for accountId: String) -> Bool {
        inMemoryCryptoKeys[accountId] != nil
    }
}

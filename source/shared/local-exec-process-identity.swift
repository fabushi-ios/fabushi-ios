import Foundation

let LOCAL_EXEC_GENERATION_TOKEN_ENV = "SAND_LOCAL_EXEC_GENERATION_TOKEN"
let LOCAL_EXEC_GENERATION_TOKEN_ARG = "--sand-local-exec-generation="
let LOCAL_EXEC_DAEMON_PUBLICATION_LAG_MS: Int64 = 60_000

struct LocalExecProcessIdentity: Equatable, Sendable {
    let pid: Int
    let startEpochMs: Int64
    let command: String
    let entryRealpath: String
    let generationToken: String
}

struct ExpectedLocalExecProcessIdentity: Equatable, Sendable {
    let pid: Int
    let entryRealpath: String
    let generationToken: String
    let startEpochMs: Int64?
    let command: String?
    let discoveryStartedAt: Int64?
}

private func containsExactArgument(_ command: String, argument: String) -> Bool {
    guard !argument.isEmpty else { return false }
    var cursor = command.startIndex
    while cursor < command.endIndex,
          let range = command.range(of: argument, range: cursor..<command.endIndex) {
        let beforeIsBoundary = range.lowerBound == command.startIndex
            || command[command.index(before: range.lowerBound)].isWhitespace
        let afterIsBoundary = range.upperBound == command.endIndex
            || command[range.upperBound].isWhitespace
        if beforeIsBoundary && afterIsBoundary {
            return true
        }
        cursor = range.upperBound
    }
    return false
}

func commandCarriesLocalExecGeneration(
    _ command: String,
    entryRealpath: String,
    generationToken: String
) -> Bool {
    !entryRealpath.isEmpty
        && !generationToken.isEmpty
        && containsExactArgument(command, argument: entryRealpath)
        && containsExactArgument(
            command,
            argument: "\(LOCAL_EXEC_GENERATION_TOKEN_ARG)\(generationToken)"
        )
}

func sameLocalExecProcessIdentity(
    _ left: LocalExecProcessIdentity,
    _ right: LocalExecProcessIdentity
) -> Bool {
    left == right
}

func localExecDiscoveryTimeMatchesProcess(
    _ discoveryStartedAt: Int64,
    processStartEpochMs: Int64,
    observedAtMs: Int64
) -> Bool {
    discoveryStartedAt >= processStartEpochMs
        && discoveryStartedAt - processStartEpochMs <= LOCAL_EXEC_DAEMON_PUBLICATION_LAG_MS
        && discoveryStartedAt <= observedAtMs
}

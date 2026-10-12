import Foundation

/// Native iOS entrypoint for Grok's box-exec daemon responsibility.
/// There is intentionally no command-line process or localhost listener.
enum IOSBoxExecEntrypoint {
    static func makeServer(transport: any RemoteRunnerTransport) -> IOSBoxExecServerAdapter {
        IOSBoxExecServerAdapter(runner: RemoteRunner(transport: transport))
    }
}

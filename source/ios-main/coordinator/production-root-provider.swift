import Foundation

@MainActor
enum ProductionCoordinatorRootProvider {
    static func make(
        appDataDirectory: URL,
        featureHostTest: Bool = false,
        auxiliary: ProductionCoordinatorAuxiliaryPorts = .live()
    ) throws -> ProductionCoordinatorProvider {
        let main = try IOSMainRuntime(
            appDataDirectory: appDataDirectory,
            featureHostTest: featureHostTest
        )
        return ProductionCoordinatorProvider(main: main, auxiliary: auxiliary)
    }
}

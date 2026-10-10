import Foundation

enum ClientPersistenceChannels {
    static let read = "sand:client-persistence-read"
    static let write = "sand:client-persistence-write"
    static let remove = "sand:client-persistence-remove"
    static let listKeys = "sand:client-persistence-list-keys"
    static let migrate = "sand:client-persistence-migrate"
}

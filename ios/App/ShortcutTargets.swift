import AppIntents
import Foundation

struct XLatchTarget: AppEntity {
    static var typeDisplayRepresentation = TypeDisplayRepresentation(name: "xlatch target")
    static var defaultQuery = XLatchTargetQuery()
    let id: String
    let title: String
    let server: String
    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(title)", subtitle: "\(server)")
    }
    init(connection: Connection, capability: Capability) {
        id = Self.identifier(connection: connection, capability: capability)
        title = capability.manifest.title
        server = URL(string: connection.url)?.host ?? connection.url
    }
    static func identifier(connection: Connection, capability: Capability) -> String {
        "\(connection.deviceID):\(capability.revision):\(capability.id)"
    }
}

struct XLatchTargetQuery: EntityStringQuery {
    func entities(for identifiers: [String]) async throws -> [XLatchTarget] {
        try await suggestedEntities().filter { identifiers.contains($0.id) }
    }
    func suggestedEntities() async throws -> [XLatchTarget] { try await QuickSend.targets() }
    func entities(matching string: String) async throws -> [XLatchTarget] {
        try await suggestedEntities().filter { $0.title.localizedCaseInsensitiveContains(string) || $0.server.localizedCaseInsensitiveContains(string) }
    }
}

@MainActor enum QuickSend {
    static var selectedID: String? {
        get { UserDefaults.standard.string(forKey: "quick-send-target") }
        set { UserDefaults.standard.set(newValue, forKey: "quick-send-target") }
    }
    static func connection() throws -> Connection {
        guard let connection = try CredentialStore.load() else { throw ClientError.message("Open xlatch and pair a server first.") }
        return connection
    }
    static func available(_ capabilities: [Capability], connection: Connection) -> [Capability] {
        let disabled = ShareActionPreferences.disabled(deviceID: connection.deviceID)
        return ShareActionPreferences.ordered(capabilities.filter { $0.status == "active" && !disabled.contains($0.id) }, deviceID: connection.deviceID)
    }
    static func targets() async throws -> [XLatchTarget] {
        let connection = try connection()
        let cached = APIClient.cachedCapabilities()
        let actions: [Capability]
        if cached.isEmpty { actions = try await APIClient(connection: connection).capabilities() }
        else { actions = cached }
        return available(actions, connection: connection).map { XLatchTarget(connection: connection, capability: $0) }
    }
    static func resolve(_ id: String, connection: Connection, capabilities: [Capability], mime: String) throws -> Capability {
        guard let capability = available(capabilities, connection: connection).first(where: {
            XLatchTarget.identifier(connection: connection, capability: $0) == id
        }) else { throw ClientError.message("This target is unavailable, disabled or changed. Choose it again in xlatch → Server → Shortcuts & Back Tap.") }
        guard capability.accepts(mime) else { throw ClientError.message("This target does not accept \(mime). Choose a compatible target.") }
        return capability
    }
    static func send(_ input: ShareInput, target: XLatchTarget?) async throws -> OutboxItem {
        guard let id = target?.id ?? selectedID else { throw ClientError.message("Choose a quick-send target in xlatch → Server → Shortcuts & Back Tap.") }
        let connection = try connection()
        let client = try APIClient(connection: connection)
        let cached = APIClient.cachedCapabilities()
        let actions: [Capability]
        if cached.isEmpty { actions = try await client.capabilities() }
        else { actions = cached }
        let action = try resolve(id, connection: connection, capabilities: actions, mime: input.mime)
        return try await OutboxDelivery.shared.submit(input, capability: action, connection: connection)
    }
}

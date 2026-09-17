import Foundation

actor OutboxDelivery {
    static let shared = OutboxDelivery()
    private let makeStore: () throws -> OutboxStore
    private let connection: () throws -> Connection?
    private let deliver: (OutboxItem, Connection) async throws -> Job
    private let schedule: () -> Void
    private var draining = false
    private(set) var lastError: String?

    init(store: @escaping () throws -> OutboxStore = { try OutboxStore() },
         connection: @escaping () throws -> Connection? = CredentialStore.load,
         deliver: @escaping (OutboxItem, Connection) async throws -> Job = OutboxDelivery.invoke,
         schedule: @escaping () -> Void = OutboxBackground.schedule) {
        makeStore = store; self.connection = connection; self.deliver = deliver; self.schedule = schedule
    }
    func submit(_ input: ShareInput, capability: Capability, connection: Connection, id: String = UUID().uuidString) async throws -> OutboxItem {
        let store = try makeStore()
        let item = try store.enqueue(OutboxItem(id: id, input: input, capability: capability, connection: connection, now: Date()))
        schedule()
        await drain(id: item.id)
        return try store.items().first(where: { $0.id == item.id }) ?? item
    }
    func drain(id: String? = nil, expedite: Bool = false) async {
        guard !draining else { return }
        draining = true; lastError = nil
        defer { draining = false; schedule() }
        do {
            let store = try makeStore()
            if expedite { try store.expedite() }
            // Bound each wake; the next foreground/background opportunity resumes the rest.
            for _ in 0..<10 {
                guard !Task.isCancelled, let item = try store.claim(id: id) else { break }
                do {
                    guard let current = try connection(), try item.matches(current) else { throw ClientError.message("Pairing changed or was removed. This share will not be sent to another server or device.") }
                    guard !ShareActionPreferences.disabled(deviceID: current.deviceID).contains(item.capability.id) else { throw ClientError.message("This action is disabled on this phone. Enable it before retrying.") }
                    try Task.checkCancellation()
                    guard try store.owns(item) else { continue }
                    let job = try await deliver(item, current)
                    try store.finish(item, job: job)
                } catch {
                    try store.finish(item, error: error.localizedDescription, retry: ClientError.isRetryable(error))
                }
            }
        } catch {
            lastError = "Outbox storage needs attention: " + error.localizedDescription
        }
    }
    static func invoke(_ item: OutboxItem, connection: Connection) async throws -> Job {
        let client = try APIClient(connection: connection)
        let current = try await client.capabilities()
        guard current.contains(where: { $0.id == item.capability.id && $0.revision == item.capability.revision && $0.status == "active" }) else {
            throw ClientError.message("The target was removed, changed, or is no longer granted. Review this share; it will not switch targets automatically.")
        }
        try Task.checkCancellation()
        return try await client.invoke(item.capability, input: item.input(), key: item.id)
    }
}

import Foundation

actor OutboxDelivery {
    static let shared = OutboxDelivery()
    private let makeStore: () throws -> OutboxStore
    private let connections: () throws -> [Connection]
    private let deliver: (OutboxItem, Connection) async throws -> Job
    private let schedule: () -> Void
    private var draining = false
    private(set) var lastError: String?

    init(store: @escaping () throws -> OutboxStore = { try OutboxStore() },
         connections: @escaping () throws -> [Connection] = CredentialStore.loadAll,
         deliver: ((OutboxItem, Connection) async throws -> Job)? = nil,
         schedule: @escaping () -> Void = OutboxBackground.schedule) {
        makeStore = store; self.connections = connections
        self.deliver = deliver ?? { item, connection in try await OutboxDelivery.invoke(item, connection: connection, store: store()) }
        self.schedule = schedule
    }
    func submit(_ input: ShareInput, capability: Capability, connection: Connection, id: String = UUID().uuidString, chain: [Capability]? = nil) async throws -> OutboxItem {
        let store = try makeStore()
        let item = try store.enqueue(OutboxItem(id: id, input: input, capability: capability, connection: connection, now: Date(), chain: chain))
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
                    guard let current = try connections().first(where: { try item.matches($0) }) else { throw ClientError.message("Pairing changed or was removed. This share will not be sent to another server or device.") }
                    guard !(item.chain ?? [item.capability]).contains(where: { ShareActionPreferences.disabled(deviceID: current.deviceID).contains($0.id) }) else { throw ClientError.message("This action is disabled on this phone. Enable it before retrying.") }
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
    static func invoke(_ item: OutboxItem, connection: Connection, store: OutboxStore) async throws -> Job {
        let client = try APIClient(connection: connection)
        let current = try await client.capabilities()
        guard (item.chain ?? [item.capability]).allSatisfy({ step in current.contains(where: { $0.id == step.id && $0.revision == step.revision && $0.status == "active" }) }) else {
            throw ClientError.message("The target was removed, changed, or is no longer granted. Review this share; it will not switch targets automatically.")
        }
        try Task.checkCancellation()
        let progress = UploadReporter(item: item, store: store)
        var input = try item.input()
        if input.localFile != nil {
            let first = item.chain?.first ?? item.capability
            guard first.manifest.file_input == "path" || first.manifest.execution?.kind == "save_file" else {
                throw ClientError.message("This action uses the old inline-file adapter. Update it for large-file paths, or choose Save to server. Your file remains in Outbox.")
            }
            input = try await upload(item, input: input, client: client, progress: progress, store: store)
        }
        guard try store.owns(item) else { throw CancellationError() }
        if let chain = item.chain {
            return try await client.rpc(["op": "invoke_chain", "steps": chain.map { ["capability_id": $0.id, "revision": $0.revision] }, "input": input.payload, "idempotency_key": item.id], progress: item.localFile == nil ? progress.update : nil)
        }
        return try await client.invoke(item.capability, input: input, key: item.id, progress: item.localFile == nil ? progress.update : nil)
    }
    private struct UploadState: Decodable { let offset: UInt64; let complete: Bool; let chunk_bytes: Int }
    private static func upload(_ item: OutboxItem, input: ShareInput, client: APIClient, progress: UploadReporter, store: OutboxStore) async throws -> ShareInput {
        guard let url = input.localFile, let metadata = input.payload["file"] as? [String: Any],
              let name = metadata["name"] as? String, let mime = metadata["mime_type"] as? String else { throw ClientError.message("Saved file metadata is missing.") }
        let handle = try FileHandle(forReadingFrom: url); defer { try? handle.close() }
        let size = try handle.seekToEnd()
        guard let expectedSize = metadata["size"] as? NSNumber, expectedSize.uint64Value == size else { throw ClientError.message("Saved file size changed.") }
        let begin: [String: Any] = ["action":"begin", "id":item.id, "name":name, "mime_type":mime, "size":size]
        var state: UploadState = try await client.rpc(["op":"upload", "request":begin])
        guard state.offset <= size, state.chunk_bytes > 0, state.chunk_bytes <= 1024 * 1024 else { throw ClientError.message("Invalid upload response.") }
        progress.update(sent: Int64(state.offset), total: Int64(size))
        while state.offset < size {
            try Task.checkCancellation()
            guard try store.owns(item) else { throw CancellationError() }
            try handle.seek(toOffset: state.offset)
            guard let chunk = try handle.read(upToCount: state.chunk_bytes), !chunk.isEmpty else { throw ClientError.message("Saved file changed during upload.") }
            let expected = state.offset + UInt64(chunk.count)
            let next: UploadState = try await client.rpc(["op":"upload", "request":["action":"chunk", "id":item.id, "offset":state.offset, "data_base64":chunk.base64EncodedString()]])
            guard next.offset == expected, next.chunk_bytes == state.chunk_bytes else { throw ClientError.message("Server upload offset changed unexpectedly.") }
            state = next
            progress.update(sent: Int64(state.offset), total: Int64(size))
        }
        guard state.complete else { throw ClientError.message("Upload is incomplete.") }
        var payload = input.payload
        payload["file"] = ["artifact_id":item.id, "name":name, "mime_type":mime, "size":size]
        return ShareInput(mime: input.mime, label: input.label, payload: payload)
    }
}

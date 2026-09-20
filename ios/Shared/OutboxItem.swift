import Foundation
import CryptoKit

struct OutboxItem: Codable, Identifiable {
    enum State: String, Codable { case waiting, sending, paused, sent, cancelled, expired }
    let id: String
    let deviceID: String
    let serverPin: String
    let deviceKey: Data
    let serverURL: String
    let capability: Capability
    var chain: [Capability]? = nil
    let mime: String
    var label: String
    let payloadHash: Data
    var localFile: String? = nil
    var payload: Data?
    let created: Date
    let expires: Date
    var state: State = .waiting
    var attempts = 0
    var upload: UploadProgress?
    var nextAttempt: Date
    var lease: String?
    var leaseUntil: Date?
    var jobID: String?
    var detail: String?

    init(id: String = UUID().uuidString, input: ShareInput, capability: Capability, connection: Connection, now: Date, chain: [Capability]? = nil) throws {
        self.localFile = input.localFile?.lastPathComponent
        self.chain = chain
        if let chain { guard (2...16).contains(chain.count), chain.first == capability else { throw ClientError.message("Choose 2–16 chain steps.") } }
        self.id = id; deviceID = connection.deviceID; serverPin = connection.pin
        deviceKey = try Curve25519.Signing.PrivateKey(rawRepresentation: connection.privateKey).publicKey.rawRepresentation
        serverURL = connection.url; self.capability = capability; mime = input.mime
        label = String(input.label.prefix(200))
        payload = try JSONSerialization.data(withJSONObject: input.payload, options: [.sortedKeys])
        var digest = SHA256()
        digest.update(data: payload ?? Data())
        if let url = input.localFile {
            let handle = try FileHandle(forReadingFrom: url); defer { try? handle.close() }
            while let chunk = try handle.read(upToCount: 1024 * 1024), !chunk.isEmpty { digest.update(data: chunk) }
        }
        payloadHash = Data(digest.finalize())
        guard let payload, payload.count <= 7 * 1024 * 1024 else { throw ClientError.message("This share exceeds the Outbox item limit.") }
        guard capability.status == "active", capability.accepts(input.mime) else { throw ClientError.message("This target does not accept this content.") }
        created = now; expires = now.addingTimeInterval(7 * 86400); nextAttempt = now
    }
    func matches(_ connection: Connection) throws -> Bool {
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: connection.privateKey).publicKey.rawRepresentation
        return deviceID == connection.deviceID && serverPin == connection.pin && deviceKey == key
    }
    func input() throws -> ShareInput {
        guard let payload, let value = try JSONSerialization.jsonObject(with: payload) as? [String: Any] else { throw ClientError.message("The saved content is unavailable.") }
        return ShareInput(mime: mime, label: label, payload: value, localFile: try localFile.map(SharedFiles.resolve))
    }
    var statusLabel: String {
        switch state {
        case .waiting: return "Waiting for server"
        case .sending: return upload?.label ?? "Connecting to server"
        case .paused: return "Needs attention"
        case .sent: return "Accepted by server"
        case .cancelled: return "Retries stopped"
        case .expired: return "Expired"
        }
    }
    var targetTitle: String { chain?.map { $0.manifest.title }.joined(separator: " → ") ?? capability.manifest.title }
    var receipt: String { jobID ?? "outbox:\(id)" }
    var confirmation: String {
        switch state {
        case .sent: return "Accepted by xlatch. Check Activity for the result."
        case .paused: return "Saved on this iPhone. Open Outbox in xlatch to resolve a delivery issue."
        default: return "Saved on this iPhone—waiting for server. Open xlatch to retry; background delivery is best effort."
        }
    }
}

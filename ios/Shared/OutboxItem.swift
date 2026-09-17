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
    let mime: String
    var label: String
    let payloadHash: Data
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

    init(id: String = UUID().uuidString, input: ShareInput, capability: Capability, connection: Connection, now: Date) throws {
        self.id = id; deviceID = connection.deviceID; serverPin = connection.pin
        deviceKey = try Curve25519.Signing.PrivateKey(rawRepresentation: connection.privateKey).publicKey.rawRepresentation
        serverURL = connection.url; self.capability = capability; mime = input.mime
        label = String(input.label.prefix(200))
        payload = try JSONSerialization.data(withJSONObject: input.payload, options: [.sortedKeys])
        payloadHash = Data(SHA256.hash(data: payload ?? Data()))
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
        return ShareInput(mime: mime, label: label, payload: value)
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
    var receipt: String { jobID ?? "outbox:\(id)" }
    var confirmation: String {
        switch state {
        case .sent: return "Accepted by xlatch. Check Activity for the result."
        case .paused: return "Saved on this iPhone. Open Outbox in xlatch to resolve a delivery issue."
        default: return "Saved on this iPhone—waiting for server. Open xlatch to retry; background delivery is best effort."
        }
    }
}

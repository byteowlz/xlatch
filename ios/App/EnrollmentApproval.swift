import SwiftUI
import CryptoKit
import LocalAuthentication
import Security

struct EnrollmentStatus: Decodable {
    let server_id: String
    let enabled: Bool
    let is_approver: Bool
    let device_status: String
    let pending_payload: String?
}

struct EnrollmentReview: Decodable, Identifiable {
    let server_id: String
    let policy_version: Int
    let device_id: String
    let name: String
    let public_key: String
    let grants: [[String]]
    let nonce: String
    let expires_at: Int64
    var id: String { device_id }
    var expired: Bool { expires_at < Int64(Date().timeIntervalSince1970) }
}

struct PendingEnrollment: Identifiable {
    let payload: String
    let review: EnrollmentReview
    var id: String { review.id }
    var code: String { SHA256.hash(data: Data(payload.utf8)).prefix(6).map { String(format: "%02X", $0) }.joined() }
    init(_ payload: String, serverID: String) throws {
        let review = try JSONDecoder().decode(EnrollmentReview.self, from: Data(payload.utf8))
        guard review.server_id == serverID, review.policy_version == 1,
              review.grants.allSatisfy({ $0.count == 2 }), !review.grants.isEmpty else {
            throw ClientError.message("Enrollment request has an unsupported or incorrect server context.")
        }
        self.payload = payload; self.review = review
    }
    func signingBytes(approve: Bool) -> Data {
        Data("xlatch.enrollment.decision.v1\n\(approve ? "approve" : "reject")\n\(payload)".utf8)
    }
}

/// The share extension never receives this key. Each signature uses a fresh auth context.
enum ApprovalKey {
    static var deviceSupportsApprovals: Bool {
        #if targetEnvironment(simulator)
        false
        #else
        SecureEnclave.isAvailable
        #endif
    }
    private static func query(_ connection: Connection) throws -> [String: Any] {
        guard let group = Bundle.main.object(forInfoDictionaryKey: "XLatchApprovalKeychainGroup") as? String else {
            throw ClientError.message("Approval keychain is not configured in this build.")
        }
        return [kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: "com.byteowlz.xlatch.approval",
                kSecAttrAccount as String: "\(connection.pin):\(connection.deviceID)",
                kSecAttrAccessGroup as String: group]
    }
    private static func key(_ connection: Connection, create: Bool, reason: String) throws -> SecureEnclave.P256.Signing.PrivateKey {
        guard deviceSupportsApprovals else { throw ClientError.message("Approvals require a physical device with Secure Enclave and biometrics. Simulator approval is unavailable.") }
        let context = LAContext()
        context.localizedReason = reason
        context.touchIDAuthenticationAllowableReuseDuration = 0
        context.localizedFallbackTitle = ""
        var authError: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &authError) else {
            throw authError ?? NSError(domain: "xlatch", code: 1, userInfo: [NSLocalizedDescriptionKey: "Set up Face ID or Touch ID to approve devices."])
        }
        let base = try query(connection)
        var lookup = base; lookup[kSecReturnData as String] = true
        var result: CFTypeRef?
        let status = SecItemCopyMatching(lookup as CFDictionary, &result)
        if status == errSecSuccess, let data = result as? Data {
            return try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: data, authenticationContext: context)
        }
        guard status == errSecItemNotFound && create else { throw ClientError.message("The approval key is unavailable (\(status)). Re-pairing cannot replace an enrolled approver.") }
        var error: Unmanaged<CFError>?
        guard let access = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, [.privateKeyUsage, .biometryCurrentSet], &error) else {
            throw ClientError.message("Could not protect the approval key with biometrics.")
        }
        let key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: access, authenticationContext: context)
        var insert = base
        insert[kSecValueData as String] = key.dataRepresentation
        insert[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        let saved = SecItemAdd(insert as CFDictionary, nil)
        guard saved == errSecSuccess else { throw ClientError.message("Could not save the approval key (\(saved)).") }
        return key
    }
    static func enable(connection: Connection, serverID: String, token: String) async throws -> [String: String] {
        try await Task.detached {
            let key = try key(connection, create: true, reason: "Protect new device enrollment in xlatch")
            let publicKey = key.publicKey.x963Representation.base64EncodedString()
            let bytes = Data("xlatch.enrollment.enable.v1\n\(serverID)\n\(connection.deviceID)\n\(token)\n\(publicKey)".utf8)
            return ["public_key": publicKey, "signature": try key.signature(for: bytes).derRepresentation.base64EncodedString()]
        }.value
    }
    static func sign(connection: Connection, pending: PendingEnrollment, approve: Bool) async throws -> String {
        try await Task.detached {
            let key = try key(connection, create: false, reason: approve ? "Approve this device in xlatch" : "Reject this device in xlatch")
            return try key.signature(for: pending.signingBytes(approve: approve)).derRepresentation.base64EncodedString()
        }.value
    }
}

struct EnrollmentSettingsView: View {
    @EnvironmentObject var model: AppModel
    @State private var token = ""
    @State private var busy = false
    @State private var error: String?
    var body: some View {
        Form {
            if let status = model.enrollmentStatus {
                Section {
                    LabeledContent("New devices", value: status.enabled ? "Phone approval required" : "QR pairing")
                    LabeledContent("This phone", value: status.is_approver ? "Approver" : "Client")
                } footer: { Text("Enrollment approval does not protect against agents that can modify the server itself. Protected service installation is required for that boundary.") }
                if !status.enabled, let connection = model.connection {
                    Section("Enable phone approval") {
                        Text("On your server, run:")
                        Text("xlatch enrollment-bootstrap \(connection.deviceID)").font(.caption.monospaced()).textSelection(.enabled)
                        SecureField("Paste the one-time token", text: $token).autocorrectionDisabled().textInputAutocapitalization(.never)
                        Text("Keep this phone and its biometrics available. Changing enrolled biometrics invalidates its approval key. Recovery currently requires setting up a new server identity and pairing all devices again.").font(.footnote).foregroundStyle(.secondary)
                        Button("Enable with Face ID or Touch ID") { enable(status: status, connection: connection) }.disabled(busy || token.count != 64)
                    }
                }
                if status.is_approver {
                    Section("Awaiting your approval") {
                        if model.pendingEnrollments.isEmpty { Text("No pending devices").foregroundStyle(.secondary) }
                        ForEach(model.pendingEnrollments) { pending in
                            NavigationLink {
                                EnrollmentReviewView(pending: pending)
                            } label: {
                                VStack(alignment: .leading) { Text(pending.review.name); Text(pending.code).font(.caption.monospaced()).foregroundStyle(.secondary) }
                            }
                        }
                    }
                }
            }
            if let error { Section { Text(error).foregroundStyle(.red) } }
        }.navigationTitle("Device approvals")
    }
    private func enable(status: EnrollmentStatus, connection: Connection) {
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                let proof = try await ApprovalKey.enable(connection: connection, serverID: status.server_id, token: token)
                let client = try model.client()
                let _: EnrollmentStatus = try await client.rpc(["op": "enrollment", "request": ["action": "enable", "token": token, "public_key": proof["public_key"] ?? "", "signature": proof["signature"] ?? ""]])
                token = ""; await model.refresh()
            } catch { self.error = error.localizedDescription }
        }
    }
}

struct EnrollmentReviewView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let pending: PendingEnrollment
    @State private var busy = false
    @State private var error: String?
    @State private var codeMatches = false
    var body: some View {
        Form {
            Section("Device requesting access") {
                Text(pending.review.name)
                LabeledContent("Verification code", value: pending.code).font(.body.monospaced())
                Text("Compare this code with the code displayed on the new device. Device names are not proof of identity.").font(.footnote)
                Text(pending.review.public_key).font(.caption.monospaced()).textSelection(.enabled)
            }
            Section("Server identity") { Text(pending.review.server_id).font(.caption.monospaced()).textSelection(.enabled) }
            Section("Exact action grants") {
                ForEach(Array(pending.review.grants.enumerated()), id: \.offset) { _, grant in
                    VStack(alignment: .leading) { Text(grant[0]); Text(grant[1]).font(.caption2.monospaced()).foregroundStyle(.secondary) }
                }
            }
            Section {
                Text("Expires \(Date(timeIntervalSince1970: TimeInterval(pending.review.expires_at)).formatted())")
                Toggle("The codes match", isOn: $codeMatches)
                Button("Approve with Face ID or Touch ID") { decide(true) }.disabled(busy || !codeMatches || pending.review.expired)
                Button("Reject device", role: .destructive) { decide(false) }.disabled(busy || pending.review.expired)
                if let error { Text(error).foregroundStyle(.red) }
            }
        }.navigationTitle("Review device")
    }
    private func decide(_ approve: Bool) {
        guard let connection = model.connection else { return }
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                guard model.enrollmentStatus?.server_id == pending.review.server_id, !pending.review.expired else { throw ClientError.message("This enrollment request has expired or belongs to another server.") }
                let signature = try await ApprovalKey.sign(connection: connection, pending: pending, approve: approve)
                let _: JSONValue = try await model.client().rpc(["op": "enrollment", "request": ["action": "decide", "id": pending.id, "approve": approve, "signature": signature]])
                await model.refresh(); dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

struct PendingEnrollmentView: View {
    @EnvironmentObject var model: AppModel
    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Label(model.enrollmentStatus?.device_status == "pending" ? "Waiting for approval" : "Enrollment \(model.enrollmentStatus?.device_status ?? "pending")", systemImage: "lock.shield")
                    if let pending = model.ownPendingEnrollment {
                        LabeledContent("Verification code", value: pending.code).font(.body.monospaced())
                        Text("On your existing approver phone, open Server → Device approvals. Compare the codes before approving.")
                    }
                    if let error = model.error { Text(error).foregroundStyle(.red) }
                    Button("Check approval") { Task { await model.refresh() } }
                    Button("Cancel and forget this request", role: .destructive) { model.disconnect() }
                }
            }.navigationTitle("xlatch")
        }
    }
}

extension AppModel {
    func refreshEnrollment(using client: APIClient) async throws -> Bool {
            let status: EnrollmentStatus = try await client.rpc(["op": "enrollment", "request": ["action": "status"]])
            enrollmentStatus = status
            ownPendingEnrollment = try status.pending_payload.map { try PendingEnrollment($0, serverID: status.server_id) }
            if status.device_status != "active" {
                capabilities = []; jobs = []; pendingEnrollments = []
                activeServerURL = client.lastSuccessfulURL; error = nil; lastUpdated = Date()
                return false
            }
            if status.is_approver {
                let payloads: [String] = try await client.rpc(["op": "enrollment", "request": ["action": "pending"]])
                pendingEnrollments = try payloads.map { try PendingEnrollment($0, serverID: status.server_id) }
            } else { pendingEnrollments = [] }
        return true
    }
}

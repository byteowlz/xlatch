import SwiftUI
import CryptoKit

struct ApproverReview: Decodable, Identifiable {
    let id: String
    let server_id: String
    let operation: String
    let device_id: String
    let name: String
    let request_key: String
    let public_key: String
    let expires_at: Int64
}
struct PendingApprover: Identifiable {
    let payload: String
    let review: ApproverReview
    var id: String { review.id }
    var code: String { SHA256.hash(data: Data(payload.utf8)).prefix(6).map { String(format: "%02X", $0) }.joined() }
    init(_ payload: String, serverID: String) throws {
        review = try JSONDecoder().decode(ApproverReview.self, from: Data(payload.utf8))
        guard review.server_id == serverID, ["add", "remove"].contains(review.operation) else {
            throw ClientError.message("Unsupported approver review or different server identity.")
        }
        self.payload = payload
    }
    func signingBytes(approve: Bool) -> Data {
        Data("xlatch.approver.decision.v1\n\(approve ? "approve" : "reject")\n\(payload)".utf8)
    }
}
struct BackupApprover: Decodable, Identifiable {
    let id: String
    let name: String
    let public_key: String
}
struct BackupStatus: Decodable {
    let pending: [String]
    let approvers: [BackupApprover]
}
struct BackupApproversView: View {
    @EnvironmentObject var model: AppModel
    @State private var pending: [PendingApprover] = []
    @State private var approvers: [BackupApprover] = []
    @State private var error: String?
    @State private var busy = false
    var body: some View {
        Form {
            Section {
                Text("A backup phone can approve devices and actions, and revoke a lost approver. Set it up while your current approver still works.")
                Text("Pair the backup phone normally, then request approval authority here on that phone. On an existing approver, compare the verification codes and approve with Face ID or Touch ID.").foregroundStyle(.secondary)
                if model.enrollmentStatus?.is_approver == false {
                    Button("Request approval authority") { propose() }.disabled(busy)
                }
            }
            Section("Pending reviews") {
                if pending.isEmpty { Text("No pending approver changes").foregroundStyle(.secondary) }
                ForEach(pending) { item in
                    NavigationLink { ApproverReviewView(pending: item) } label: {
                        VStack(alignment: .leading) {
                            Text("\(item.review.operation == "add" ? "Add" : "Revoke") approver · \(item.review.name)")
                            Text(item.code).font(.caption.monospaced())
                        }
                    }
                }
            }
            if !approvers.isEmpty {
                Section("Trusted approvers") {
                    ForEach(approvers) { approver in
                        VStack(alignment: .leading) {
                            Text(approver.name)
                            Text(approver.id).font(.caption.monospaced()).textSelection(.enabled)
                            if approver.id != model.connection?.deviceID {
                                Button("Review revocation", role: .destructive) { remove(approver.id) }.disabled(busy)
                            }
                        }
                    }
                }
            }
            Section { Text("If every approver key is lost, there is no local bypass. Set up a new server identity and re-pair your devices.").font(.footnote) }
            if let error { Section { Text(error).foregroundStyle(.red) } }
        }.navigationTitle("Backup approvers")
            .refreshable { await refresh() }
            .task {
                while !Task.isCancelled {
                    await refresh()
                    do { try await Task.sleep(for: .seconds(3)) } catch { break }
                }
            }
    }
    private func refresh() async {
        do {
            guard let server = model.enrollmentStatus?.server_id else { return }
            let status: BackupStatus = try await model.client().rpc(["op":"enrollment", "request":["action":"recovery", "request":["action":"status"]]])
            pending = try status.pending.map { try PendingApprover($0, serverID: server) }
            approvers = status.approvers
        } catch { self.error = error.localizedDescription }
    }
    private func propose() {
        guard let connection = model.connection, let server = model.enrollmentStatus?.server_id else { return }
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                var proof = try await ApprovalKey.proposeBackup(connection: connection, serverID: server)
                proof["action"] = "propose"
                let _: String = try await model.client().rpc(["op":"enrollment", "request":["action":"recovery", "request":proof]])
                await refresh()
            } catch { self.error = error.localizedDescription }
        }
    }
    private func remove(_ id: String) {
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                let _: String = try await model.client().rpc(["op":"enrollment", "request":["action":"recovery", "request":["action":"remove", "device_id":id]]])
                await refresh()
            } catch { self.error = error.localizedDescription }
        }
    }
}
struct ApproverReviewView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let pending: PendingApprover
    @State private var checked = false
    @State private var busy = false
    @State private var error: String?
    var body: some View {
        Form {
            Section("Change to approval authority") {
                Text(pending.review.name)
                Text(pending.review.operation == "add" ? "This device will be able to approve actions, grants, new devices, and changes to approvers." : "This device will lose all access. Its outstanding jobs will be cancelled.")
                LabeledContent("Verification code", value: pending.code).font(.body.monospaced())
                Text(pending.review.public_key).font(.caption.monospaced()).textSelection(.enabled)
                Text("Expires \(Date(timeIntervalSince1970: TimeInterval(pending.review.expires_at)).formatted())").font(.caption)
            }
            if model.enrollmentStatus?.is_approver == true, pending.review.device_id != model.connection?.deviceID {
                Section {
                    Toggle(pending.review.operation == "add" ? "I compared this code with the backup phone" : "I verified the device to revoke", isOn: $checked)
                    Button("Approve with Face ID or Touch ID") { decide(true) }.disabled(!checked || busy)
                    Button("Reject change", role: .destructive) { decide(false) }.disabled(busy)
                }
            } else { Text("Show this code to an existing approver. This phone cannot approve its own promotion.") }
            if let error { Text(error).foregroundStyle(.red) }
        }.navigationTitle("Review approver")
    }
    private func decide(_ approve: Bool) {
        guard let connection = model.connection else { return }
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                guard pending.review.server_id == model.enrollmentStatus?.server_id,
                      pending.review.expires_at >= Int64(Date().timeIntervalSince1970) else { throw ClientError.message("Approver review expired or server changed.") }
                let signature = try await ApprovalKey.signBytes(connection: connection, bytes: pending.signingBytes(approve: approve), reason: "Authorize this change to xlatch approvers")
                let _: JSONValue = try await model.client().rpc(["op":"enrollment", "request":["action":"recovery", "request":["action":"decide", "id":pending.id, "approve":approve, "signature":signature]]])
                await model.refresh(); dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

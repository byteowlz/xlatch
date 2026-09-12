import SwiftUI
import CryptoKit

struct ApprovalTarget: Decodable, Identifiable {
    let id: String
    let name: String
    let public_key: String
}

struct ApprovalCatalog: Decodable {
    let capabilities: [Capability]
    let devices: [ApprovalTarget]
}

struct CapabilityReview: Decodable {
    let id: String
    let server_id: String
    let policy_version: Int
    let approver_id: String
    let manifest: JSONValue
    let revision: String
    let devices: [ApprovalTarget]
    let expires_at: Int64
    var expired: Bool { expires_at < Int64(Date().timeIntervalSince1970) }
}

struct PendingCapabilityApproval {
    let payload: String
    let review: CapabilityReview
    init(_ payload: String, connection: Connection) throws {
        let review = try JSONDecoder().decode(CapabilityReview.self, from: Data(payload.utf8))
        guard review.server_id == connection.serverID, review.approver_id == connection.deviceID,
              review.policy_version == 1, UUID(uuidString: review.id) != nil,
              review.revision.count == 64, review.revision.allSatisfy({ $0.isHexDigit }),
              review.manifest["id"]?.text != nil,
              let kind = review.manifest["execution"]?["kind"]?.text,
              ["echo", "save_file", "command"].contains(kind),
              Set(review.devices.map(\.id)).count == review.devices.count else {
            throw ClientError.message("Unsupported approval or incorrect server/device context.")
        }
        self.payload = payload; self.review = review
    }
    func signingBytes(approve: Bool) -> Data {
        Data("xlatch.capability.decision.v1\n\(approve ? "approve" : "reject")\n\(payload)".utf8)
    }
}

struct CapabilityApprovalListView: View {
    @EnvironmentObject var model: AppModel
    @State private var catalog: ApprovalCatalog?
    @State private var error: String?
    var body: some View {
        List {
            if let catalog {
                if catalog.capabilities.isEmpty { Text("No registered actions").foregroundStyle(.secondary) }
                ForEach(catalog.capabilities) { capability in
                    NavigationLink {
                        CapabilityApprovalSelectionView(capability: capability, devices: catalog.devices)
                    } label: {
                        VStack(alignment: .leading) {
                            Text(capability.manifest.title)
                            Text("\(capability.id) · \(capability.status)").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            if let error { Text(error).foregroundStyle(.red) }
        }
        .navigationTitle("Action approvals")
        .task { await refresh() }
        .refreshable { await refresh() }
    }
    private func refresh() async {
        do {
            catalog = try await model.client().rpc(["op": "approval", "request": ["action": "catalog"]])
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

struct CapabilityApprovalSelectionView: View {
    @EnvironmentObject var model: AppModel
    let capability: Capability
    let devices: [ApprovalTarget]
    @State private var selected = Set<String>()
    @State private var pending: PendingCapabilityApproval?
    @State private var busy = false
    @State private var error: String?
    var body: some View {
        Form {
            Section {
                Text(capability.manifest.title)
                Text(capability.manifest.description)
                Text(capability.revision).font(.caption.monospaced()).textSelection(.enabled)
            }
            Section {
                ForEach(devices) { device in
                    Toggle(isOn: Binding(get: { selected.contains(device.id) }, set: { enabled in
                        if enabled { selected.insert(device.id) } else { selected.remove(device.id) }
                    })) {
                        VStack(alignment: .leading) {
                            Text(device.name)
                            Text(device.id).font(.caption2.monospaced()).foregroundStyle(.secondary)
                        }
                    }
                }
            } header: { Text("Grant this revision to") }
              footer: { Text("Selected devices gain access to this revision. Other grants remain unchanged. Select none to activate the action without adding access.") }
            Section {
                Button("Review action and grants") { prepare() }.disabled(busy)
                if let error { Text(error).foregroundStyle(.red) }
            }
        }
        .navigationTitle("Choose access")
        .sheet(isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } })) {
            if let pending { NavigationStack { CapabilityApprovalReviewView(pending: pending) } }
        }
    }
    private func prepare() {
        guard let connection = model.connection else { return }
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                let payload: String = try await model.client().rpc(["op": "approval", "request": ["action": "prepare", "capability_id": capability.id, "revision": capability.revision, "devices": selected.sorted()]])
                let prepared = try PendingCapabilityApproval(payload, connection: connection)
                guard prepared.review.manifest["id"]?.text == capability.id,
                      prepared.review.revision == capability.revision,
                      Set(prepared.review.devices.map(\.id)) == selected else {
                    throw ClientError.message("The prepared review differs from your selection. Refresh and review again.")
                }
                pending = prepared
            } catch { self.error = error.localizedDescription }
        }
    }
}

struct CapabilityApprovalReviewView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let pending: PendingCapabilityApproval
    @State private var consent = false
    @State private var busy = false
    @State private var error: String?
    var body: some View {
        Form {
            Section("Action") {
                Text(pending.review.manifest["title"]?.text ?? "Action")
                Text(pending.review.manifest["description"]?.text ?? "")
                Text(pending.review.manifest["id"]?.text ?? "").font(.caption.monospaced())
                Text(pending.review.revision).font(.caption.monospaced()).textSelection(.enabled)
            }
            Section("Execution") {
                Text(pending.review.manifest["execution"]?.pretty ?? "").font(.caption.monospaced()).textSelection(.enabled)
                Text("Host actions have the execution account’s permissions. The executable hash does not freeze scripts, dependencies or other files it loads. Approval does not run the action.").font(.footnote).foregroundStyle(.secondary)
                Text("Timeout (seconds): \(pending.review.manifest["timeout_seconds"]?.pretty ?? "")")
            }
            Section("Input and output") {
                Text("Accepted content").font(.headline)
                Text(pending.review.manifest["accepts"]?.pretty ?? "").font(.caption.monospaced())
                DisclosureGroup("Input schema") { Text(pending.review.manifest["input_schema"]?.pretty ?? "").font(.caption.monospaced()).textSelection(.enabled) }
                DisclosureGroup("Output schema") { Text(pending.review.manifest["output_schema"]?.pretty ?? "").font(.caption.monospaced()).textSelection(.enabled) }
            }
            Section("Add access for these devices") {
                if pending.review.devices.isEmpty { Text("No new grants") }
                ForEach(pending.review.devices) { target in
                    VStack(alignment: .leading) {
                        Text(target.name)
                        Text(target.id).font(.caption.monospaced())
                        Text(target.public_key).font(.caption2.monospaced()).textSelection(.enabled)
                    }
                }
                Text("Other grants remain unchanged. Device names are labels, not proof of identity.").font(.footnote).foregroundStyle(.secondary)
            }
            Section("Approval") {
                Text("Server: \(pending.review.server_id)").font(.caption.monospaced()).textSelection(.enabled)
                Text("Expires \(Date(timeIntervalSince1970: TimeInterval(pending.review.expires_at)).formatted())")
                DisclosureGroup("Exact signed request") { Text(pending.payload).font(.caption2.monospaced()).textSelection(.enabled) }
                Toggle("I authorize this execution and these grants", isOn: $consent)
                Button("Approve with Face ID or Touch ID") { decide(true) }.disabled(busy || !consent || pending.review.expired)
                Button("Reject request", role: .destructive) { decide(false) }.disabled(busy || pending.review.expired)
                if let error { Text(error).foregroundStyle(.red) }
            }
        }
        .navigationTitle("Review approval")
        .interactiveDismissDisabled(busy)
        .toolbar { Button("Close") { dismiss() }.disabled(busy) }
    }
    private func decide(_ approve: Bool) {
        guard let connection = model.connection else { return }
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                guard connection.serverID == pending.review.server_id, connection.deviceID == pending.review.approver_id,
                      !pending.review.expired else { throw ClientError.message("Approval expired or connection changed. Review again.") }
                let signature = try await ApprovalKey.signCapability(connection: connection, pending: pending, approve: approve)
                let _: JSONValue = try await model.client().rpc(["op": "approval", "request": ["action": "decide", "id": pending.review.id, "approve": approve, "signature": signature]])
                await model.refresh(); dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

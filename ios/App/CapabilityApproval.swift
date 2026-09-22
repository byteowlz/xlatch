import SwiftUI
import CryptoKit

struct ApprovalTarget: Decodable, Identifiable, Equatable {
    let id: String
    let name: String
    let public_key: String
}

struct ApprovalCatalog: Decodable {
    let capabilities: [ApprovalAction]
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
              ["echo", "save_file", "command", "compose"].contains(kind),
              Set(review.devices.map(\.id)).count == review.devices.count else {
            throw ClientError.message("Unsupported approval or incorrect server/device context.")
        }
        self.payload = payload; self.review = review
    }
    func signingBytes(approve: Bool) -> Data {
        Data("xlatch.capability.decision.v1\n\(approve ? "approve" : "reject")\n\(payload)".utf8)
    }
}

struct ApprovalAction: Decodable, Identifiable {
    let manifest: JSONValue
    let revision: String
    let status: String
    var id: String { manifest["id"]?.text ?? "" }
    var title: String { manifest["title"]?.text ?? id }
    var description: String { manifest["description"]?.text ?? "" }
    var icon: ActionIcon? {
        guard let value = manifest["icon"], let data = try? JSONEncoder().encode(value) else { return nil }
        return try? JSONDecoder().decode(ActionIcon.self, from: data)
    }
    var scope: String {
        switch manifest["execution"]?["kind"]?.text {
        case "command": "Runs a command on your server"
        case "compose": "Runs a sequence of server actions"
        case "save_file": "Saves shared content on your server"
        default: "Processes shared content on your server"
        }
    }
}

extension PendingCapabilityApproval {
    func validate(action: ApprovalAction, targets: [ApprovalTarget]) throws {
        guard !review.expired, review.manifest == action.manifest,
              review.revision == action.revision,
              review.devices.sorted(by: { $0.id < $1.id }) == targets.sorted(by: { $0.id < $1.id }) else {
            throw ClientError.message("The action or device access changed. Refresh and approve again.")
        }
    }
}

@MainActor final class CapabilityApprovalController: ObservableObject {
    @Published var busy = false
    @Published var workingID: String?
    @Published var error: String?
    @Published var success: String?
    @Published var completed = Set<String>()

    func approve(_ action: ApprovalAction, targets: [ApprovalTarget], connection: Connection) async {
        guard !busy else { return }
        busy = true; workingID = action.id; error = nil; success = nil
        defer { busy = false; workingID = nil }
        do {
            let client = try APIClient(connection: connection)
            let payload: String = try await client.rpc(["op":"approval", "request":["action":"prepare", "capability_id":action.id, "revision":action.revision, "devices":targets.map(\.id).sorted()]])
            let pending = try PendingCapabilityApproval(payload, connection: connection)
            try pending.validate(action: action, targets: targets)
            let signature = try await ApprovalKey.signCapability(connection: connection, pending: pending, approve: true)
            guard !pending.review.expired else { throw ClientError.message("Approval expired. Tap approve to try again.") }
            let _: JSONValue = try await client.rpc(["op":"approval", "request":["action":"decide", "id":pending.review.id, "approve":true, "signature":signature]])
            completed.insert(action.id + ":" + action.revision)
            success = targets.isEmpty ? "\(action.title) approved. No device access added." : "\(action.title) approved for \(targets.count == 1 ? targets[0].name : "\(targets.count) devices")."
        } catch { self.error = error.localizedDescription }
    }
}

struct CapabilityApprovalListView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @StateObject private var approval = CapabilityApprovalController()
    @State private var catalog: ApprovalCatalog?
    @State private var error: String?
    @State private var loading = false
    var body: some View {
        List {
            if let success = approval.success {
                Section { Label(success, systemImage: "checkmark.circle.fill").foregroundStyle(.primary) }
            }
            if let error = approval.error ?? error {
                Section { Text(error).foregroundStyle(.red); Button("Refresh") { Task { await refresh() } }.disabled(approval.busy) }
            }
            if loading && catalog == nil { ProgressView("Checking approval access…") }
            if let catalog {
                Section {
                    let pending = catalog.capabilities.filter { $0.status != "active" }
                    if pending.isEmpty { Text("No actions awaiting approval").foregroundStyle(.secondary) }
                    ForEach(pending) { actionRow($0, devices: catalog.devices) }
                } header: { Text("Awaiting approval") }
                  footer: { Text("Tap approve, then use Face ID or Touch ID. This activates the action and allows this iPhone to use it. Approval does not run the action.") }
                Section("Active actions & access") {
                    ForEach(catalog.capabilities.filter { $0.status == "active" }) { actionRow($0, devices: catalog.devices) }
                }
            } else if !loading, let status = model.enrollmentStatus {
                Section("Approval access") {
                    Text(status.enabled ? "This phone is not an approver." : "Set up phone approval first.")
                    NavigationLink("Phone approval setup") { EnrollmentSettingsView() }
                }
            }
        }
        .navigationTitle("Action approvals")
        .task { await refresh() }
        .refreshable { await refresh() }
        .onChange(of: scenePhase) { _, phase in if phase == .active { Task { await refresh() } } }
    }
    private func actionRow(_ action: ApprovalAction, devices: [ApprovalTarget]) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            NavigationLink {
                CapabilityApprovalSelectionView(action: action, devices: devices, approval: approval)
            } label: {
                VStack(alignment: .leading, spacing: 4) {
                    HStack { CapabilityIcon(icon: action.icon); Text(action.title).font(.headline).foregroundStyle(.primary) }
                    Text(action.description).font(.subheadline).foregroundStyle(.secondary)
                    Text(action.scope).font(.caption).foregroundStyle(.secondary)
                }
            }.disabled(approval.busy)
            if approval.completed.contains(action.id + ":" + action.revision) {
                Label("Approved", systemImage: "checkmark.circle").foregroundStyle(.secondary)
            } else if let phone = devices.first(where: { $0.id == model.connection?.deviceID }) {
                Button {
                    guard let connection = model.connection else { return }
                    Task { await approval.approve(action, targets: [phone], connection: connection); await model.refresh(); await refresh() }
                } label: {
                    HStack {
                        if approval.workingID == action.id { ProgressView() } else { Image(systemName: "faceid") }
                        Text(action.status == "active" ? "Allow this iPhone" : "Approve for this iPhone")
                    }.frame(maxWidth: .infinity)
                }.buttonStyle(.borderedProminent).disabled(approval.busy)
            }
        }.padding(.vertical, 6)
    }
    private func refresh() async {
        guard !loading, !approval.busy else { return }
        loading = true; error = nil
        defer { loading = false }
        do {
            let client = try model.client()
            guard try await model.refreshEnrollment(using: client), model.enrollmentStatus?.is_approver == true else { catalog = nil; return }
            catalog = try await client.rpc(["op":"approval", "request":["action":"catalog"]])
        } catch { self.error = error.localizedDescription }
    }
}

struct CapabilityApprovalSelectionView: View {
    @EnvironmentObject var model: AppModel
    let action: ApprovalAction
    let devices: [ApprovalTarget]
    @ObservedObject var approval: CapabilityApprovalController
    @State private var selected = Set<String>()
    @State private var initialized = false
    @State private var approved = false
    var body: some View {
        Form {
            Section {
                Text(action.title).font(.title2.bold())
                Text(action.description)
                Label(action.scope, systemImage: "server.rack").font(.subheadline).foregroundStyle(.secondary)
                Text("Approval enables this action. It does not run it.").font(.footnote).foregroundStyle(.secondary)
            }
            Section {
                ForEach(devices) { device in
                    Toggle(isOn: Binding(get: { selected.contains(device.id) }, set: { enabled in
                        if enabled { selected.insert(device.id) } else { selected.remove(device.id) }
                        approved = false
                    })) {
                        VStack(alignment: .leading) {
                            Text(device.id == model.connection?.deviceID ? "This iPhone · \(device.name)" : device.name)
                            Text(device.id).font(.caption2.monospaced()).foregroundStyle(.secondary)
                        }
                    }.disabled(approval.busy)
                }
            } header: { Text("Allow access for") }
              footer: { Text("Selected devices gain access. Existing grants stay unchanged; these switches do not revoke access.") }
            Section {
                DisclosureGroup("Execution details") {
                    Text(action.manifest["execution"]?.pretty ?? "").font(.caption.monospaced()).textSelection(.enabled)
                    Text("Host commands run with the execution account’s permissions.").font(.footnote)
                }
                DisclosureGroup("Full action manifest") {
                    Text(action.manifest.pretty).font(.caption.monospaced()).textSelection(.enabled)
                    Text(action.revision).font(.caption2.monospaced()).textSelection(.enabled)
                }
            }
        }
        .navigationTitle("Approve action")
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 10) {
                if let error = approval.error { Text(error).font(.footnote).foregroundStyle(.red) }
                if approved, let success = approval.success { Label(success, systemImage: "checkmark.circle.fill").font(.footnote) }
                Text(selected.isEmpty ? "Activate only · no device access added" : "Allow access for \(selected.count) \(selected.count == 1 ? "device" : "devices")").font(.caption).foregroundStyle(.secondary)
                Button {
                    guard let connection = model.connection else { return }
                    let targets = devices.filter { selected.contains($0.id) }
                    Task {
                        await approval.approve(action, targets: targets, connection: connection)
                        approved = approval.success != nil
                        await model.refresh()
                    }
                } label: {
                    HStack {
                        if approval.busy { ProgressView() } else { Image(systemName: approved ? "checkmark" : "faceid") }
                        Text(approved ? "Approved" : "Approve with Face ID")
                    }.frame(maxWidth: .infinity).padding(.vertical, 6)
                }.buttonStyle(.borderedProminent).disabled(approval.busy || approved)
            }.padding().background(.bar)
        }
        .onAppear {
            guard !initialized else { return }
            initialized = true
            if let id = model.connection?.deviceID, devices.contains(where: { $0.id == id }) { selected = [id] }
        }
    }
}

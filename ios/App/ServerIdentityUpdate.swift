import SwiftUI
import LocalAuthentication

struct ServerIdentityUpdate: Decodable {
    let purpose: String
    let server_id: String
    let url: String
    let urls: [String]
    let pin: String
    func connection(replacing old: Connection) throws -> Connection {
        guard purpose == "xlatch.identity", old.serverID == server_id, pin != old.pin else {
            throw ClientError.message("This QR does not identify a new certificate for your existing server. Refresh the old connection before migrating it.")
        }
        let ticket = PairingTicket(version: 1, url: url, pin: pin, token: String(repeating: "0", count: 64), expires_at: Int64(Date().timeIntervalSince1970) + 60, urls: urls)
        try ticket.validate()
        return Connection(url: url, pin: pin, deviceID: old.deviceID, privateKey: old.privateKey, urls: urls, serverID: server_id, approvalKeyID: old.approvalKeyID ?? "\(old.pin):\(old.deviceID)")
    }
}

struct ServerIdentityUpdateView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var scanning = false
    @State private var update: ServerIdentityUpdate?
    @State private var error: String?
    @State private var approvedMigration = false
    @State private var busy = false
    var body: some View {
        Form {
            Section {
                Text("Use this only after an administrator-approved move to the protected service. It keeps your existing device and approval keys while trusting the server’s fresh TLS certificate.")
                Text("On the installed service, run xlatch identity with its control directory, then scan that QR here.").font(.footnote).foregroundStyle(.secondary)
                Button("Scan new server identity") { scanning = true }.disabled(busy)
            }
            if let update {
                Section("New certificate") {
                    Text(update.url)
                    Text(update.pin).font(.caption.monospaced()).textSelection(.enabled)
                    Toggle("I approved this server migration", isOn: $approvedMigration)
                    Button("Authenticate and trust this certificate") { apply(update) }.disabled(!approvedMigration || busy)
                }
            }
            if let error { Section { Text(error).foregroundStyle(.red) } }
        }.navigationTitle("Update identity")
            .sheet(isPresented: $scanning) { QRScanner { value in
                scanning = false; approvedMigration = false
                do {
                    let decoded = try JSONDecoder().decode(ServerIdentityUpdate.self, from: Data(value.utf8))
                    guard let old = model.connection else { throw ClientError.message("No paired server.") }
                    _ = try decoded.connection(replacing: old)
                    update = decoded; error = nil
                } catch { self.error = error.localizedDescription; update = nil }
            } }
    }
    private func apply(_ update: ServerIdentityUpdate) {
        guard let old = model.connection else { return }
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                let context = LAContext()
                guard try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "Trust the new certificate for your xlatch server") else { return }
                let connection = try update.connection(replacing: old)
                let client = try APIClient(connection: connection)
                let status: EnrollmentStatus = try await client.rpc(["op":"enrollment","request":["action":"status"]])
                guard status.server_id == update.server_id else { throw ClientError.message("The new server identity does not match.") }
                try CredentialStore.save(connection)
                model.connection = connection
                await model.refresh(); dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

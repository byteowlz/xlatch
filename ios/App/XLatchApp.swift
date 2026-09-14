import SwiftUI
import UserNotifications
import CoreSpotlight
import UniformTypeIdentifiers
import OSLog

@MainActor final class AppModel: ObservableObject {
    @Published var enrollmentStatus: EnrollmentStatus?
    @Published var pendingEnrollments: [PendingEnrollment] = []
    @Published var ownPendingEnrollment: PendingEnrollment?
    @Published var connection: Connection?
    @Published var capabilities: [Capability] = []
    @Published var jobs: [Job] = []
    @Published var error: String?
    @Published var refreshing = false
    @Published var lastUpdated: Date?
    @Published var activeServerURL: String?
    @Published var disabledActionIDs: Set<String> = []
    var enabledCapabilities: [Capability] { capabilities.filter { !disabledActionIDs.contains($0.id) } }
    func setAction(_ id: String, enabled: Bool) {
        guard let connection else { return }
        if enabled { disabledActionIDs.remove(id) } else { disabledActionIDs.insert(id) }
        ShareActionPreferences.save(disabledActionIDs, deviceID: connection.deviceID)
    }
    init() {
        do { connection = try CredentialStore.load(); capabilities = APIClient.cachedCapabilities(); if let connection { disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID) } }
        catch { self.error = error.localizedDescription }
    }
    func client() throws -> APIClient {
        guard let connection else { throw ClientError.message("Pair your server first.") }
        return try APIClient(connection: connection)
    }
    func refresh() async {
        guard connection != nil, !refreshing else { return }
        refreshing = true; defer { refreshing = false }
        do {
            let client = try client()
            guard try await refreshEnrollment(using: client) else { return }
            capabilities = try await client.capabilities()
            let recent: [Job] = try await client.rpc(["op": "jobs"])
            let previous = Dictionary(uniqueKeysWithValues: jobs.map { ($0.id, $0.status) })
            for job in recent where job.isFinished && ["queued", "running"].contains(previous[job.id] ?? "") {
                let content = UNMutableNotificationContent()
                content.title = job.status == "succeeded" ? "Your result is ready" : "Action \(job.statusLabel.lowercased())"
                content.body = capabilities.first(where: { $0.id == job.capability_id })?.manifest.title ?? job.capability_id
                content.sound = .default
                try? await UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: job.id, content: content, trigger: nil))
            }
            connection = try CredentialStore.load()
            jobs = recent; activeServerURL = client.lastSuccessfulURL; error = nil; lastUpdated = Date()
        } catch { self.error = error.localizedDescription }
    }
    func pair(_ ticket: PairingTicket) async throws {
        connection = try await APIClient.pair(ticket, name: UIDevice.current.name)
        enrollmentStatus = nil; pendingEnrollments = []; ownPendingEnrollment = nil
        capabilities = []; jobs = []; activeServerURL = nil
        if let connection { disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID) }
        await refresh()
    }
    func disconnect() {
        do { try CredentialStore.clear(); connection = nil; enrollmentStatus = nil; pendingEnrollments = []; ownPendingEnrollment = nil; capabilities = []; jobs = []; activeServerURL = nil; lastUpdated = nil; error = nil }
        catch { self.error = error.localizedDescription }
    }
}

@main struct XLatchApp: App {
    @StateObject private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase
    var body: some Scene {
        WindowGroup {
            Group {
                if model.connection == nil { PairView() }
                else if let status = model.enrollmentStatus, status.device_status != "active" { PendingEnrollmentView() }
                else {
                    TabView {
                        ActionsView().tabItem { Label("Actions", systemImage: "bolt") }
                        ActivityView().tabItem { Label("Activity", systemImage: "tray") }
                        SettingsView().tabItem { Label("Server", systemImage: "externaldrive.connected.to.line.below") }
                    }
                }
            }
            .environmentObject(model)
            .tint(Color(red: 0.12, green: 0.43, blue: 0.34))
            .onContinueUserActivity(CSSearchableItemActionType) { _ in
                Task { await model.refresh() }
            }
            .task {
                let attributes = CSSearchableItemAttributeSet(contentType: .text)
                attributes.title = "xlatch · CrossLatch"
                attributes.alternateNames = ["xlatch", "CrossLatch"]
                attributes.keywords = ["xlatch", "crosslatch"]
                attributes.contentDescription = "Open your shared actions and results."
                let item = CSSearchableItem(uniqueIdentifier: "open-xlatch", domainIdentifier: "com.byteowlz.xlatch", attributeSet: attributes)
                item.expirationDate = .distantFuture
                do { try await CSSearchableIndex.default().indexSearchableItems([item]) }
                catch { Logger(subsystem: "com.byteowlz.xlatch", category: "search").error("Could not index app search entry: \(error.localizedDescription)") }
            }
            .task {
                while !Task.isCancelled {
                    if scenePhase == .active { await model.refresh() }
                    do { try await Task.sleep(for: .seconds(3)) } catch { break }
                }
            }
        }
    }
}

struct PairView: View {
    @EnvironmentObject var model: AppModel
    @State private var scanning = false
    @State private var manual = false
    @State private var code = ""
    @State private var busy = false
    @State private var error: String?
    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 24) {
                Spacer(minLength: 20)
                Image(systemName: "point.3.connected.trianglepath.dotted").font(.system(size: 52, weight: .light)).foregroundStyle(.tint).accessibilityHidden(true)
                Text("Connect your server").font(.largeTitle.bold())
                Text("Send things from your iPhone to the tools on your machine. Your server decides which actions are available.")
                    .font(.title3).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Label("Paired with your device’s own key", systemImage: "lock.shield").font(.subheadline)
                if let error = error ?? model.error { Text(error).foregroundStyle(.red).font(.callout).accessibilityLabel("Error: \(error)") }
                Spacer(minLength: 20)
                Button { scanning = true } label: {
                    HStack { if busy { ProgressView() }; Label(busy ? "Connecting…" : "Scan pairing code", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity) }.padding(.vertical, 8)
                }.buttonStyle(.borderedProminent).disabled(busy)
                Button("Paste pairing code") { manual = true }.frame(maxWidth: .infinity).disabled(busy)
                Text("Create a pairing code with xlatch pair on your server. Codes expire after five minutes.")
                    .font(.footnote).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .center)
            }.padding(28).navigationTitle("xlatch").navigationBarTitleDisplayMode(.inline)
            .sheet(isPresented: $scanning) { QRScanner { value in scanning = false; connect(value) } }
            .sheet(isPresented: $manual) {
                NavigationStack {
                    Form { Section("Pairing code") { TextEditor(text: $code).font(.system(.body, design: .monospaced)).frame(minHeight: 160).autocorrectionDisabled().textInputAutocapitalization(.never) } }
                        .navigationTitle("Paste code").toolbar {
                            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { manual = false } }
                            ToolbarItem(placement: .confirmationAction) { Button("Connect") { manual = false; connect(code) }.disabled(code.isEmpty) }
                        }
                }
            }
        }
    }
    private func connect(_ value: String) {
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                let ticket = try JSONDecoder().decode(PairingTicket.self, from: Data(value.utf8))
                try ticket.validate()
                try await model.pair(ticket)
            } catch { self.error = error.localizedDescription }
        }
    }
}

struct ActionsView: View {
    @EnvironmentObject var model: AppModel
    var body: some View {
        NavigationStack {
            List {
                if let error = model.error { Section { Label(error, systemImage: "wifi.exclamationmark").foregroundStyle(.secondary) } }
                if model.enabledCapabilities.isEmpty {
                    ContentUnavailableView("No actions yet", systemImage: "bolt.slash", description: Text("Enable an action in Server settings, or grant this phone a new server action."))
                } else {
                    Section {
                        ForEach(model.enabledCapabilities) { capability in
                            NavigationLink { ComposeView(capability: capability) } label: {
                                VStack(alignment: .leading, spacing: 6) {
                                    Text(capability.manifest.title).font(.headline)
                                    Text(capability.manifest.description).font(.subheadline).foregroundStyle(.secondary)
                                    Text(capability.contentLabel).font(.caption).foregroundStyle(.secondary)
                                }.padding(.vertical, 6)
                            }
                        }
                    } footer: { Text("Also available in the share menu of other apps. Choose xlatch, then an action.") }
                }
            }.navigationTitle("Actions").refreshable { await model.refresh() }
        }
    }
}

struct ComposeView: View {
    @EnvironmentObject var model: AppModel
    let capability: Capability
    @State private var text = ""
    @State private var file: ShareInput?
    @State private var picking = false
    @State private var submitting = false
    @State private var error: String?
    @State private var submitted: Job?
    @State private var requestKey = UUID().uuidString
    var body: some View {
        Form {
            Section { Text(capability.manifest.description).foregroundStyle(.secondary) }
            Section("Content") {
                if let file {
                    Label(file.label, systemImage: "doc")
                    Button("Remove file", role: .destructive) { self.file = nil; requestKey = UUID().uuidString }
                } else {
                    TextEditor(text: $text).frame(minHeight: 150).accessibilityLabel("Text or prompt").onChange(of: text) { _, _ in requestKey = UUID().uuidString }
                    Button { picking = true } label: { Label("Choose a file", systemImage: "paperclip") }
                }
            }
            if let error { Section { Text(error).foregroundStyle(.red) } }
            Section {
                Button { submit() } label: { HStack { if submitting { ProgressView() }; Text(submitting ? "Sending…" : "Run action").frame(maxWidth: .infinity) } }
                    .disabled(submitting || (text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && file == nil))
            }
            if let submitted {
                Section { NavigationLink("View submitted job") { JobView(id: submitted.id) } } footer: { Text("Your server has accepted this job. You can leave this screen.") }
            }
        }.navigationTitle(capability.manifest.title)
            .fileImporter(isPresented: $picking, allowedContentTypes: [.item]) { result in
                do {
                    let url = try result.get(); let access = url.startAccessingSecurityScopedResource(); defer { if access { url.stopAccessingSecurityScopedResource() } }
                    let type = try url.resourceValues(forKeys: [.contentTypeKey]).contentType
                    let mime = type?.preferredMIMEType ?? "application/octet-stream"
                    guard capability.accepts(mime) else { throw ClientError.message("This action does not accept \(mime).") }
                    file = try ShareInput.file(at: url, mime: mime); requestKey = UUID().uuidString
                } catch { self.error = error.localizedDescription }
            }
    }
    private func submit() {
        submitting = true; error = nil
        Task {
            defer { submitting = false }
            do {
                let input = file ?? ShareInput.text(text, mime: capability.accepts("text/plain") ? "text/plain" : "text/uri-list")
                guard capability.accepts(input.mime) else { throw ClientError.message("Choose a file of a supported type for this action.") }
                submitted = try await model.client().invoke(capability, input: input, key: requestKey)
                await model.refresh()
            } catch { self.error = error.localizedDescription }
        }
    }
}

struct ActivityView: View {
    @EnvironmentObject var model: AppModel
    var body: some View {
        NavigationStack {
            List {
                if model.jobs.isEmpty { ContentUnavailableView("Nothing sent yet", systemImage: "tray", description: Text("Your submitted actions and their results will appear here.")) }
                ForEach(model.jobs) { job in
                    NavigationLink { JobView(id: job.id) } label: {
                        HStack(alignment: .top) {
                            VStack(alignment: .leading, spacing: 6) {
                                Text(model.capabilities.first(where: { $0.id == job.capability_id })?.manifest.title ?? job.capability_id).font(.headline)
                                Text(Date(timeIntervalSince1970: TimeInterval(job.created_at)), style: .relative).font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Text(job.statusLabel).font(.subheadline).foregroundStyle(job.status == "failed" ? Color.red : Color.secondary)
                        }.padding(.vertical, 4)
                    }
                }
            }.navigationTitle("Activity").refreshable { await model.refresh() }
        }
    }
}

struct JobView: View {
    @EnvironmentObject var model: AppModel
    let id: String
    @State private var job: Job?
    @State private var error: String?
    @State private var resultFile: URL?
    var body: some View {
        List {
            if let job {
                Section { LabeledContent("Status", value: job.statusLabel) }
                if let result = job.result {
                    Section("Result") {
                        if let resultFile {
                            Label(resultFile.lastPathComponent, systemImage: "doc")
                            if let image = UIImage(contentsOfFile: resultFile.path) { Image(uiImage: image).resizable().scaledToFit().accessibilityLabel("Generated result") }
                        } else if result["file"] == nil {
                            Text(result.text ?? result.pretty).textSelection(.enabled)
                        } else { Text("Preparing file…") }
                        if let resultFile { ShareLink(item: resultFile) { Label("Save or share file", systemImage: "square.and.arrow.up") } }
                        else { ShareLink(item: result.text ?? result.pretty) { Label("Share result", systemImage: "square.and.arrow.up") } }
                    }
                }
                if let error = job.error { Section("Could not complete") { Text(error).foregroundStyle(.red).textSelection(.enabled) } }
                if !job.isFinished {
                    Section { HStack { ProgressView(); Text("Your server is working on this.") }; Button("Cancel job", role: .destructive) { Task { await cancel() } } }
                }
            } else { ProgressView("Loading job…") }
            if let error { Text(error).foregroundStyle(.red) }
        }.navigationTitle("Job").task {
            repeat {
                await load()
                if job?.isFinished == true { break }
                do { try await Task.sleep(for: .seconds(2)) } catch { break }
            } while !Task.isCancelled
        }.refreshable { await load() }
    }
    private func load() async {
        do {
            job = try await model.client().rpc(["op": "job", "id": id]); error = nil
            if let artifact = job?.result?["file"], let encoded = artifact["data_base64"]?.text, let data = Data(base64Encoded: encoded), data.count <= 4 * 1024 * 1024 {
                let name = artifact["name"]?.text ?? "result.bin"
                let safeName = URL(fileURLWithPath: name).lastPathComponent
                let dir = FileManager.default.temporaryDirectory.appendingPathComponent(id, isDirectory: true)
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
                let url = dir.appendingPathComponent(safeName)
                try data.write(to: url, options: [.atomic, .completeFileProtection]); resultFile = url
            }
        } catch { self.error = error.localizedDescription }
    }
    private func cancel() async {
        do { job = try await model.client().rpc(["op": "cancel", "id": id]) }
        catch { self.error = error.localizedDescription }
    }
}

struct SettingsView: View {
    @EnvironmentObject var model: AppModel
    @State private var notificationStatus: String?
    @State private var confirmDisconnect = false
    private var addresses: [String] {
        guard let connection = model.connection else { return [] }
        return ([connection.url] + (connection.urls ?? [])).reduce(into: []) { result, address in
            if !result.contains(address) { result.append(address) }
        }
    }
    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("Address selection", value: "Automatic")
                    LabeledContent("Server verification", value: model.connection?.keyPin == nil ? "Paired certificate" : "Paired server key")
                    if let address = model.activeServerURL {
                        VStack(alignment: .leading, spacing: 4) {
                            Label(model.error == nil ? "Connected using" : "Last connected using", systemImage: model.error == nil ? "checkmark.circle.fill" : "clock")
                                .font(.subheadline).foregroundStyle(model.error == nil ? .green : .secondary)
                            Text(address).textSelection(.enabled)
                        }
                    } else {
                        Text(model.refreshing ? "Checking connection…" : "Connection not yet verified").foregroundStyle(.secondary)
                    }
                    if let date = model.lastUpdated { LabeledContent("Last reached") { Text(date, style: .time) } }
                    Button(model.refreshing ? "Checking…" : "Check connection now") { Task { await model.refresh() } }.disabled(model.refreshing)
                    if let error = model.error { Text(error).foregroundStyle(.red) }
                } header: { Text("Connection") } footer: {
                    Text("xlatch uses the first reachable address with your server’s verified identity. It switches automatically when your network changes; your tailnet or VPN must be connected to use its address.")
                }
                Section {
                    ForEach(addresses, id: \.self) { address in
                        HStack(alignment: .top) {
                            Text(address).textSelection(.enabled)
                            Spacer()
                            if address == model.activeServerURL && model.error == nil {
                                Text("In use").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                } header: { Text("Available server addresses") } footer: {
                    Text(addresses.count > 1 ? "All addresses were saved during pairing. The LAN address may be faster at home; another saved address can work when you’re away." : "Only one address was saved. Pair again with a server advertising both LAN and tailnet addresses to enable switching between them.")
                }
                Section {
                    ForEach(model.capabilities) { capability in
                        Toggle(isOn: Binding(get: { !model.disabledActionIDs.contains(capability.id) }, set: { model.setAction(capability.id, enabled: $0) })) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(capability.manifest.title)
                                Text(capability.manifest.description).font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    if model.capabilities.isEmpty { Text("No actions granted to this phone yet.").foregroundStyle(.secondary) }
                } header: { Text("Actions on this phone") } footer: {
                    Text("Enabled actions appear in the Actions tab and, for compatible content, in the share sheet. Turning one off does not revoke its server permission or affect other devices. New approved and granted actions appear automatically.")
                }
                Section("Notifications") {
                    Button("Enable result notifications") {
                        Task {
                            do { let allowed = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]); notificationStatus = allowed ? "Notifications enabled." : "Notifications are disabled in Settings." }
                            catch { notificationStatus = error.localizedDescription }
                        }
                    }
                    if let notificationStatus { Text(notificationStatus).font(.footnote) }
                    Text("This version checks results while the app is open and when you return. Background push notifications are not connected yet.").font(.footnote).foregroundStyle(.secondary)
                }
                Section { Button("Forget this server", role: .destructive) { confirmDisconnect = true } } footer: { Text("This removes the key from your phone. Use xlatch revoke on the server to revoke the device there too.") }
                Section {
                    NavigationLink("Device approvals") { EnrollmentSettingsView() }
                    if model.enrollmentStatus?.is_approver == true {
                        NavigationLink("Action approvals") { CapabilityApprovalListView() }
                    }
                    NavigationLink("Update server identity") { ServerIdentityUpdateView() }
                }
            }.navigationTitle("Server").confirmationDialog("Forget this server?", isPresented: $confirmDisconnect, titleVisibility: .visible) { Button("Forget server", role: .destructive) { model.disconnect() } }
        }
    }
}

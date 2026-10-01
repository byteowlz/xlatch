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
    @Published var parkedItems: [ParkedItem] = []
    @Published var error: String?
    @Published var refreshing = false
    @Published var lastUpdated: Date?
    @Published var activeServerURL: String?
    @Published var disabledActionIDs: Set<String> = []
    @Published private(set) var actionOrder: [String] = []
    @Published private(set) var actionIcons: [String: ActionIconOverride] = [:]
    var orderedCapabilities: [Capability] {
        guard let connection else { return capabilities }
        return ShareActionPreferences.ordered(capabilities, deviceID: connection.deviceID)
    }
    var enabledCapabilities: [Capability] { orderedCapabilities.filter { !disabledActionIDs.contains($0.id) } }
    func setAction(_ id: String, enabled: Bool) {
        guard let connection else { return }
        if enabled { disabledActionIDs.remove(id) } else { disabledActionIDs.insert(id) }
        ShareActionPreferences.save(disabledActionIDs, deviceID: connection.deviceID)
    }
    func moveActions(from source: IndexSet, to destination: Int) {
        guard let connection else { return }
        var ids = orderedCapabilities.map(\.id)
        ids.move(fromOffsets: source, toOffset: destination)
        actionOrder = ids
        ShareActionPreferences.saveOrder(ids, deviceID: connection.deviceID)
    }
    func iconOverride(for capabilityID: String) -> ActionIconOverride? {
        actionIcons[capabilityID]
    }
    func setIcon(_ icon: ActionIconOverride?, for capabilityID: String) {
        guard let connection else { return }
        if let icon { actionIcons[capabilityID] = icon } else { actionIcons.removeValue(forKey: capabilityID) }
        ShareActionPreferences.saveIcon(icon, for: capabilityID, deviceID: connection.deviceID)
    }
    private func loadActionPresentation() {
        guard let connection else { actionOrder = []; actionIcons = [:]; return }
        actionOrder = ShareActionPreferences.ordered(capabilities, deviceID: connection.deviceID).map(\.id)
        actionIcons = Dictionary(uniqueKeysWithValues: capabilities.compactMap { capability in
            ShareActionPreferences.icon(for: capability.id, deviceID: connection.deviceID).map { (capability.id, $0) }
        })
    }
    init() {
        do { connection = try CredentialStore.load(); capabilities = APIClient.cachedCapabilities(); if let connection { disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID) }; loadActionPresentation() }
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
            loadActionPresentation()
            parkedItems = try await client.rpc(["op": "parked"])
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
        capabilities = []; jobs = []; parkedItems = []; activeServerURL = nil
        if let connection { disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID) }
        loadActionPresentation()
        await refresh()
    }
    func disconnect() {
        do { try CredentialStore.clear(); connection = nil; enrollmentStatus = nil; pendingEnrollments = []; ownPendingEnrollment = nil; capabilities = []; jobs = []; parkedItems = []; activeServerURL = nil; lastUpdated = nil; error = nil; loadActionPresentation() }
        catch { self.error = error.localizedDescription }
    }
}

@main struct XLatchApp: App {
    @UIApplicationDelegateAdaptor(OutboxLifecycle.self) private var lifecycle
    @StateObject private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase
    var body: some Scene {
        WindowGroup {
            Group {
                if model.connection == nil {
                    TabView {
                        PairView().tabItem { Label("Connect", systemImage: "link") }
                        OutboxView().tabItem { Label("Outbox", systemImage: "tray.and.arrow.up") }
                    }
                }
                else if let status = model.enrollmentStatus, status.device_status != "active" {
                    TabView {
                        PendingEnrollmentView().tabItem { Label("Approval", systemImage: "person.badge.key") }
                        OutboxView().tabItem { Label("Outbox", systemImage: "tray.and.arrow.up") }
                    }
                }
                else {
                    TabView {
                        ActionsView().tabItem { Label("Actions", systemImage: "bolt") }
                        ParkedView().tabItem { Label("Later", systemImage: "bookmark") }
                        ActivityView().tabItem { Label("Activity", systemImage: "tray") }
                        SettingsView().tabItem { Label("Settings", systemImage: "gearshape") }
                    }
                }
            }
            .environmentObject(model)
            .onChange(of: scenePhase) { _, phase in
                if phase == .active { Task { await OutboxDelivery.shared.drain(expedite: true) } }
                else if phase == .background { OutboxBackground.schedule() }
            }
            .task { await OutboxDelivery.shared.drain(expedite: true) }
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
                    if scenePhase == .active {
                        await model.refresh()
                        await OutboxDelivery.shared.drain()
                    }
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
                Section {
                    NavigationLink { CapabilityApprovalListView() } label: {
                        Label("Review actions & access", systemImage: "checkmark.shield")
                    }
                } footer: { Text("Approve new server actions and choose which devices can use them.") }
                if let error = model.error { Section { Label(error, systemImage: "wifi.exclamationmark").foregroundStyle(.secondary) } }
                if model.enabledCapabilities.isEmpty {
                    ContentUnavailableView("No actions yet", systemImage: "bolt.slash", description: Text("Review new actions below, or enable an existing action in Settings."))
                } else {
                    Section {
                        ForEach(model.enabledCapabilities) { capability in
                            NavigationLink { ComposeView(capability: capability) } label: {
                                VStack(alignment: .leading, spacing: 6) {
                                    HStack { CapabilityIcon(icon: capability.manifest.icon, override: model.iconOverride(for: capability.id)); Text(capability.manifest.title).font(.headline) }
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
    @State private var submitted: OutboxItem?
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
            if submitting { Section { LiveUploadProgress(id: requestKey) } }
            if let submitted {
                Section {
                    if let jobID = submitted.jobID { NavigationLink("View submitted job") { JobView(id: jobID) } }
                    else { NavigationLink("View Outbox") { OutboxView() } }
                } footer: { Text(submitted.confirmation) }
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
                guard let connection = model.connection else { throw ClientError.message("Pair your server first.") }
                submitted = try await OutboxDelivery.shared.submit(input, capability: capability, connection: connection, id: requestKey)
                await model.refresh()
            } catch { self.error = error.localizedDescription }
        }
    }
}

struct ParkedView: View {
    @EnvironmentObject var model: AppModel
    var body: some View {
        NavigationStack {
            List {
                if let error = model.error { Text(error).foregroundStyle(.red) }
                if model.parkedItems.isEmpty {
                    ContentUnavailableView("Nothing saved for later", systemImage: "bookmark",
                        description: Text("Use Save for later in the xlatch share sheet, then choose a target here when you are ready."))
                }
                ForEach(model.parkedItems) { item in
                    NavigationLink { ParkedItemView(item: item) } label: {
                        HStack(spacing: 12) {
                            Image(systemName: parkedSymbol(item.mime_type)).foregroundStyle(.tint).frame(width: 28)
                            VStack(alignment: .leading, spacing: 4) {
                                Text(item.label).font(.headline).lineLimit(2)
                                HStack(spacing: 6) {
                                    Text(item.created, style: .relative)
                                    if let preparation = item.preparation {
                                        Label(preparation.status == "succeeded" ? "Prepared" : preparation.status.capitalized,
                                              systemImage: preparation.status == "succeeded" ? "checkmark.circle.fill" : "hourglass")
                                    }
                                }.font(.caption).foregroundStyle(.secondary)
                            }
                        }.padding(.vertical, 4)
                    }
                    .swipeActions {
                        Button(role: .destructive) { Task { await delete(item) } } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
                }
            }
            .navigationTitle("Later")
            .refreshable { await model.refresh() }
        }
    }
    private func delete(_ item: ParkedItem) async {
        do {
            let _: [String: String] = try await model.client().rpc(["op":"delete_parked", "id":item.id])
            model.parkedItems.removeAll { $0.id == item.id }
        } catch { model.error = error.localizedDescription }
    }
}

private func parkedSymbol(_ mime: String) -> String {
    if mime.hasPrefix("image/") { return "photo" }
    if mime.hasPrefix("audio/") { return "waveform" }
    if mime.hasPrefix("video/") { return "video" }
    if mime == "text/uri-list" { return "link" }
    if mime.hasPrefix("text/") { return "text.alignleft" }
    return "doc"
}

struct ParkedItemView: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject var model: AppModel
    let item: ParkedItem
    @State private var candidates: [Capability] = []
    @State private var loading = true
    @State private var sending: String?
    @State private var error: String?
    var body: some View {
        List {
            Section("Saved content") {
                Label(item.label, systemImage: parkedSymbol(item.mime_type)).lineLimit(4)
                LabeledContent("Type", value: item.mime_type)
                if let preparation = item.preparation {
                    LabeledContent("Preparation", value: preparation.status == "succeeded" ? "Ready" : preparation.status.capitalized)
                    if let failure = preparation.error { Text(failure).font(.footnote).foregroundStyle(.orange) }
                }
            }
            if let error { Section { Text(error).foregroundStyle(.red) } }
            if loading { Section { ProgressView("Finding compatible actions…") } }
            else if candidates.isEmpty {
                ContentUnavailableView("No compatible actions", systemImage: "bolt.slash",
                    description: Text("Enable or grant an action that accepts this content."))
            } else {
                Section("Send to") {
                    ForEach(orderedCandidates) { capability in
                        Button { Task { await dispatch(capability) } } label: {
                            HStack {
                                CapabilityIcon(icon: capability.manifest.icon, override: model.iconOverride(for: capability.id))
                                Text(capability.manifest.title).font(.headline).foregroundStyle(.primary)
                                Spacer()
                                if sending == capability.id { ProgressView() }
                                else { Image(systemName: "arrow.up.right").foregroundStyle(.tint) }
                            }.contentShape(Rectangle())
                        }.disabled(sending != nil)
                    }
                }
            }
        }
        .navigationTitle("Saved for later")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
    }
    private var orderedCandidates: [Capability] {
        guard let connection = model.connection else { return candidates }
        return ShareActionPreferences.ordered(candidates, deviceID: connection.deviceID)
    }
    private func load() async {
        loading = true; defer { loading = false }
        do {
            candidates = try await model.client().rpc(["op":"parked_candidates", "id":item.id])
            error = nil
        } catch { self.error = error.localizedDescription }
    }
    private func dispatch(_ capability: Capability) async {
        sending = capability.id; defer { sending = nil }
        do {
            let _: Job = try await model.client().rpc(["op":"dispatch_parked", "id":item.id,
                "capability_id":capability.id, "revision":capability.revision])
            model.parkedItems.removeAll { $0.id == item.id }
            await model.refresh(); dismiss()
        } catch { self.error = error.localizedDescription }
    }
}

struct ActivityView: View {
    @EnvironmentObject var model: AppModel
    @State private var saved: [OutboxItem] = []
    @State private var error: String?
    private var rows: [ActivityEntry] {
        ActivityEntry.merge(jobs: model.jobs, outbox: saved, connection: model.connection)
    }
    var body: some View {
        NavigationStack {
            List {
                if let error { Text(error).foregroundStyle(.red) }
                if let error = model.error { Text("Showing saved activity. " + error).foregroundStyle(.secondary) }
                if rows.isEmpty { ContentUnavailableView("Nothing sent yet", systemImage: "tray", description: Text("Queued shares, accepted jobs and completed results appear here.")) }
                ForEach(rows) { row in
                    NavigationLink {
                        if let id = row.jobID { JobView(id: id) }
                        else { OutboxView() }
                    } label: {
                        VStack(alignment: .leading, spacing: 5) {
                            HStack {
                                Text(row.title).font(.headline)
                                Spacer()
                                Text(row.status).font(.caption).foregroundStyle(row.needsAttention ? .orange : .secondary)
                            }
                            if let preview = row.preview { Text(preview).font(.subheadline).lineLimit(2) }
                            Text(row.server).font(.caption).foregroundStyle(.secondary)
                            Text(row.created, style: .relative).font(.caption2).foregroundStyle(.secondary)
                        }.padding(.vertical, 4)
                    }
                    .swipeActions(edge: .trailing, allowsFullSwipe: false) { deliveryActions(for: row) }
                    .contextMenu { deliveryActions(for: row) }
                }
                Section { NavigationLink("Manage queued shares") { OutboxView() } }
            }.navigationTitle("Activity")
                .refreshable { await model.refresh(); reload() }
                .task {
                    while !Task.isCancelled {
                        reload()
                        do { try await Task.sleep(for: .seconds(1)) } catch { break }
                    }
                }
        }
    }
    private func reload() {
        do { saved = try OutboxStore().items(); error = nil }
        catch { self.error = error.localizedDescription }
    }
    @ViewBuilder private func deliveryActions(for row: ActivityEntry) -> some View {
        if row.canStop, let id = row.outboxID {
            Button(role: .destructive) { edit { try $0.cancel(id) } } label: { Label("Stop retrying", systemImage: "stop.circle") }
        }
        if row.canRetry, let id = row.outboxID {
            Button { retry(id) } label: { Label("Retry now", systemImage: "arrow.clockwise") }.tint(.blue)
        }
    }
    private func edit(_ change: (OutboxStore) throws -> Void) {
        do { try change(OutboxStore()); reload(); error = nil }
        catch { self.error = error.localizedDescription }
    }
    private func retry(_ id: String) {
        edit { try $0.retry(id) }
        Task { await OutboxDelivery.shared.drain(id: id); reload() }
    }
}

struct ActivityEntry: Identifiable {
    let id: String
    let jobID: String?
    let title: String
    let status: String
    let server: String
    let preview: String?
    let created: Date
    let needsAttention: Bool
    let outboxID: String?
    let outboxState: OutboxItem.State?
    var canRetry: Bool { outboxState.map { [.waiting, .paused].contains($0) } ?? false }
    var canStop: Bool { outboxState.map { [.waiting, .sending, .paused].contains($0) } ?? false }

    static func merge(jobs: [Job], outbox: [OutboxItem], connection: Connection?) -> [ActivityEntry] {
        let local = outbox.filter { item in
            guard let connection else { return false }
            return (try? item.matches(connection)) == true
        }
        let receipts = Dictionary(local.compactMap { item in item.jobID.map { ($0, item) } }, uniquingKeysWith: { first, _ in first })
        let known = Set(jobs.map(\.id))
        let remote = jobs.map { job in
            let receipt = receipts[job.id]
            return ActivityEntry(id: job.id, jobID: job.id, title: receipt?.targetTitle ?? job.capability_id,
                status: job.status == "succeeded" ? "Completed" : job.statusLabel, server: receipt?.serverURL ?? connection?.url ?? "",
                preview: receipt?.label, created: Date(timeIntervalSince1970: TimeInterval(job.created_at)), needsAttention: job.status == "failed",
                outboxID: nil, outboxState: nil)
        }
        let pending = local.filter { !known.contains($0.jobID ?? "") }.map { item in
            ActivityEntry(id: "outbox:" + item.id, jobID: item.jobID, title: item.targetTitle,
                status: item.statusLabel, server: item.serverURL, preview: item.label, created: item.created,
                needsAttention: [.paused, .expired].contains(item.state), outboxID: item.id, outboxState: item.state)
        }
        return (remote + pending).sorted { $0.created == $1.created ? $0.id < $1.id : $0.created > $1.created }
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
                if let steps = job.steps, !steps.isEmpty {
                    Section("Steps") {
                        ForEach(steps) { step in
                            NavigationLink { JobView(id: step.job_id) } label: {
                                VStack(alignment: .leading) {
                                    Text("\(step.position + 1). \(step.capability_id)")
                                    Text(step.status.capitalized).font(.caption).foregroundStyle(.secondary)
                                    if let error = step.error { Text(error).font(.caption).foregroundStyle(.red) }
                                }
                            }
                        }
                    }
                }
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
                    ForEach(model.orderedCapabilities) { capability in
                        HStack(spacing: 12) {
                            NavigationLink { ActionAppearanceView(capability: capability) } label: {
                                HStack(spacing: 12) {
                                    CapabilityIcon(icon: capability.manifest.icon, override: model.iconOverride(for: capability.id))
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(capability.manifest.title)
                                        Text(capability.manifest.description).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                                    }
                                }
                            }
                            Toggle("Enable \(capability.manifest.title)", isOn: Binding(get: { !model.disabledActionIDs.contains(capability.id) }, set: { model.setAction(capability.id, enabled: $0) }))
                                .labelsHidden()
                        }
                    }.onMove(perform: model.moveActions)
                    if model.capabilities.isEmpty { Text("No actions granted to this phone yet.").foregroundStyle(.secondary) }
                } header: { Text("Actions on this phone") } footer: {
                    Text("Tap an action to change its icon. Use Edit to reorder actions in the app, share sheet, chains and Shortcuts. Turning one off does not revoke its server permission or affect other devices.")
                }
                if let connection = model.connection {
                    Section {
                        Picker("Prepare saved URLs with", selection: Binding(
                            get: { ShareActionPreferences.saveForLaterPreparation(deviceID: connection.deviceID) },
                            set: { ShareActionPreferences.saveForLaterPreparation($0, deviceID: connection.deviceID) }
                        )) {
                            Text("Nothing").tag(String?.none)
                            ForEach(model.orderedCapabilities.filter { $0.accepts("text/uri-list") }) { capability in
                                Text(capability.manifest.title).tag(Optional(capability.id))
                            }
                        }
                    } header: { Text("Save for Later") } footer: {
                        Text("Optional. When you save a URL, xlatch runs the selected granted action in the background and keeps any successful typed result with the original item. Other content is saved unchanged.")
                    }
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
                    NavigationLink("Shortcuts & Back Tap") { ShortcutSettingsView() }
                    NavigationLink("Device approvals") { EnrollmentSettingsView() }
                    if model.enrollmentStatus?.is_approver == true { NavigationLink("Devices & aliases") { DeviceManagementView() } }
                    NavigationLink("Action approvals & access") { CapabilityApprovalListView() }
                    NavigationLink("Update server identity") { ServerIdentityUpdateView() }
                }
            }.navigationTitle("Settings").toolbar { EditButton() }
                .confirmationDialog("Forget this server?", isPresented: $confirmDisconnect, titleVisibility: .visible) { Button("Forget server", role: .destructive) { model.disconnect() } }
        }
    }
}

private struct ActionAppearanceView: View {
    @EnvironmentObject private var model: AppModel
    let capability: Capability
    @State private var choosingImage = false
    @State private var error: String?
    private let symbols = ["bolt.fill", "paperplane.fill", "tray.and.arrow.down.fill", "link", "text.bubble.fill", "waveform", "photo.fill", "film.fill", "doc.fill", "wand.and.stars", "terminal.fill", "gearshape.fill"]
    private var selected: ActionIconOverride? { model.iconOverride(for: capability.id) }
    var body: some View {
        Form {
            Section {
                HStack(spacing: 16) {
                    CapabilityIcon(icon: capability.manifest.icon, override: selected, size: 48)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(capability.manifest.title).font(.headline)
                        Text(selected == nil ? "Using server icon" : "Using icon on this iPhone").font(.subheadline).foregroundStyle(.secondary)
                    }
                }.padding(.vertical, 4)
            }
            Section("Symbols") {
                LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 4), spacing: 16) {
                    ForEach(symbols, id: \.self) { symbol in
                        Button { model.setIcon(.system(symbol), for: capability.id) } label: {
                            Image(systemName: symbol).font(.title2).frame(width: 44, height: 44)
                                .background(selected?.systemName == symbol ? Color.accentColor.opacity(0.16) : Color.clear, in: RoundedRectangle(cornerRadius: 10))
                        }.buttonStyle(.plain).accessibilityLabel("Use \(symbol) icon")
                    }
                }.padding(.vertical, 6)
            }
            Section {
                Button { choosingImage = true } label: { Label("Choose image…", systemImage: "photo") }
                if selected != nil { Button("Use server icon") { model.setIcon(nil, for: capability.id) } }
                if let error { Text(error).foregroundStyle(.red) }
            } footer: { Text("This choice is stored only on this iPhone. The action’s server-provided icon remains unchanged.") }
        }.navigationTitle("Action icon").navigationBarTitleDisplayMode(.inline)
            .fileImporter(isPresented: $choosingImage, allowedContentTypes: [.image]) { result in
                Task { await importImage(result) }
            }
    }
    @MainActor private func importImage(_ result: Result<URL, Error>) async {
        do {
            let url = try result.get()
            let accessed = url.startAccessingSecurityScopedResource()
            defer { if accessed { url.stopAccessingSecurityScopedResource() } }
            let data = try Data(contentsOf: url, options: .mappedIfSafe)
            model.setIcon(.custom(try ActionIcon.imported(data)), for: capability.id)
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

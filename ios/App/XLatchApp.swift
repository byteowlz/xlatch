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
    @Published var connections: [Connection] = []
    @Published var capabilities: [Capability] = []
    @Published var catalogs: [String: [Capability]] = [:]
    @Published var catalogErrors: [String: String] = [:]
    @Published var jobs: [Job] = []
    @Published var parkedItems: [ParkedItem] = []
    @Published var error: String?
    @Published var refreshing = false
    @Published var lastUpdated: Date?
    @Published var activeServerURL: String?
    @Published var disabledActionIDs: Set<String> = []
    @Published private(set) var actionIcons: [String: ActionIconOverride] = [:]
    var allTargets: [ShareTarget] {
        ShareActionPreferences.orderedTargets(connections.flatMap { server in
            ShareActionPreferences.ordered(catalogs[server.id] ?? APIClient.cachedCapabilities(for: server), deviceID: server.deviceID)
                .map { ShareTarget(capability: $0, connection: server) }
        })
    }
    func moveTargets(from source: IndexSet, to destination: Int) {
        var ids = allTargets.map(\.id)
        ids.move(fromOffsets: source, toOffset: destination)
        ShareActionPreferences.saveTargetOrder(ids)
        objectWillChange.send()
    }
    func setTarget(_ target: ShareTarget, enabled: Bool) {
        var disabled = ShareActionPreferences.disabled(deviceID: target.connection.deviceID)
        if enabled { disabled.remove(target.capability.id) } else { disabled.insert(target.capability.id) }
        ShareActionPreferences.save(disabled, deviceID: target.connection.deviceID)
        if target.connection.deviceID == connection?.deviceID { disabledActionIDs = disabled }
        objectWillChange.send()
    }
    func refreshCatalogs() async {
        await withTaskGroup(of: (String, [Capability]?, String?).self) { group in
            for server in connections {
                group.addTask {
                    do { return (server.id, try await APIClient(connection: server).capabilities(), nil) }
                    catch { return (server.id, nil, error.localizedDescription) }
                }
            }
            for await (id, actions, failure) in group {
                if let actions { catalogs[id] = actions }
                catalogErrors[id] = failure
            }
        }
    }
    var orderedCapabilities: [Capability] {
        guard let connection else { return capabilities }
        return ShareActionPreferences.ordered(capabilities, deviceID: connection.deviceID)
    }
    var enabledCapabilities: [Capability] { orderedCapabilities.filter { !disabledActionIDs.contains($0.id) } }
    func iconOverride(for capabilityID: String) -> ActionIconOverride? {
        actionIcons[capabilityID]
    }
    func setIcon(_ icon: ActionIconOverride?, for capabilityID: String) {
        guard let connection else { return }
        if let icon { actionIcons[capabilityID] = icon } else { actionIcons.removeValue(forKey: capabilityID) }
        ShareActionPreferences.saveIcon(icon, for: capabilityID, deviceID: connection.deviceID)
    }
    private func loadActionPresentation() {
        guard let connection else { actionIcons = [:]; return }
        actionIcons = Dictionary(uniqueKeysWithValues: capabilities.compactMap { capability in
            ShareActionPreferences.icon(for: capability.id, deviceID: connection.deviceID).map { (capability.id, $0) }
        })
    }
    init() {
        do { connections = try CredentialStore.loadAll(); connection = try CredentialStore.load(); capabilities = APIClient.cachedCapabilities(for: connection); if let connection { disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID) }; loadActionPresentation() }
        catch { self.error = error.localizedDescription }
    }
    func client() throws -> APIClient {
        guard let connection else { throw ClientError.message("Pair your server first.") }
        return try APIClient(connection: connection)
    }
    func resendCandidates(for jobID: String) async throws -> [Capability] {
        try await client().rpc(["op": "job_candidates", "id": jobID])
    }
    func resend(_ jobID: String, to capability: Capability, idempotencyKey: String) async throws -> Job {
        try await client().rpc([
            "op": "resend_job", "id": jobID,
            "capability_id": capability.id, "revision": capability.revision,
            "idempotency_key": idempotencyKey
        ])
    }
    func refresh() async {
        guard let expectedConnection = connection, !refreshing else { return }
        let expectedID = expectedConnection.deviceID
        refreshing = true; defer { refreshing = false }
        do {
            let client = try APIClient(connection: expectedConnection)
            guard try await refreshEnrollment(using: client) else { return }
            let refreshedCapabilities = try await client.capabilities()
            let refreshedParked: [ParkedItem] = try await client.rpc(["op": "parked"])
            let recent: [Job] = try await client.rpc(["op": "jobs"])
            guard connection?.deviceID == expectedID else { return }
            capabilities = refreshedCapabilities
            loadActionPresentation()
            parkedItems = refreshedParked
            let previous = Dictionary(uniqueKeysWithValues: jobs.map { ($0.id, $0.status) })
            for job in recent where job.isFinished && ["queued", "running"].contains(previous[job.id] ?? "") {
                let content = UNMutableNotificationContent()
                content.title = job.status == "succeeded" ? "Your result is ready" : "Action \(job.statusLabel.lowercased())"
                content.body = capabilities.first(where: { $0.id == job.capability_id })?.manifest.title ?? job.capability_id
                content.sound = .default
                try? await UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: job.id, content: content, trigger: nil))
            }
            connections = try CredentialStore.loadAll(); connection = try CredentialStore.load()
            if let connection { catalogs[connection.id] = refreshedCapabilities }
            jobs = recent; activeServerURL = client.lastSuccessfulURL; error = nil; lastUpdated = Date()
        } catch { self.error = error.localizedDescription }
    }
    func pair(_ ticket: PairingTicket) async throws {
        connection = try await APIClient.pair(ticket, name: UIDevice.current.name)
        connections = try CredentialStore.loadAll()
        enrollmentStatus = nil; pendingEnrollments = []; ownPendingEnrollment = nil
        capabilities = []; jobs = []; parkedItems = []; activeServerURL = nil
        if let connection { disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID) }
        loadActionPresentation()
        await refresh()
    }
    func selectServer(_ id: String) async {
        guard connection?.id != id else { return }
        do {
            try CredentialStore.select(id)
            connections = try CredentialStore.loadAll(); connection = try CredentialStore.load()
            enrollmentStatus = nil; pendingEnrollments = []; ownPendingEnrollment = nil
            capabilities = APIClient.cachedCapabilities(for: connection); jobs = []; parkedItems = []; activeServerURL = nil; lastUpdated = nil; error = nil
            if let connection { disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID) }
            loadActionPresentation(); await refresh()
        } catch { self.error = error.localizedDescription }
    }
    func disconnect() {
        do {
            try CredentialStore.clear(); connections = try CredentialStore.loadAll(); connection = try CredentialStore.load()
            enrollmentStatus = nil; pendingEnrollments = []; ownPendingEnrollment = nil; capabilities = APIClient.cachedCapabilities(for: connection); jobs = []; parkedItems = []; activeServerURL = nil; lastUpdated = nil; error = nil; loadActionPresentation()
            if connection != nil { Task { await refresh() } }
        }
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
                    if scenePhase == .active { await model.refreshCatalogs() }
                    do { try await Task.sleep(for: .seconds(30)) } catch { break }
                }
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
                    ForEach(model.connections, id: \.id) { server in
                        NavigationLink { ServerActionApprovalView(serverID: server.id) } label: {
                            Label("Review actions · \(ServerPresentation.label(for: server))", systemImage: "checkmark.shield")
                        }
                    }
                } footer: { Text("Approve new actions on their server and choose which devices can use them.") }
                if let error = model.error { Section { Label(error, systemImage: "wifi.exclamationmark").foregroundStyle(.secondary) } }
                if model.allTargets.filter({ !ShareActionPreferences.disabled(deviceID: $0.connection.deviceID).contains($0.capability.id) }).isEmpty {
                    ContentUnavailableView("No actions yet", systemImage: "bolt.slash", description: Text("Review new actions below, or enable an existing action in Settings."))
                } else {
                    Section {
                        ForEach(model.allTargets.filter { !ShareActionPreferences.disabled(deviceID: $0.connection.deviceID).contains($0.capability.id) }) { target in
                            NavigationLink { ComposeView(capability: target.capability, server: target.connection) } label: {
                                HStack(spacing: 12) {
                                    CapabilityIcon(icon: target.capability.manifest.icon, override: ShareActionPreferences.icon(for: target.capability.id, deviceID: target.connection.deviceID))
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(target.capability.manifest.title).font(.headline)
                                        Text(ServerPresentation.label(for: target.connection)).font(.caption).foregroundStyle(.secondary)
                                    }
                                }.padding(.vertical, 6)
                            }
                        }
                    } footer: { Text("Also available in the share menu of other apps. Choose xlatch, then an action.") }
                }
            }.navigationTitle("Actions").refreshable { await model.refreshCatalogs() }
        }
    }
}

private struct ServerActionApprovalView: View {
    @EnvironmentObject var model: AppModel
    let serverID: String
    @State private var ready = false
    var body: some View {
        Group {
            if ready { CapabilityApprovalListView() }
            else { ProgressView("Loading server…") }
        }.task {
            let deviceID = model.connections.first(where: { $0.id == serverID })?.deviceID
            await model.selectServer(serverID)
            ready = deviceID != nil && model.connection?.deviceID == deviceID
        }
    }
}

struct ComposeView: View {
    @EnvironmentObject var model: AppModel
    let capability: Capability
    var server: Connection? = nil
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
                guard let connection = server ?? model.connection else { throw ClientError.message("Pair your server first.") }
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
    @State private var resendSelection: ResendSelection?
    @State private var openedJob: OpenedJob?
    @State private var retryingJobID: String?
    @State private var retryKeys: [String: String] = [:]
    private var rows: [ActivityEntry] {
        ActivityEntry.merge(jobs: model.jobs, outbox: saved, connection: model.connection, capabilities: model.capabilities)
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
                .navigationDestination(item: $openedJob) { selection in JobView(id: selection.id) }
                .sheet(item: $resendSelection) { selection in
                    NavigationStack { ResendJobView(id: selection.id) }
                        .environmentObject(model)
                }
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
        if row.jobStatus == "failed", row.jobID != nil {
            Button { Task { await retry(job: row) } } label: {
                Label(retryingJobID == row.jobID ? "Retrying…" : "Retry action", systemImage: "arrow.clockwise")
            }.tint(.blue).disabled(retryingJobID != nil)
        }
        if let id = row.jobID, row.jobStatus.map({ ["succeeded", "failed", "cancelled"].contains($0) }) == true {
            Button { resendSelection = ResendSelection(id: id) } label: {
                Label("Send again…", systemImage: "arrowshape.turn.up.right")
            }.tint(.teal)
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
    private func retry(job row: ActivityEntry) async {
        guard let id = row.jobID, let capabilityID = row.capabilityID else { return }
        retryingJobID = id; defer { retryingJobID = nil }
        do {
            let candidates = try await model.resendCandidates(for: id)
            guard let capability = candidates.first(where: { $0.id == capabilityID }) else {
                throw ClientError.message("This action is no longer available or granted. Choose Send again to select another action.")
            }
            let key = retryKeys[id] ?? UUID().uuidString
            retryKeys[id] = key
            let resent = try await model.resend(id, to: capability, idempotencyKey: key)
            retryKeys.removeValue(forKey: id)
            await model.refresh()
            openedJob = OpenedJob(id: resent.id)
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

private struct ResendSelection: Identifiable { let id: String }
private struct OpenedJob: Identifiable, Hashable { let id: String }

struct ActivityEntry: Identifiable {
    let id: String
    let jobID: String?
    let capabilityID: String?
    let jobStatus: String?
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

    static func merge(jobs: [Job], outbox: [OutboxItem], connection: Connection?, capabilities: [Capability]) -> [ActivityEntry] {
        let local = outbox.filter { item in
            guard let connection else { return false }
            return (try? item.matches(connection)) == true
        }
        let receipts = Dictionary(local.compactMap { item in item.jobID.map { ($0, item) } }, uniquingKeysWith: { first, _ in first })
        let known = Set(jobs.map(\.id))
        let remote = jobs.map { job in
            let receipt = receipts[job.id]
            let title = receipt?.targetTitle ?? capabilities.first(where: { $0.id == job.capability_id })?.manifest.title ?? job.capability_id
            return ActivityEntry(id: job.id, jobID: job.id, capabilityID: job.capability_id, jobStatus: job.status, title: title,
                status: job.status == "succeeded" ? "Completed" : job.statusLabel, server: receipt?.serverURL ?? connection?.url ?? "",
                preview: receipt?.label, created: Date(timeIntervalSince1970: TimeInterval(job.created_at)), needsAttention: job.status == "failed",
                outboxID: nil, outboxState: nil)
        }
        let pending = local.filter { !known.contains($0.jobID ?? "") }.map { item in
            ActivityEntry(id: "outbox:" + item.id, jobID: item.jobID, capabilityID: nil, jobStatus: nil, title: item.targetTitle,
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
    @State private var retrying = false
    @State private var resentJob: Job?
    @State private var retryKey = UUID().uuidString
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
                } else {
                    Section("Run again") {
                        if job.status == "failed" {
                            Button { Task { await retry(job) } } label: {
                                if retrying { HStack { ProgressView(); Text("Retrying…") } }
                                else { Label("Retry action", systemImage: "arrow.clockwise") }
                            }.disabled(retrying)
                        }
                        NavigationLink { ResendJobView(id: job.id) } label: {
                            Label("Send to another action…", systemImage: "arrowshape.turn.up.right")
                        }
                        if let resentJob {
                            NavigationLink { JobView(id: resentJob.id) } label: {
                                Label("View retried job", systemImage: "arrow.up.right.circle.fill")
                            }
                        }
                    }
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
    private func retry(_ source: Job) async {
        retrying = true; defer { retrying = false }
        do {
            let candidates = try await model.resendCandidates(for: source.id)
            guard let capability = candidates.first(where: { $0.id == source.capability_id }) else {
                throw ClientError.message("This action is no longer available or granted. Choose another action below.")
            }
            resentJob = try await model.resend(source.id, to: capability, idempotencyKey: retryKey)
            retryKey = UUID().uuidString
            await model.refresh()
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

private struct ResendJobView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let id: String
    @State private var candidates: [Capability] = []
    @State private var loading = true
    @State private var sending: String?
    @State private var submitted: Job?
    @State private var error: String?
    @State private var sendKeys: [String: String] = [:]

    var body: some View {
        List {
            if let submitted {
                Section {
                    Label("Sent again", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                    NavigationLink { JobView(id: submitted.id) } label: {
                        Label("View new job", systemImage: "arrow.up.right.circle")
                    }
                }
            } else if loading {
                ProgressView("Finding compatible actions…")
            } else if candidates.isEmpty {
                ContentUnavailableView("No compatible actions", systemImage: "bolt.slash",
                    description: Text("The original content may no longer match a granted action."))
            } else {
                Section("Send to") {
                    ForEach(orderedCandidates) { capability in
                        Button { Task { await send(to: capability) } } label: {
                            HStack(spacing: 12) {
                                CapabilityIcon(icon: capability.manifest.icon, override: model.iconOverride(for: capability.id))
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(capability.manifest.title).font(.headline).foregroundStyle(.primary)
                                    Text(capability.contentLabel).font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                if sending == capability.id { ProgressView() }
                                else { Image(systemName: "arrow.up.right").foregroundStyle(.tint) }
                            }.contentShape(Rectangle())
                        }.disabled(sending != nil)
                    }
                }
            }
            if let error { Section { Text(error).foregroundStyle(.red).textSelection(.enabled) } }
        }
        .navigationTitle("Send again")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } } }
        .task { await load() }
    }

    private var orderedCandidates: [Capability] {
        guard let connection = model.connection else { return candidates }
        return ShareActionPreferences.ordered(candidates, deviceID: connection.deviceID)
    }
    private func load() async {
        loading = true; defer { loading = false }
        do { candidates = try await model.resendCandidates(for: id); error = nil }
        catch { self.error = error.localizedDescription }
    }
    private func send(to capability: Capability) async {
        sending = capability.id; defer { sending = nil }
        do {
            let key = sendKeys[capability.id] ?? UUID().uuidString
            sendKeys[capability.id] = key
            submitted = try await model.resend(id, to: capability, idempotencyKey: key)
            sendKeys.removeValue(forKey: capability.id)
            await model.refresh()
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

struct SettingsView: View {
    @EnvironmentObject var model: AppModel
    @State private var filter: String?
    private var targets: [ShareTarget] { model.allTargets.filter { filter == nil || $0.connection.id == filter } }
    var body: some View {
        NavigationStack {
            Form {
                Section("Servers") {
                    ForEach(model.connections, id: \.id) { server in
                        NavigationLink { ServerSettingsView(server: server) } label: {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(ServerPresentation.label(for: server))
                                Text(server.displayName).font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    NavigationLink { PairView() } label: { Label("Add server", systemImage: "plus.circle") }
                }
                Section {
                    Picker("Show actions", selection: $filter) {
                        Text("All servers").tag(String?.none)
                        ForEach(model.connections, id: \.id) { server in
                            Text(ServerPresentation.label(for: server)).tag(Optional(server.id))
                        }
                    }
                }
                Section {
                    ForEach(targets) { target in
                        HStack(spacing: 12) {
                            NavigationLink { ActionAppearanceView(capability: target.capability, server: target.connection) } label: {
                                HStack(spacing: 12) {
                                    CapabilityIcon(icon: target.capability.manifest.icon, override: ShareActionPreferences.icon(for: target.capability.id, deviceID: target.connection.deviceID))
                                    VStack(alignment: .leading, spacing: 3) {
                                        Text(target.capability.manifest.title)
                                        Text(ServerPresentation.label(for: target.connection)).font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                            }
                            Toggle("Enable \(target.capability.manifest.title)", isOn: Binding(
                                get: { !ShareActionPreferences.disabled(deviceID: target.connection.deviceID).contains(target.capability.id) },
                                set: { model.setTarget(target, enabled: $0) }
                            )).labelsHidden()
                        }
                    }.onMove { source, destination in
                        if filter == nil { model.moveTargets(from: source, to: destination) }
                    }.moveDisabled(filter != nil)
                    if targets.isEmpty { Text("No actions available. Check your servers and grants.").foregroundStyle(.secondary) }
                } header: { Text("Share sheet order & icons") } footer: {
                    Text("Use Edit to reorder targets across all servers. Tap an action to change its icon. Server filters are optional; choose All servers to change the share sheet order.")
                }
                ForEach(model.connections.filter { model.catalogErrors[$0.id] != nil }, id: \.id) { server in
                    Section(ServerPresentation.label(for: server)) {
                        Text(model.catalogErrors[server.id] ?? "").foregroundStyle(.red)
                    }
                }
                Section { NavigationLink("Shortcuts & Back Tap") { ShortcutSettingsView() } }
            }.navigationTitle("Settings").toolbar { EditButton() }
                .refreshable { await model.refreshCatalogs() }
                .task { await model.refreshCatalogs() }
        }
    }
}

struct ServerSettingsView: View {
    let server: Connection
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
                    if let connection = model.connection { NavigationLink("Server alias") { ServerAliasView(connection: connection) } }
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
            }.navigationTitle(model.connection.map { ServerPresentation.label(for: $0) } ?? "Server")
                .disabled(model.connection?.deviceID != server.deviceID)
                .task { await model.selectServer(server.id) }
                .confirmationDialog("Forget this server?", isPresented: $confirmDisconnect, titleVisibility: .visible) { Button("Forget server", role: .destructive) { model.disconnect() } }
        }
    }
}

private struct ActionSymbolCategory: Identifiable {
    let title: String
    let symbols: [ActionSymbol]
    var id: String { title }
}

private struct ActionSymbol: Identifiable {
    let name: String
    let title: String
    var id: String { name }
}

private enum ActionSymbolCatalog {
    static let categories: [ActionSymbolCategory] = [
        ActionSymbolCategory(title: "Send & save", symbols: [
            ActionSymbol(name: "paperplane.fill", title: "Send"),
            ActionSymbol(name: "tray.and.arrow.down.fill", title: "Inbox"),
            ActionSymbol(name: "tray.and.arrow.up.fill", title: "Outbox"),
            ActionSymbol(name: "bookmark.fill", title: "Bookmark"),
            ActionSymbol(name: "archivebox.fill", title: "Archive"),
            ActionSymbol(name: "folder.fill", title: "Folder"),
            ActionSymbol(name: "externaldrive.fill", title: "Drive"),
            ActionSymbol(name: "icloud.fill", title: "Cloud"),
            ActionSymbol(name: "square.and.arrow.up", title: "Share"),
            ActionSymbol(name: "square.and.arrow.down", title: "Download"),
            ActionSymbol(name: "arrow.up.right", title: "Open"),
            ActionSymbol(name: "arrow.triangle.branch", title: "Route"),
            ActionSymbol(name: "arrow.triangle.merge", title: "Merge"),
            ActionSymbol(name: "arrow.triangle.2.circlepath", title: "Sync"),
            ActionSymbol(name: "pin.fill", title: "Pin"),
            ActionSymbol(name: "shippingbox.fill", title: "Package"),
        ]),
        ActionSymbolCategory(title: "AI & development", symbols: [
            ActionSymbol(name: "terminal.fill", title: "Terminal"),
            ActionSymbol(name: "chevron.left.forwardslash.chevron.right", title: "Code"),
            ActionSymbol(name: "cpu.fill", title: "Processor"),
            ActionSymbol(name: "brain", title: "Brain"),
            ActionSymbol(name: "sparkles", title: "AI"),
            ActionSymbol(name: "wand.and.stars", title: "Transform"),
            ActionSymbol(name: "bolt.fill", title: "Action"),
            ActionSymbol(name: "gearshape.fill", title: "Settings"),
            ActionSymbol(name: "wrench.and.screwdriver.fill", title: "Tools"),
            ActionSymbol(name: "hammer.fill", title: "Build"),
            ActionSymbol(name: "ladybug.fill", title: "Debug"),
            ActionSymbol(name: "testtube.2", title: "Experiment"),
            ActionSymbol(name: "curlybraces", title: "JSON"),
            ActionSymbol(name: "number", title: "Number"),
            ActionSymbol(name: "function", title: "Function"),
            ActionSymbol(name: "network", title: "Network"),
        ]),
        ActionSymbolCategory(title: "Links & reading", symbols: [
            ActionSymbol(name: "link", title: "Link"),
            ActionSymbol(name: "globe", title: "Web"),
            ActionSymbol(name: "safari.fill", title: "Browser"),
            ActionSymbol(name: "book.fill", title: "Book"),
            ActionSymbol(name: "books.vertical.fill", title: "Library"),
            ActionSymbol(name: "doc.text.fill", title: "Article"),
            ActionSymbol(name: "newspaper.fill", title: "News"),
            ActionSymbol(name: "text.magnifyingglass", title: "Research"),
            ActionSymbol(name: "magnifyingglass", title: "Search"),
            ActionSymbol(name: "eyeglasses", title: "Read"),
            ActionSymbol(name: "list.bullet", title: "List"),
            ActionSymbol(name: "text.alignleft", title: "Text"),
            ActionSymbol(name: "text.quote", title: "Quote"),
            ActionSymbol(name: "character.book.closed.fill", title: "Dictionary"),
            ActionSymbol(name: "graduationcap.fill", title: "Learn"),
            ActionSymbol(name: "lightbulb.fill", title: "Idea"),
        ]),
        ActionSymbolCategory(title: "Audio & video", symbols: [
            ActionSymbol(name: "waveform", title: "Audio"),
            ActionSymbol(name: "mic.fill", title: "Record"),
            ActionSymbol(name: "headphones", title: "Listen"),
            ActionSymbol(name: "music.note", title: "Music"),
            ActionSymbol(name: "speaker.wave.2.fill", title: "Speaker"),
            ActionSymbol(name: "play.fill", title: "Play"),
            ActionSymbol(name: "film.fill", title: "Movie"),
            ActionSymbol(name: "video.fill", title: "Video"),
            ActionSymbol(name: "captions.bubble.fill", title: "Captions"),
            ActionSymbol(name: "text.bubble.fill", title: "Transcript"),
            ActionSymbol(name: "music.note.list", title: "Playlist"),
            ActionSymbol(name: "radio.fill", title: "Radio"),
            ActionSymbol(name: "antenna.radiowaves.left.and.right", title: "Broadcast"),
            ActionSymbol(name: "ear.fill", title: "Hearing"),
            ActionSymbol(name: "pianokeys", title: "Instrument"),
            ActionSymbol(name: "metronome.fill", title: "Tempo"),
        ]),
        ActionSymbolCategory(title: "Images & design", symbols: [
            ActionSymbol(name: "photo.fill", title: "Image"),
            ActionSymbol(name: "photo.on.rectangle.angled", title: "Gallery"),
            ActionSymbol(name: "camera.fill", title: "Camera"),
            ActionSymbol(name: "camera.viewfinder", title: "Capture"),
            ActionSymbol(name: "crop", title: "Crop"),
            ActionSymbol(name: "paintpalette.fill", title: "Palette"),
            ActionSymbol(name: "paintbrush.fill", title: "Paint"),
            ActionSymbol(name: "pencil.tip", title: "Draw"),
            ActionSymbol(name: "eyedropper.halffull", title: "Color"),
            ActionSymbol(name: "square.on.circle", title: "Shapes"),
            ActionSymbol(name: "scribble.variable", title: "Sketch"),
            ActionSymbol(name: "viewfinder", title: "Scan"),
            ActionSymbol(name: "slider.horizontal.3", title: "Adjust"),
            ActionSymbol(name: "rectangle.stack.fill", title: "Layers"),
            ActionSymbol(name: "cube.fill", title: "3D"),
            ActionSymbol(name: "scissors", title: "Cut"),
        ]),
        ActionSymbolCategory(title: "Files & work", symbols: [
            ActionSymbol(name: "doc.fill", title: "File"),
            ActionSymbol(name: "doc.richtext", title: "Document"),
            ActionSymbol(name: "doc.zipper", title: "ZIP archive"),
            ActionSymbol(name: "tablecells.fill", title: "Spreadsheet"),
            ActionSymbol(name: "chart.bar.fill", title: "Chart"),
            ActionSymbol(name: "chart.pie.fill", title: "Analysis"),
            ActionSymbol(name: "rectangle.on.rectangle", title: "Slides"),
            ActionSymbol(name: "calendar", title: "Calendar"),
            ActionSymbol(name: "checklist", title: "Tasks"),
            ActionSymbol(name: "checkmark.circle.fill", title: "Done"),
            ActionSymbol(name: "clock.fill", title: "Later"),
            ActionSymbol(name: "timer", title: "Timer"),
            ActionSymbol(name: "briefcase.fill", title: "Work"),
            ActionSymbol(name: "building.2.fill", title: "Company"),
            ActionSymbol(name: "signature", title: "Sign"),
            ActionSymbol(name: "printer.fill", title: "Print"),
        ]),
        ActionSymbolCategory(title: "Messages & people", symbols: [
            ActionSymbol(name: "bubble.left.and.bubble.right.fill", title: "Chat"),
            ActionSymbol(name: "envelope.fill", title: "Email"),
            ActionSymbol(name: "at", title: "Mention"),
            ActionSymbol(name: "bell.fill", title: "Notification"),
            ActionSymbol(name: "person.fill", title: "Person"),
            ActionSymbol(name: "person.2.fill", title: "Team"),
            ActionSymbol(name: "person.crop.circle.fill", title: "Profile"),
            ActionSymbol(name: "phone.fill", title: "Call"),
            ActionSymbol(name: "megaphone.fill", title: "Announce"),
            ActionSymbol(name: "hand.thumbsup.fill", title: "Like"),
            ActionSymbol(name: "heart.fill", title: "Favorite"),
            ActionSymbol(name: "star.fill", title: "Star"),
            ActionSymbol(name: "gift.fill", title: "Gift"),
            ActionSymbol(name: "flag.fill", title: "Flag"),
            ActionSymbol(name: "questionmark.circle.fill", title: "Help"),
            ActionSymbol(name: "info.circle.fill", title: "Information"),
        ]),
        ActionSymbolCategory(title: "Devices & security", symbols: [
            ActionSymbol(name: "desktopcomputer", title: "Desktop"),
            ActionSymbol(name: "laptopcomputer", title: "Laptop"),
            ActionSymbol(name: "iphone", title: "Phone"),
            ActionSymbol(name: "ipad", title: "Tablet"),
            ActionSymbol(name: "server.rack", title: "Server"),
            ActionSymbol(name: "display", title: "Display"),
            ActionSymbol(name: "keyboard.fill", title: "Keyboard"),
            ActionSymbol(name: "wifi", title: "Wi-Fi"),
            ActionSymbol(name: "bolt.horizontal.fill", title: "Connect"),
            ActionSymbol(name: "lock.fill", title: "Lock"),
            ActionSymbol(name: "key.fill", title: "Key"),
            ActionSymbol(name: "shield.fill", title: "Security"),
            ActionSymbol(name: "checkmark.shield.fill", title: "Verified"),
            ActionSymbol(name: "faceid", title: "Face ID"),
            ActionSymbol(name: "qrcode", title: "QR code"),
            ActionSymbol(name: "touchid", title: "Fingerprint"),
        ]),
        ActionSymbolCategory(title: "Life & places", symbols: [
            ActionSymbol(name: "house.fill", title: "Home"),
            ActionSymbol(name: "mappin.and.ellipse", title: "Place"),
            ActionSymbol(name: "map.fill", title: "Map"),
            ActionSymbol(name: "location.fill", title: "Location"),
            ActionSymbol(name: "car.fill", title: "Car"),
            ActionSymbol(name: "airplane", title: "Travel"),
            ActionSymbol(name: "cart.fill", title: "Shopping"),
            ActionSymbol(name: "creditcard.fill", title: "Payment"),
            ActionSymbol(name: "dollarsign.circle.fill", title: "Money"),
            ActionSymbol(name: "leaf.fill", title: "Nature"),
            ActionSymbol(name: "sun.max.fill", title: "Day"),
            ActionSymbol(name: "moon.fill", title: "Night"),
            ActionSymbol(name: "flame.fill", title: "Fire"),
            ActionSymbol(name: "pawprint.fill", title: "Pets"),
            ActionSymbol(name: "fork.knife", title: "Food"),
            ActionSymbol(name: "cup.and.saucer.fill", title: "Coffee"),
        ]),
    ]

    static func matching(_ query: String) -> [ActionSymbolCategory] {
        let terms = query.split(whereSeparator: \.isWhitespace).map(String.init)
        return categories.compactMap { category in
            let symbols = category.symbols.filter { symbol in
                let keywords = "\(category.title) \(symbol.title) \(symbol.name)"
                return terms.allSatisfy { keywords.localizedStandardContains($0) }
            }
            return symbols.isEmpty ? nil : ActionSymbolCategory(title: category.title, symbols: symbols)
        }
    }
}

private struct ActionSymbolGrid: View {
    let symbols: [ActionSymbol]
    let selectedName: String?
    let onSelect: (String) -> Void
    var body: some View {
    LazyVGrid(columns: [GridItem(.adaptive(minimum: 60))], spacing: 12) {
        ForEach(symbols) { symbol in
            Button { onSelect(symbol.name) } label: {
                VStack(spacing: 6) {
                    Image(systemName: symbol.name).font(.title2).frame(height: 28)
                    Text(symbol.title).font(.caption2).lineLimit(2).multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity, minHeight: 68)
                .padding(.horizontal, 2)
                .background(selectedName == symbol.name ? Color.accentColor.opacity(0.16) : Color.clear, in: RoundedRectangle(cornerRadius: 10))
                .overlay(alignment: .topTrailing) {
                    if selectedName == symbol.name {
                        Image(systemName: "checkmark.circle.fill").font(.caption).foregroundStyle(.tint)
                    }
                }
            }.buttonStyle(.plain)
                .accessibilityLabel("Use \(symbol.title) icon")
                .accessibilityAddTraits(selectedName == symbol.name ? [.isSelected] : [])
        }
    }.padding(.vertical, 6)
    }
}

private struct ActionIconPreview: View {
    let capability: Capability
    let selected: ActionIconOverride?
    var body: some View {
        HStack(spacing: 16) {
            CapabilityIcon(icon: capability.manifest.icon, override: selected, size: 48)
            VStack(alignment: .leading, spacing: 4) {
                Text(capability.manifest.title).font(.headline)
                Text(selected?.usesServerArtwork != false ? "Using server icon" : "Using icon on this iPhone").font(.subheadline).foregroundStyle(.secondary)
            }
        }.padding(.vertical, 4)
    }
}

private struct ActionIconColorPicker: View {
    let tint: ActionIconTint?
    let onSelect: (ActionIconTint?) -> Void
    var body: some View {
        Section {
            Button { onSelect(nil) } label: {
                HStack {
                    Text("Original / automatic")
                    Spacer()
                    if tint == nil { Image(systemName: "checkmark").accessibilityLabel("Selected") }
                }
            }
            ColorPicker("Custom color", selection: Binding(
                get: { tint?.color ?? .accentColor },
                set: { onSelect(ActionIconTint(color: $0)) }
            ), supportsOpacity: false)
        } header: { Text("Icon color") }
        footer: { Text("A custom color tints the icon in the app and share sheet. Original keeps artwork such as Pi in its supplied light and dark colors.") }
    }
}

private struct ActionArtworkPicker: View {
    let icon: ActionIcon?
    let usesServerArtwork: Bool
    let chooseImage: () -> Void
    let useServer: () -> Void
    var body: some View {
    Section {
        Button(action: chooseImage) { Label("Choose image…", systemImage: "photo") }
        Button(action: useServer) {
            HStack {
                CapabilityIcon(icon: icon, size: 24)
                Text("Use server icon")
                Spacer()
                if usesServerArtwork { Image(systemName: "checkmark").accessibilityLabel("Selected") }
            }
        }
    } footer: { Text("Your choice appears in the app and share sheet on this iPhone. Use the server icon to follow updates from the action’s provider.") }
    }
}

private struct ActionAppearanceView: View {
    @EnvironmentObject private var model: AppModel
    let capability: Capability
    var server: Connection? = nil
    @State private var choosingImage = false
    @State private var error: String?
    @State private var iconSearch = ""
    private var categories: [ActionSymbolCategory] { ActionSymbolCatalog.matching(iconSearch) }
    private var selected: ActionIconOverride? {
        guard let connection = server ?? model.connection else { return nil }
        return ShareActionPreferences.icon(for: capability.id, deviceID: connection.deviceID)
    }
    private func setIcon(_ icon: ActionIconOverride?) {
        saveAppearance(ActionIconOverride(systemName: icon?.systemName, image: icon?.image, tint: selected?.tint))
    }
    private func saveAppearance(_ value: ActionIconOverride) {
        let icon: ActionIconOverride? = value.usesServerArtwork && value.tint == nil ? nil : value
        guard let connection = server ?? model.connection else { return }
        ShareActionPreferences.saveIcon(icon, for: capability.id, deviceID: connection.deviceID)
        if connection.deviceID == model.connection?.deviceID { model.setIcon(icon, for: capability.id) }
        model.objectWillChange.send()
    }
    var body: some View {
        Form {
            Section { ActionIconPreview(capability: capability, selected: selected) }
            ActionArtworkPicker(icon: capability.manifest.icon, usesServerArtwork: selected?.usesServerArtwork != false,
                                chooseImage: { choosingImage = true }, useServer: { setIcon(nil) })
            ActionIconColorPicker(tint: selected?.tint) { color in
                var appearance = selected ?? ActionIconOverride()
                appearance.tint = color
                saveAppearance(appearance)
            }
            if categories.isEmpty {
                ContentUnavailableView.search(text: iconSearch)
            }
            ForEach(categories) { category in
                Section(category.title) {
                    ActionSymbolGrid(symbols: category.symbols, selectedName: selected?.systemName) { setIcon(.system($0)) }
                }
            }
            if let error { Section { Text(error).foregroundStyle(.red) } }
        }.navigationTitle("Action icon").navigationBarTitleDisplayMode(.inline)
            .searchable(text: $iconSearch, prompt: "Search icons, e.g. audio, code, save")
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
            setIcon(.custom(try ActionIcon.imported(data)))
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

struct ServerAliasView: View {
    let connection: Connection
    @State private var alias = ""
    @State private var showAlias = true
    var body: some View {
        Form {
            Section {
                TextField(connection.displayName, text: $alias)
                    .textInputAutocapitalization(.never)
                Toggle("Show alias below share targets", isOn: $showAlias)
            } header: { Text("Server alias") } footer: { Text("Aliases are saved on this iPhone. Leave the name empty to use the server address.") }
        }
        .navigationTitle("Server label")
        .onAppear { alias = ServerPresentation.alias(for: connection); showAlias = ServerPresentation.showsAlias(for: connection) }
        .onChange(of: alias) { _, value in ServerPresentation.save(alias: value, show: showAlias, for: connection) }
        .onChange(of: showAlias) { _, value in ServerPresentation.save(alias: alias, show: value, for: connection) }
    }
}

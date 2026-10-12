import SwiftUI
import UniformTypeIdentifiers

final class ShareViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        let model = ShareModel(context: extensionContext)
        let host = UIHostingController(rootView: ShareView(model: model))
        addChild(host); view.addSubview(host.view)
        host.view.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([host.view.topAnchor.constraint(equalTo: view.topAnchor), host.view.bottomAnchor.constraint(equalTo: view.bottomAnchor), host.view.leadingAnchor.constraint(equalTo: view.leadingAnchor), host.view.trailingAnchor.constraint(equalTo: view.trailingAnchor)])
        host.didMove(toParent: self)
        Task { await model.load() }
    }
}

@MainActor final class ShareModel: ObservableObject {
    @Published var extraInputs: [ShareInput] = []
    @Published var batchPosition: String?
    var contents: [ShareInput] { content.map { [$0] + extraInputs.map { $0.addingText(additionalText) } } ?? [] }
    @Published var input: ShareInput?
    @Published var pageInput: ShareInput?
    @Published var includePageText = false
    @Published var additionalText = ""
    var content: ShareInput? {
        let original = includePageText ? pageInput ?? input : input
        return original?.addingText(additionalText)
    }
    @Published var capabilities: [Capability] = []
    @Published var catalogs: [String: [Capability]] = [:]
    @Published var serverErrors: [String: String] = [:]
    @Published var chainConnection: Connection?
    @Published var groupReceipts: [OutboxItem] = []
    @Published var groupFailures: [String] = []
    @Published var connections: [Connection] = []
    @Published var connection: Connection?
    @Published var deviceID: String?
    @Published var chain: [Capability] = []
    @Published var selectingGroup = false
    @Published var selectedGroupIDs: Set<String> = []
    @Published var nextSteps: [Capability] = []
    @Published var findingSteps = false
    @Published var savedMessage: String?
    private var selectionVersion = UUID()
    @Published var loading = true
    @Published var sending: String?
    @Published var sent = false
    @Published var receipt: OutboxItem?
    @Published var parked = false
    @Published var uploadingID: String?
    @Published var parkProgress: UploadProgress?
    @Published var error: String?
    private let context: NSExtensionContext?
    private var requestKeys: [String: String] = [:]
    private var parkedIndices: Set<Int> = []
    var hasParkedItems: Bool { !parkedIndices.isEmpty }
    private var completed = false
    init(context: NSExtensionContext?) { self.context = context }
    func done() {
        guard !completed else { return }
        completed = true
        context?.completeRequest(returningItems: nil)
    }
    func load() async {
        loading = true; error = nil
        defer { loading = false }
        do {
            guard let items = context?.inputItems as? [NSExtensionItem] else { throw ClientError.message("No shareable content was provided.") }
            let providers = items.flatMap { $0.attachments ?? [] }
            if let page = await Self.page(in: items) {
                extraInputs = []
                pageInput = .text(try page.text())
                input = page.sharedURLs.first.map { .text($0, mime: "text/uri-list") } ?? pageInput
            } else {
                pageInput = nil
                let loaded = try await ShareContentLoader.loadAll(providers)
                input = loaded.first; extraInputs = Array(loaded.dropFirst())
            }
            connections = try CredentialStore.loadAll()
            guard !connections.isEmpty else { throw ClientError.message("Open xlatch and pair a server first.") }
            connection = try CredentialStore.load() ?? connections.first
            deviceID = connection?.deviceID
            catalogs = Dictionary(uniqueKeysWithValues: connections.map { ($0.id, APIClient.cachedCapabilities(for: $0)) })
            serverErrors = [:]
            await withTaskGroup(of: (String, [Capability]?, String?).self) { group in
                for server in connections {
                    group.addTask {
                        do { return (server.id, try await APIClient(connection: server).capabilities(), nil) }
                        catch { return (server.id, nil, error.localizedDescription) }
                    }
                }
                for await (id, actions, failure) in group {
                    if let actions { catalogs[id] = actions }
                    if let failure { serverErrors[id] = failure }
                }
            }
            capabilities = connection.flatMap { catalogs[$0.id] } ?? []
        } catch { self.error = error.localizedDescription }
    }
    func refreshServer(_ server: Connection) async {
        do {
            catalogs[server.id] = try await APIClient(connection: server).capabilities()
            serverErrors.removeValue(forKey: server.id)
        } catch { serverErrors[server.id] = error.localizedDescription }
    }
    var stepReferences: [[String: String]] { chain.map { ["capability_id": $0.id, "revision": $0.revision] } }
    var choices: [ShareTarget] {
        let servers = chainConnection.map { [$0] } ?? connections
        return ShareActionPreferences.orderedTargets(servers.flatMap { server in
            let source = chain.isEmpty ? (catalogs[server.id] ?? []).filter { capability in !contents.isEmpty && contents.allSatisfy { capability.accepts($0.mime) } } : nextSteps
            let disabled = ShareActionPreferences.disabled(deviceID: server.deviceID)
            let ordered = ShareActionPreferences.ordered(source.filter { !disabled.contains($0.id) }, deviceID: server.deviceID)
            return ordered.filter { !selectingGroup || Self.isLeaf($0) }.map { ShareTarget(capability: $0, connection: server) }
        })
    }
    var selectedGroup: [ShareTarget] { choices.filter { selectedGroupIDs.contains($0.id) } }
    var canSaveGroup: Bool { Set(selectedGroup.map { $0.connection.id }).count == 1 }
    private static func isLeaf(_ capability: Capability) -> Bool {
        guard let kind = capability.manifest.execution?.kind else { return false }
        return !["compose", "fan_out"].contains(kind)
    }
    func iconOverride(for capabilityID: String, server: Connection? = nil) -> ActionIconOverride? {
        (server ?? chainConnection ?? connection).flatMap { ShareActionPreferences.icon(for: capabilityID, deviceID: $0.deviceID) }
    }
    func addStep(_ target: ShareTarget) async {
        let capability = target.capability
        guard !selectingGroup, sending == nil, !findingSteps, chain.count < 16, capability.manifest.execution?.kind != nil,
              Self.isLeaf(capability), choices.contains(where: { $0.id == target.id }) else { return }
        chainConnection = target.connection
        chain.append(capability)
        await refreshSteps()
    }
    func undoStep() async {
        guard sending == nil else { return }
        if !chain.isEmpty { chain.removeLast() }
        if chain.isEmpty { chainConnection = nil }
        await refreshSteps()
    }
    func clearChain() {
        selectionVersion = UUID(); chainConnection = nil; chain = []; nextSteps = []; findingSteps = false; error = nil; savedMessage = nil
    }
    func beginGroupSelection() {
        guard sending == nil, chain.isEmpty else { return }
        selectingGroup = true; selectedGroupIDs = []; error = nil; savedMessage = nil
    }
    func cancelGroupSelection() {
        selectingGroup = false; selectedGroupIDs = []
    }
    func toggleGroupTarget(_ target: ShareTarget) {
        guard selectingGroup, sending == nil, Self.isLeaf(target.capability) else { return }
        if selectedGroupIDs.contains(target.id) { selectedGroupIDs.remove(target.id) }
        else if selectedGroupIDs.count < 16 { selectedGroupIDs.insert(target.id) }
    }
    func refreshSteps() async {
        let version = UUID(); selectionVersion = version
        nextSteps = []; error = nil
        guard !chain.isEmpty else { findingSteps = false; return }
        findingSteps = true
        defer { if selectionVersion == version { findingSteps = false } }
        do {
            guard let connection = chainConnection else { throw ClientError.message("Choose a first step.") }
            let candidates: [Capability] = try await APIClient(connection: connection).rpc(["op": "chain_candidates", "steps": stepReferences])
            if selectionVersion == version { nextSteps = candidates }
        } catch { if selectionVersion == version { self.error = "Could not check compatible steps. " + error.localizedDescription } }
    }
    func saveChain(title: String) async {
        guard chain.count >= 2, sending == nil else { return }
        sending = "save-chain"; defer { sending = nil }
        do {
            guard let connection = chainConnection else { throw ClientError.message("Choose a first step.") }
            let _: Capability = try await APIClient(connection: connection).rpc(["op": "save_chain", "steps": stepReferences, "title": title])
            savedMessage = "Saved for approval. Open xlatch to approve the new target and grant access."
        } catch { self.error = error.localizedDescription }
    }
    func saveGroup(title: String) async {
        let targets = selectedGroup
        guard targets.count >= 2, canSaveGroup, sending == nil else { return }
        sending = "save-group"; defer { sending = nil }
        do {
            let connection = targets[0].connection
            let references = targets.map { ["capability_id": $0.capability.id, "revision": $0.capability.revision] }
            let _: Capability = try await APIClient(connection: connection).rpc(["op": "save_group", "targets": references, "title": title])
            savedMessage = "Saved for approval. Open xlatch to approve the new target and grant access."
            cancelGroupSelection()
        } catch { self.error = error.localizedDescription }
    }
    func sendGroup() async {
        let targets = selectedGroup
        guard targets.count >= 2, sending == nil, !contents.isEmpty else { return }
        sending = "send-group"; error = nil
        defer { sending = nil; uploadingID = nil; batchPosition = nil }
        await sendBatch(targets: targets)
    }
    private func sendBatch(targets: [ShareTarget], steps: [Capability]? = nil) async {
        let inputs = contents
        let suffix = includePageText ? ":page" : ":original"
        let targetKeys = targets.map { target in
            let revisions = (steps ?? [target.capability]).map(\.revision).joined(separator: ">")
            return target.id + "@" + revisions
        }
        let keyIDs = inputs.indices.flatMap { index in targetKeys.map { $0 + ":item-\(index)" + suffix } }
        let ids = keyIDs.map { keyID -> String in
            let id = requestKeys[keyID] ?? UUID().uuidString; requestKeys[keyID] = id; return id
        }
        do {
            groupReceipts = try await OutboxDelivery.shared.submitBatch(inputs, targets: targets, ids: ids, chain: steps) { [weak self] position, id in
                Task { @MainActor in self?.batchPosition = "Sending \(position) of \(ids.count)"; self?.uploadingID = id }
            }
            groupFailures = []
            for (index, item) in groupReceipts.enumerated() {
                let target = targets[index % targets.count]
                receipt = item
                if item.state == .paused || item.state == .expired || item.state == .cancelled {
                    groupFailures.append(item.label + ": " + (item.detail ?? item.confirmation))
                    continue
                }
                do { try await observeReceipt(keyID: keyIDs[index], connection: target.connection) }
                catch { groupFailures.append(item.label + " · " + target.capability.manifest.title + ": " + error.localizedDescription) }
            }
            receipt = groupReceipts.first
            sent = true
        } catch { self.error = error.localizedDescription }
    }
    func send(_ target: ShareTarget) async {
        let capability = target.capability
        guard sending == nil, !findingSteps, chain.count < 16, choices.contains(where: { $0.id == target.id }), let input = content else { return }
        sending = target.id; error = nil; defer { sending = nil; uploadingID = nil; batchPosition = nil }
        do {
            let connection = target.connection
            let steps = chain.isEmpty ? [capability] : chain + [capability]
            if contents.count > 1 {
                await sendBatch(targets: [ShareTarget(capability: steps[0], connection: connection)], steps: chain.isEmpty ? nil : steps)
                return
            }
            let keyID = connection.id + ":" + steps.map { $0.id + "@" + $0.revision }.joined(separator: ">") + (includePageText ? ":page" : ":original")
            let key = requestKeys[keyID] ?? UUID().uuidString; requestKeys[keyID] = key
            uploadingID = key
            receipt = try await OutboxDelivery.shared.submit(input, capability: steps[0], connection: connection, id: key, chain: chain.isEmpty ? nil : steps)
            try await observeReceipt(keyID: keyID, connection: connection)
            sent = true
        } catch { self.error = error.localizedDescription }
    }
    private func observeReceipt(keyID: String, connection: Connection) async throws {
        guard let jobID = receipt?.jobID else { return }
        let client = try APIClient(connection: connection)
        let terminal: Job?
        do {
            terminal = try await ImmediateJobObservation.wait(for: jobID) { id in
                try await client.rpc(["op": "job", "id": id])
            }
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            // The invocation is durably accepted. A transient status-read failure
            // must not turn it into a duplicate invocation.
            terminal = nil
        }
        if let terminal, terminal.status != "succeeded" {
            requestKeys.removeValue(forKey: keyID)
            receipt = nil
            throw ClientError.message(ImmediateJobObservation.failureMessage(for: terminal))
        }
    }
    func park() async {
        guard sending == nil, !contents.isEmpty else { return }
        sending = "park"; error = nil
        defer { sending = nil; parkProgress = nil; batchPosition = nil }
        do {
            guard let connection else { throw ClientError.message("Choose a server for Save for later.") }
            let client = try APIClient(connection: connection)
            let inputs = contents
            for (index, original) in inputs.enumerated() where !parkedIndices.contains(index) {
                batchPosition = "Saving \(index + 1) of \(inputs.count)"
                let key = "park-\(index)"
                let id = requestKeys[key] ?? UUID().uuidString; requestKeys[key] = id
                let input = try await client.upload(original, id: id) { [weak self] sent, total in
                    Task { @MainActor in self?.parkProgress = UploadProgress(sent: sent, total: total) }
                }
                var request: [String: Any] = ["op": "park", "id": id, "label": String(input.label.prefix(500)), "mime_type": input.mime, "input": input.payload]
                if original.mime == "text/uri-list", let selected = ShareActionPreferences.saveForLaterPreparation(deviceID: connection.deviceID),
                   let capability = capabilities.first(where: { $0.id == selected && $0.accepts(original.mime) }) {
                    request["preparation"] = ["capability_id": capability.id, "revision": capability.revision]
                }
                let _: ParkedItem = try await client.rpc(request)
                parkedIndices.insert(index)
            }
            for input in inputs { if let file = input.localFile { try? FileManager.default.removeItem(at: file) } }
            parked = true; sent = true
        } catch { self.error = "Saved \(parkedIndices.count) of \(contents.count) items. Tap Save for later to retry the remaining items. " + error.localizedDescription }
    }
    private static func page(in items: [NSExtensionItem]) async -> CapturedContext? {
        for provider in items.flatMap({ $0.attachments ?? [] }) where provider.hasItemConformingToTypeIdentifier(UTType.propertyList.identifier) {
            // Safari enrichment is optional; another attachment still provides the original share.
            guard let values = try? await provider.loadItem(forTypeIdentifier: UTType.propertyList.identifier) as? [String: Any],
                  let page = values[NSExtensionJavaScriptPreprocessingResultsKey] as? [String: Any] else { continue }
            return CapturedContext(sharedURLs: (page["url"] as? String).flatMap(CapturedContext.webURL).map { [$0] } ?? [],
                pageTitle: page["title"] as? String, pageText: page["text"] as? String)
        }
        return nil
    }

}

struct ShareView: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var model: ShareModel
    @State private var namingChain = false
    @State private var chainName = ""
    @State private var namingGroup = false
    @State private var groupName = ""
    var body: some View {
        NavigationStack {
            Group {
                if model.sent { confirmation }
                else { actionList }
            }
            .navigationTitle("xlatch").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { model.done() }.disabled(model.sending != nil)
                }
                ToolbarItem(placement: .primaryAction) {
                    if model.chain.isEmpty && model.content != nil {
                        Button(model.selectingGroup ? "Cancel" : "Select multiple") {
                            if model.selectingGroup { model.cancelGroupSelection() }
                            else { model.beginGroupSelection() }
                        }.disabled(model.sending != nil)
                    }
                }
            }
        }
        .alert("Save chain as target", isPresented: $namingChain) {
            TextField("Target name", text: $chainName)
            Button("Submit for approval") { Task { await model.saveChain(title: chainName) } }
            Button("Cancel", role: .cancel) {}
        } message: { Text("The new target needs approval before it becomes available.") }
        .alert("Save group as target", isPresented: $namingGroup) {
            TextField("Target name", text: $groupName)
            Button("Submit for approval") { Task { await model.saveGroup(title: groupName) } }
            Button("Cancel", role: .cancel) {}
        } message: { Text("Every share will be sent independently to all selected targets. The new target needs approval before it becomes available.") }
        .tint(colorScheme == .dark
            ? Color(red: 0.38, green: 0.85, blue: 0.68)
            : Color(red: 0.12, green: 0.43, blue: 0.34))
    }
    private var confirmation: some View {
        VStack(spacing: 20) {
            Image(systemName: "checkmark.circle").font(.system(size: 48)).foregroundStyle(.tint)
            Text(!model.groupReceipts.isEmpty ? (model.groupFailures.isEmpty ? "\(model.groupReceipts.count) shares saved" : "Some items need attention") : (model.parked ? "Saved for later" : (model.receipt?.state == .sent ? "Sent to your server" : "Saved on this iPhone"))).font(.title2.bold())
            Text(model.parked ? "Open Later in xlatch whenever you are ready to choose a target." : (model.receipt?.confirmation ?? "Open xlatch’s Outbox to check delivery."))
                .foregroundStyle(.secondary).multilineTextAlignment(.center)
            ForEach(model.groupReceipts) { item in
                VStack(alignment: .leading) {
                    Text(item.label).font(.headline).lineLimit(2)
                    Text(item.capability.manifest.title).font(.subheadline)
                    if let server = model.connections.first(where: { $0.deviceID == item.deviceID && $0.pin == item.serverPin }) {
                        Text(ServerPresentation.label(for: server)).font(.caption).foregroundStyle(.secondary)
                    }
                    Text(item.confirmation).font(.footnote).foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            ForEach(model.groupFailures, id: \.self) { failure in Text(failure).font(.footnote).foregroundStyle(.red) }
            Button("Done") { model.done() }.buttonStyle(.borderedProminent)
        }.padding(28).task {
            guard model.groupFailures.isEmpty else { return }
            guard model.groupReceipts.isEmpty ? (model.parked || model.receipt?.state == .sent) : model.groupReceipts.allSatisfy({ $0.state == .sent }) else { return }
            do { try await Task.sleep(for: .milliseconds(700)) } catch { return }
            model.done()
        }
    }
    private var actionList: some View {
        List {
            if let input = model.content { Section("Sharing") {
                if model.contents.count > 1 { Text("\(model.contents.count) items").font(.headline) }
                Text(input.label).lineLimit(3)
                ForEach(Array(model.extraInputs.enumerated()), id: \.offset) { _, item in Text(item.label).lineLimit(2) }
            } }
            if model.content != nil {
                Section("Add text") {
                    TextField("Optional note or instructions", text: $model.additionalText, axis: .vertical)
                        .lineLimit(1...4)
                        .multilineTextAlignment(.leading)
                        .foregroundStyle(.primary)
                        .accessibilityLabel("Additional text")
                        .disabled(model.sending != nil || model.hasParkedItems)
                }
                Section {
                    if model.connections.count > 1 {
                        Picker("Save for later on", selection: Binding(
                            get: { model.connection?.id ?? "" },
                            set: { id in
                                model.connection = model.connections.first { $0.id == id }
                                model.deviceID = model.connection?.deviceID
                                model.capabilities = model.catalogs[id] ?? []
                            }
                        )) {
                            ForEach(model.connections, id: \.id) { server in Text(ServerPresentation.label(for: server)).tag(server.id) }
                        }.disabled(model.sending != nil || model.hasParkedItems)
                    }
                    Button { Task { await model.park() } } label: {
                        Label(model.sending == "park" ? "Saving…" : "Save for later", systemImage: "bookmark")
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }.disabled(model.sending != nil)
                }
            }
            if !model.chain.isEmpty { chainSection }
            if model.selectingGroup { groupSection }
            if let message = model.savedMessage { Section { Text(message) } }
            if model.pageInput != nil {
                Section {
                    Toggle("Include Safari page text", isOn: $model.includePageText)
                        .disabled(model.sending != nil || !model.chain.isEmpty || model.selectingGroup)
                } footer: { Text("Include the page title and text along with its URL.") }
            }
            if let error = model.error {
                Section {
                    Text(error).foregroundStyle(.red)
                    Button("Try again") { Task {
                        if model.content == nil { await model.load() } else if model.chain.isEmpty {
                            for server in model.connections { await model.refreshServer(server) }
                        } else { await model.refreshSteps() }
                    } }
                }
            }
            if model.loading || model.findingSteps { ProgressView("Finding compatible actions…") }
            if let position = model.batchPosition { Section { Text(position); ProgressView() } }
            if let progress = model.parkProgress { Section { UploadProgressBar(progress: progress) } }
            else if let id = model.uploadingID { Section { LiveUploadProgress(id: id) } }
            if model.content != nil { targets }
            ForEach(model.connections.filter { model.serverErrors[$0.id] != nil }, id: \.id) { server in
                Section {
                    DisclosureGroup(ServerPresentation.label(for: server) + " · unavailable") {
                        Text(model.serverErrors[server.id] ?? "").font(.footnote).foregroundStyle(.red)
                        Text("Cached targets remain available. Failed sends stay in Outbox for review or retry.").font(.footnote).foregroundStyle(.secondary)
                        Button("Retry connection") { Task { await model.refreshServer(server) } }
                    }
                }
            }
        }
    }
    private var groupSection: some View {
        Section {
            if model.selectedGroup.isEmpty {
                Text("Choose at least two targets below.").foregroundStyle(.secondary)
            } else {
                Text(model.selectedGroup.map { $0.capability.manifest.title }.joined(separator: " + "))
                    .font(.subheadline).foregroundStyle(.secondary)
            }
            Button {
                Task { await model.sendGroup() }
            } label: {
                Label(model.sending == "send-group" ? "Sending…" : "Send to \(model.selectedGroup.count)", systemImage: "paperplane.fill")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .disabled(model.selectedGroup.count < 2 || model.sending != nil)
            Button("Save selection as target…", systemImage: "square.and.arrow.down") {
                groupName = model.selectedGroup.map { $0.capability.manifest.title }.joined(separator: " + ")
                namingGroup = true
            }
            .disabled(model.selectedGroup.count < 2 || !model.canSaveGroup || model.sending != nil)
            if !model.canSaveGroup && model.selectedGroup.count >= 2 { Text("Sending across servers is supported. Saved server groups must use targets on one server.").font(.footnote).foregroundStyle(.secondary) }
        } header: { Text("Multiple targets") }
        footer: { Text("Each target receives the original item. Use a chain when one action should consume another action’s result.") }
    }
    private var chainSection: some View {
        Section {
            ForEach(Array(model.chain.enumerated()), id: \.offset) { index, step in
                HStack { CapabilityIcon(icon: step.manifest.icon, override: model.iconOverride(for: step.id), size: 24); Text("\(index + 1). \(step.manifest.title)") }
            }
            HStack {
                Button("Undo") { Task { await model.undoStep() } }.buttonStyle(.borderless)
                Spacer()
                Button("Clear") { model.clearChain() }.buttonStyle(.borderless)
            }.disabled(model.sending != nil)
            if model.chain.count >= 2 {
                Button("Save as target…") {
                    chainName = model.chain.map { $0.manifest.title }.joined(separator: " → ")
                    namingChain = true
                }.disabled(model.sending != nil)
            }
        } header: { Text("Your chain") }
        footer: { Text("Tap the next target to send. Swipe left to add another step.") }
    }
    @ViewBuilder private var targets: some View {
        if model.choices.isEmpty && !model.loading && !model.findingSteps {
            ContentUnavailableView("No compatible actions", systemImage: "bolt.slash",
                description: Text(model.chain.isEmpty ? "Enable or grant an action that accepts this content." : "No granted targets accept this output. Undo the last step or clear the chain."))
        }
        Section {
            ForEach(model.choices) { target in
                ShareTargetRow(model: model, target: target)
            }
        } header: { Text(model.selectingGroup ? "Select targets" : (model.chain.isEmpty ? "Choose an action" : "Send to next target")) }
        footer: {
            if model.selectingGroup { Text("Tap targets to select them. The normal one-tap send returns when you cancel selection.") }
            else if model.chain.isEmpty { Text("Tap to send. Select multiple sends the original item to several targets; Add step builds a chain.") }
        }
    }
}

private struct ShareTargetRow: View {
    @ObservedObject var model: ShareModel
    let target: ShareTarget
    private var capability: Capability { target.capability }
    private var canExtend: Bool {
        guard let kind = capability.manifest.execution?.kind else { return false }
        return !["compose", "fan_out"].contains(kind) && model.chain.count < 15
    }
    var body: some View {
        HStack {
            Button {
                if model.selectingGroup { model.toggleGroupTarget(target) }
                else { Task { await model.send(target) } }
            } label: {
                HStack {
                    CapabilityIcon(icon: capability.manifest.icon, override: model.iconOverride(for: capability.id, server: target.connection))
                    VStack(alignment: .leading, spacing: 3) {
                        Text(capability.manifest.title).font(.headline).foregroundStyle(Color(uiColor: .label))
                        if model.connections.count > 1 && ServerPresentation.showsAlias(for: target.connection) {
                            Text(ServerPresentation.label(for: target.connection)).font(.caption).foregroundStyle(.secondary)
                        }
                    }.multilineTextAlignment(.leading).frame(maxWidth: .infinity, alignment: .leading)
                    if model.selectingGroup {
                        Image(systemName: model.selectedGroupIDs.contains(target.id) ? "checkmark.circle.fill" : "circle")
                            .font(.title3).foregroundStyle(model.selectedGroupIDs.contains(target.id) ? Color.accentColor : Color.secondary)
                    } else if model.sending == target.id { ProgressView() }
                    else { Image(systemName: "arrow.up.right").foregroundStyle(.tint) }
                }.contentShape(Rectangle())
            }.buttonStyle(.plain)
            if canExtend && !model.selectingGroup {
                Menu {
                    Button("Add step", systemImage: "link") { Task { await model.addStep(target) } }
                } label: { Image(systemName: "ellipsis.circle").frame(minWidth: 44, minHeight: 44) }
                .accessibilityLabel("Options for " + capability.manifest.title)
            }
        }.padding(.vertical, 6)
        .disabled(model.sending != nil || model.findingSteps)
        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
            if canExtend && !model.selectingGroup {
                Button { Task { await model.addStep(target) } } label: {
                    Label("Add step", systemImage: "link")
                }.tint(.teal)
            }
        }
        .accessibilityAction(named: "Add step") { if canExtend && !model.selectingGroup { Task { await model.addStep(target) } } }
    }
}

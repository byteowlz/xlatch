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
    @Published var input: ShareInput?
    @Published var pageInput: ShareInput?
    @Published var includePageText = false
    var content: ShareInput? { includePageText ? pageInput ?? input : input }
    @Published var capabilities: [Capability] = APIClient.cachedCapabilities()
    @Published var connections: [Connection] = []
    @Published var connection: Connection?
    @Published var disabledActionIDs: Set<String> = []
    @Published var deviceID: String?
    @Published var chain: [Capability] = []
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
    private let parkID = UUID().uuidString
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
            guard let provider = ShareContentLoader.provider(in: providers) else { throw ClientError.message("No shareable content was provided.") }
            if let page = await Self.page(in: items) {
                pageInput = .text(try page.text())
                input = page.sharedURLs.first.map { .text($0, mime: "text/uri-list") } ?? pageInput
            } else {
                pageInput = nil
                input = try await ShareContentLoader.load(provider)
            }
            connections = try CredentialStore.loadAll()
            guard let connection = try CredentialStore.load() else { throw ClientError.message("Open xlatch and pair your server first.") }
            self.connection = connection
            deviceID = connection.deviceID
            disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID)
            capabilities = try await APIClient(connection: connection).capabilities()
        } catch { self.error = error.localizedDescription }
    }
    func selectServer(_ id: String) async {
        guard connection?.id != id, sending == nil else { return }
        loading = true; error = nil; clearChain()
        defer { loading = false }
        do {
            try CredentialStore.select(id)
            connections = try CredentialStore.loadAll()
            guard let connection = try CredentialStore.load() else { throw ClientError.message("This server is no longer paired.") }
            self.connection = connection
            deviceID = connection.deviceID
            disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID)
            let cached = APIClient.cachedCapabilities(for: connection)
            capabilities = cached.isEmpty ? try await APIClient(connection: connection).capabilities() : cached
        } catch { self.error = error.localizedDescription }
    }
    var stepReferences: [[String: String]] { chain.map { ["capability_id": $0.id, "revision": $0.revision] } }
    var choices: [Capability] {
        let source = chain.isEmpty ? capabilities.filter { capability in content.map { capability.accepts($0.mime) } ?? false } : nextSteps
        let enabled = source.filter { !disabledActionIDs.contains($0.id) }
        return deviceID.map { ShareActionPreferences.ordered(enabled, deviceID: $0) } ?? enabled
    }
    func iconOverride(for capabilityID: String) -> ActionIconOverride? {
        deviceID.flatMap { ShareActionPreferences.icon(for: capabilityID, deviceID: $0) }
    }
    func addStep(_ capability: Capability) async {
        guard sending == nil, !findingSteps, chain.count < 16, capability.manifest.execution?.kind != nil,
              capability.manifest.execution?.kind != "compose", choices.contains(capability) else { return }
        chain.append(capability)
        await refreshSteps()
    }
    func undoStep() async {
        guard sending == nil else { return }
        if !chain.isEmpty { chain.removeLast() }
        await refreshSteps()
    }
    func clearChain() {
        selectionVersion = UUID(); chain = []; nextSteps = []; findingSteps = false; error = nil; savedMessage = nil
    }
    func refreshSteps() async {
        let version = UUID(); selectionVersion = version
        nextSteps = []; error = nil
        guard !chain.isEmpty else { findingSteps = false; return }
        findingSteps = true
        defer { if selectionVersion == version { findingSteps = false } }
        do {
            guard let connection = try CredentialStore.load() else { throw ClientError.message("Pair your server first.") }
            let candidates: [Capability] = try await APIClient(connection: connection).rpc(["op": "chain_candidates", "steps": stepReferences])
            if selectionVersion == version { nextSteps = candidates }
        } catch { if selectionVersion == version { self.error = "Could not check compatible steps. " + error.localizedDescription } }
    }
    func saveChain(title: String) async {
        guard chain.count >= 2, sending == nil else { return }
        sending = "save-chain"; defer { sending = nil }
        do {
            guard let connection = try CredentialStore.load() else { throw ClientError.message("Pair your server first.") }
            let _: Capability = try await APIClient(connection: connection).rpc(["op": "save_chain", "steps": stepReferences, "title": title])
            savedMessage = "Saved for approval. Open xlatch to approve the new target and grant access."
        } catch { self.error = error.localizedDescription }
    }
    func send(_ capability: Capability) async {
        guard sending == nil, !findingSteps, chain.count < 16, choices.contains(capability), let input = content else { return }
        sending = capability.id; error = nil; defer { sending = nil; uploadingID = nil }
        do {
            guard let connection = try CredentialStore.load() else { throw ClientError.message("Pair your server in xlatch first.") }
            let steps = chain.isEmpty ? [capability] : chain + [capability]
            let keyID = steps.map { $0.id + "@" + $0.revision }.joined(separator: ">") + (includePageText ? ":page" : ":original")
            let key = requestKeys[keyID] ?? UUID().uuidString; requestKeys[keyID] = key
            uploadingID = key
            receipt = try await OutboxDelivery.shared.submit(input, capability: steps[0], connection: connection, id: key, chain: chain.isEmpty ? nil : steps)
            if let jobID = receipt?.jobID {
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
            sent = true
        } catch { self.error = error.localizedDescription }
    }
    func park() async {
        guard sending == nil, let original = content else { return }
        sending = "park"; error = nil; parkProgress = original.localFile == nil ? nil : UploadProgress(sent: 0, total: 0)
        defer { sending = nil; parkProgress = nil }
        do {
            guard let connection = try CredentialStore.load() else { throw ClientError.message("Pair your server in xlatch first.") }
            let client = try APIClient(connection: connection)
            let input = try await client.upload(original, id: parkID) { [weak self] sent, total in
                Task { @MainActor in self?.parkProgress = UploadProgress(sent: sent, total: total) }
            }
            var request: [String: Any] = ["op":"park", "id":parkID,
                "label":String(input.label.prefix(500)), "mime_type":input.mime, "input":input.payload]
            if original.mime == "text/uri-list", let deviceID,
               let selected = ShareActionPreferences.saveForLaterPreparation(deviceID: deviceID),
               let capability = capabilities.first(where: { $0.id == selected && $0.accepts(original.mime) }) {
                request["preparation"] = ["capability_id": capability.id, "revision": capability.revision]
            }
            let _: ParkedItem = try await client.rpc(request)
            if let localFile = original.localFile { try? FileManager.default.removeItem(at: localFile) }
            parked = true; sent = true
        } catch { self.error = error.localizedDescription }
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
    var body: some View {
        NavigationStack {
            Group {
                if model.sent { confirmation }
                else { actionList }
            }
            .navigationTitle("xlatch").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) {
                Button("Close") { model.done() }.disabled(model.sending != nil)
            } }
        }
        .alert("Save chain as target", isPresented: $namingChain) {
            TextField("Target name", text: $chainName)
            Button("Submit for approval") { Task { await model.saveChain(title: chainName) } }
            Button("Cancel", role: .cancel) {}
        } message: { Text("The new target needs approval before it becomes available.") }
        .tint(colorScheme == .dark
            ? Color(red: 0.38, green: 0.85, blue: 0.68)
            : Color(red: 0.12, green: 0.43, blue: 0.34))
    }
    private var confirmation: some View {
        VStack(spacing: 20) {
            Image(systemName: "checkmark.circle").font(.system(size: 48)).foregroundStyle(.tint)
            Text(model.parked ? "Saved for later" : (model.receipt?.state == .sent ? "Sent to your server" : "Saved on this iPhone")).font(.title2.bold())
            Text(model.parked ? "Open Later in xlatch whenever you are ready to choose a target." : (model.receipt?.confirmation ?? "Open xlatch’s Outbox to check delivery."))
                .foregroundStyle(.secondary).multilineTextAlignment(.center)
            Button("Done") { model.done() }.buttonStyle(.borderedProminent)
        }.padding(28).task {
            guard model.parked || model.receipt?.state == .sent else { return }
            do { try await Task.sleep(for: .milliseconds(700)) } catch { return }
            model.done()
        }
    }
    private var actionList: some View {
        List {
            if model.connections.count > 1 {
                Section("Server") {
                    Picker("Send with", selection: Binding(
                        get: { model.connection?.id ?? "" },
                        set: { id in Task { await model.selectServer(id) } }
                    )) {
                        ForEach(model.connections, id: \.id) { connection in
                            Text(connection.displayName).tag(connection.id)
                        }
                    }
                }
            }
            if let input = model.content { Section("Sharing") { Text(input.label).lineLimit(3) } }
            if model.content != nil {
                Section {
                    Button { Task { await model.park() } } label: {
                        Label(model.sending == "park" ? "Saving…" : "Save for later", systemImage: "bookmark")
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }.disabled(model.sending != nil)
                }
            }
            if !model.chain.isEmpty { chainSection }
            if let message = model.savedMessage { Section { Text(message) } }
            if model.pageInput != nil {
                Section {
                    Toggle("Include Safari page text", isOn: $model.includePageText)
                        .disabled(model.sending != nil || !model.chain.isEmpty)
                } footer: { Text("Include the page title and text along with its URL.") }
            }
            if let error = model.error {
                Section {
                    Text(error).foregroundStyle(.red)
                    Button("Try again") { Task {
                        if model.chain.isEmpty { await model.load() } else { await model.refreshSteps() }
                    } }
                }
            }
            if model.loading || model.findingSteps { ProgressView("Finding compatible actions…") }
            if let progress = model.parkProgress { Section { UploadProgressBar(progress: progress) } }
            else if let id = model.uploadingID { Section { LiveUploadProgress(id: id) } }
            if model.content != nil { targets }
        }
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
            ForEach(model.choices) { capability in
                ShareTargetRow(model: model, capability: capability)
            }
        } header: { Text(model.chain.isEmpty ? "Choose an action" : "Send to next target") }
        footer: { if model.chain.isEmpty { Text("Tap to send. Swipe left or use the action menu to start a chain.") } }
    }
}

private struct ShareTargetRow: View {
    @ObservedObject var model: ShareModel
    let capability: Capability
    private var canExtend: Bool {
        capability.manifest.execution?.kind != nil && capability.manifest.execution?.kind != "compose" && model.chain.count < 15
    }
    var body: some View {
        HStack {
            Button { Task { await model.send(capability) } } label: {
                HStack {
                    CapabilityIcon(icon: capability.manifest.icon, override: model.iconOverride(for: capability.id))
                    Text(capability.manifest.title).font(.headline).foregroundStyle(Color(uiColor: .label))
                        .multilineTextAlignment(.leading)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if model.sending == capability.id { ProgressView() }
                    else { Image(systemName: "arrow.up.right").foregroundStyle(.tint) }
                }.contentShape(Rectangle())
            }.buttonStyle(.plain)
            if canExtend {
                Menu {
                    Button("Add step", systemImage: "link") { Task { await model.addStep(capability) } }
                } label: { Image(systemName: "ellipsis.circle").frame(minWidth: 44, minHeight: 44) }
                .accessibilityLabel("Options for " + capability.manifest.title)
            }
        }.padding(.vertical, 6)
        .disabled(model.sending != nil || model.loading || model.findingSteps)
        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
            if canExtend {
                Button { Task { await model.addStep(capability) } } label: {
                    Label("Add step", systemImage: "link")
                }.tint(.teal)
            }
        }
        .accessibilityAction(named: "Add step") { if canExtend { Task { await model.addStep(capability) } } }
    }
}

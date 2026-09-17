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
    @Published var disabledActionIDs: Set<String> = []
    @Published var loading = true
    @Published var sending: String?
    @Published var sent = false
    @Published var receipt: OutboxItem?
    @Published var uploadingID: String?
    @Published var error: String?
    private let context: NSExtensionContext?
    private var requestKeys: [String: String] = [:]
    init(context: NSExtensionContext?) { self.context = context }
    func done() { context?.completeRequest(returningItems: nil) }
    func load() async {
        defer { loading = false }
        do {
            guard let items = context?.inputItems as? [NSExtensionItem] else { throw ClientError.message("No shareable content was provided.") }
            let providers = items.flatMap { $0.attachments ?? [] }
            guard let provider = providers.first(where: { $0.hasItemConformingToTypeIdentifier(UTType.url.identifier) || $0.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) }) ?? providers.first else { throw ClientError.message("No shareable content was provided.") }
            if let page = await Self.page(in: items) {
                pageInput = .text(try page.text())
                input = page.sharedURLs.first.map { .text($0, mime: "text/uri-list") } ?? pageInput
            } else {
                pageInput = nil
                input = try await Self.load(provider)
            }
            guard let connection = try CredentialStore.load() else { throw ClientError.message("Open xlatch and pair your server first.") }
            disabledActionIDs = ShareActionPreferences.disabled(deviceID: connection.deviceID)
            capabilities = try await APIClient(connection: connection).capabilities()
        } catch { self.error = error.localizedDescription }
    }
    func send(_ capability: Capability) async {
        guard sending == nil, let input = content else { return }
        sending = capability.id; error = nil; defer { sending = nil; uploadingID = nil }
        do {
            guard let connection = try CredentialStore.load() else { throw ClientError.message("Pair your server in xlatch first.") }
            let keyID = capability.id + (includePageText ? ":page" : ":original")
            let key = requestKeys[keyID] ?? UUID().uuidString; requestKeys[keyID] = key
            uploadingID = key
            receipt = try await OutboxDelivery.shared.submit(input, capability: capability, connection: connection, id: key)
            sent = true
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
    private static func load(_ provider: NSItemProvider) async throws -> ShareInput {
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
            let item = try await provider.loadItem(forTypeIdentifier: UTType.url.identifier)
            if let url = item as? URL { return .text(url.absoluteString, mime: "text/uri-list") }
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
            let item = try await provider.loadItem(forTypeIdentifier: UTType.plainText.identifier)
            if let text = item as? String { return .text(text) }
            if let data = item as? Data, let text = String(data: data, encoding: .utf8) { return .text(text) }
        }
        guard let typeID = provider.registeredTypeIdentifiers.first(where: { UTType($0)?.conforms(to: .data) == true }) else { throw ClientError.message("This content type is not supported yet.") }
        return try await withCheckedThrowingContinuation { continuation in
            provider.loadFileRepresentation(forTypeIdentifier: typeID) { url, error in
                do {
                    if let error { throw error }
                    guard let url else { throw ClientError.message("The source app did not provide a file.") }
                    // Read while the provider's temporary file is still valid.
                    let input = try ShareInput.file(at: url, mime: UTType(typeID)?.preferredMIMEType ?? "application/octet-stream")
                    continuation.resume(returning: input)
                } catch { continuation.resume(throwing: error) }
            }
        }
    }
}

struct ShareView: View {
    @ObservedObject var model: ShareModel
    var body: some View {
        NavigationStack {
            Group {
                if model.sent {
                    VStack(spacing: 20) {
                        Image(systemName: "checkmark.circle").font(.system(size: 48)).foregroundStyle(.tint)
                        Text(model.receipt?.state == .sent ? "Sent to your server" : "Saved on this iPhone").font(.title2.bold())
                        Text(model.receipt?.confirmation ?? "Open xlatch’s Outbox to check delivery.").foregroundStyle(.secondary).multilineTextAlignment(.center)
                        Button("Done") { model.done() }.buttonStyle(.borderedProminent)
                    }.padding(28)
                } else {
                    List {
                        if let input = model.content { Section("Sharing") { Text(input.label).lineLimit(3) } }
                        if model.pageInput != nil {
                            Section { Toggle("Include Safari page text", isOn: $model.includePageText).disabled(model.sending != nil) } footer: {
                                Text("Include the page title and text along with its URL. Turn off to share only the link.")
                            }
                        }
                        if let error = model.error { Section { Text(error).foregroundStyle(.red); Text("You can still save to a previously loaded target below for later delivery.").font(.caption); Button("Try connection again") { Task { await model.load() } } } }
                        if model.loading { ProgressView("Finding actions…") }
                        if let id = model.uploadingID { Section { LiveUploadProgress(id: id) } }
                        if let input = model.content {
                            let matches = model.capabilities.filter { $0.accepts(input.mime) && !model.disabledActionIDs.contains($0.id) }
                            if matches.isEmpty && !model.loading { ContentUnavailableView("No compatible actions", systemImage: "bolt.slash", description: Text("Enable or grant this device an action that accepts \(input.mime).")) }
                            Section("Choose an action") {
                                ForEach(matches) { capability in
                                    Button { Task { await model.send(capability) } } label: {
                                        HStack {
                                            VStack(alignment: .leading, spacing: 4) { Text(capability.manifest.title).font(.headline); Text(capability.manifest.description).font(.subheadline).foregroundStyle(.secondary) }
                                            Spacer()
                                            if model.sending == capability.id { ProgressView() } else { Image(systemName: "arrow.up.right") }
                                        }.padding(.vertical, 6)
                                    }.disabled(model.sending != nil || model.loading)
                                }
                            }
                        }
                    }
                }
            }.navigationTitle("xlatch").navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { model.done() }.disabled(model.sending != nil) } }
        }.tint(Color(red: 0.12, green: 0.43, blue: 0.34))
    }
}

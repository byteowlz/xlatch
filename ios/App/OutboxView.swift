import SwiftUI

struct OutboxView: View {
    @State private var items: [OutboxItem] = []
    @State private var error: String?
    @State private var pendingDelete: OutboxItem?
    var body: some View {
        NavigationStack {
            List {
                if let error { Section { Text(error).foregroundStyle(.red) } }
                if items.isEmpty { ContentUnavailableView("Outbox is empty", systemImage: "tray", description: Text("Shares are saved here before sending, including when your server is offline.")) }
                ForEach(items.reversed()) { item in
                    DisclosureGroup {
                        Text(item.label).textSelection(.enabled)
                        Text(item.serverURL).font(.caption).foregroundStyle(.secondary)
                        Text("Target: \(item.capability.id)").font(.caption)
                        if let payload = item.payload,
                           let object = try? JSONSerialization.jsonObject(with: payload) as? [String: Any],
                           let text = object["text"] as? String {
                            Text(text).lineLimit(12).textSelection(.enabled)
                        }
                        if let detail = item.detail { Text(detail).foregroundStyle(item.state == .paused ? .orange : .secondary) }
                        if let jobID = item.jobID { Text("Server job: \(jobID)").font(.caption).textSelection(.enabled) }
                        if [.waiting, .paused].contains(item.state) {
                            Text("Expires \(item.expires.formatted())").font(.caption)
                            Button("Retry now") { retry(item.id) }
                        }
                        if [.waiting, .sending, .paused].contains(item.state) {
                            Button("Stop retrying") { edit { try $0.cancel(item.id) } }
                        }
                        Button("Delete", role: .destructive) { pendingDelete = item }
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(item.capability.manifest.title)
                            Text(item.statusLabel).font(.caption).foregroundStyle(item.state == .paused ? .orange : .secondary)
                            if item.state == .sending, let upload = item.upload {
                                UploadProgressBar(progress: upload)
                            }
                            Text(item.created, style: .relative).font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                }
                Section {
                    Text("Up to 50 saved shares / 64 MB. Unsent content expires after seven days. Opening xlatch retries waiting items; background delivery depends on iOS. Stopping retries cannot undo work already accepted by a server.")
                    if let warning = UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.string(forKey: "outbox-background-warning") { Text(warning).foregroundStyle(.orange) }
                }.font(.footnote).foregroundStyle(.secondary)
            }.navigationTitle("Outbox")
                .refreshable { await OutboxDelivery.shared.drain(expedite: true); await reload() }
                .task {
                    while !Task.isCancelled {
                        await reload()
                        do { try await Task.sleep(for: .milliseconds(500)) } catch { break }
                    }
                }
                .confirmationDialog("Delete this saved share? It will not be retried. Work already accepted by the server is unaffected.", isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } })) {
                    Button("Delete share", role: .destructive) {
                        if let item = pendingDelete { edit { try $0.delete(item.id) } }
                        pendingDelete = nil
                    }
                }
        }
    }
    @MainActor private func reload() async {
        do { items = try OutboxStore().items(); error = await OutboxDelivery.shared.lastError }
        catch { self.error = error.localizedDescription }
    }
    private func edit(_ change: (OutboxStore) throws -> Void) {
        do { try change(OutboxStore()); items = try OutboxStore().items(); error = nil }
        catch { self.error = error.localizedDescription }
    }
    private func retry(_ id: String) {
        edit { try $0.retry(id) }
        Task { await OutboxDelivery.shared.drain(id: id); await reload() }
    }
}

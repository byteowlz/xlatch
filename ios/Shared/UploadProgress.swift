import Foundation
import OSLog
import SwiftUI

struct UploadProgress: Codable, Equatable {
    let sent: Int64
    let total: Int64
    init(sent: Int64, total: Int64) {
        self.total = max(0, total)
        self.sent = total > 0 ? min(max(0, sent), total) : max(0, sent)
    }
    var fraction: Double? { total > 0 ? Double(sent) / Double(total) : nil }
    var label: String {
        guard let fraction else { return "Uploading" }
        return sent >= total ? "Waiting for server acceptance" : "Uploading — \(Int(fraction * 100))%"
    }
    var bytesLabel: String {
        let sent = ByteCountFormatter.string(fromByteCount: sent, countStyle: .file)
        guard total > 0 else { return "\(sent) uploaded" }
        return "\(sent) of \(ByteCountFormatter.string(fromByteCount: total, countStyle: .file))"
    }
}

/// The session delegate calls serially; lock also protects test and future callers.
final class UploadReporter {
    private let lock = NSLock()
    private let item: OutboxItem
    private let store: OutboxStore
    private var lastWrite = Date.distantPast
    init(item: OutboxItem, store: OutboxStore) { self.item = item; self.store = store }
    func update(sent: Int64, total: Int64) {
        lock.lock(); defer { lock.unlock() }
        let progress = UploadProgress(sent: sent, total: total)
        let now = Date()
        // Always show upload start and completion, throttle intermediate disk writes.
        guard sent == 0 || (total > 0 && sent >= total) || now.timeIntervalSince(lastWrite) >= 1 else { return }
        do { try store.updateProgress(item, progress: progress); lastWrite = now }
        catch { Logger(subsystem: "com.byteowlz.xlatch", category: "upload").error("Could not update upload progress: \(error.localizedDescription)") }
    }
}

struct UploadProgressBar: View {
    let progress: UploadProgress
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let fraction = progress.fraction { ProgressView(value: fraction).accessibilityLabel("Upload progress") }
            else { ProgressView("Uploading") }
            Text(progress.bytesLabel).font(.caption).foregroundStyle(.secondary)
        }
    }
}

struct LiveUploadProgress: View {
    let id: String
    @State private var item: OutboxItem?
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(item?.statusLabel ?? "Saving to Outbox")
            if item?.state == .sending, let progress = item?.upload { UploadProgressBar(progress: progress) }
            else if item == nil || item?.state == .sending { ProgressView() }
        }.task(id: id) {
            while !Task.isCancelled {
                // Submission surfaces storage errors; progress is optional display-only state.
                item = try? OutboxStore().items().first(where: { $0.id == id })
                do { try await Task.sleep(for: .milliseconds(500)) } catch { break }
            }
        }
    }
}

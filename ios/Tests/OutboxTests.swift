import XCTest
import CryptoKit
@testable import XLatch

final class OutboxTests: XCTestCase {
    private func fixture(maxItems: Int = 50, maxBytes: Int = 64 * 1024 * 1024) throws -> (URL, OutboxStore, Connection, OutboxItem) {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        addTeardownBlock { try FileManager.default.removeItem(at: directory) }
        let store = try OutboxStore(directory: directory, maxItems: maxItems, maxBytes: maxBytes)
        let connection = Connection(url: "https://example.test", pin: "original-pin", deviceID: UUID().uuidString, privateKey: Curve25519.Signing.PrivateKey().rawRepresentation)
        let action = Capability(manifest: Manifest(id: "pi.session", title: "Session", description: "", accepts: ["text/plain", "image/*"]), revision: "approved", status: "active")
        let item = try OutboxItem(input: .text("saved content"), capability: action, connection: connection, now: Date())
        return (directory, store, connection, item)
    }
    func testActivityJoinsReceiptsAndExcludesOtherPairings() throws {
        let (_, _, connection, original) = try fixture()
        var accepted = original
        accepted.state = .sent
        accepted.jobID = "accepted-job"
        let job = Job(id: "accepted-job", capability_id: original.capability.id, status: "succeeded", result: nil, error: nil, created_at: Int64(original.created.timeIntervalSince1970))
        let rows = ActivityEntry.merge(jobs: [job], outbox: [accepted], connection: connection)
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows.first?.status, "Completed")
        XCTAssertEqual(rows.first?.title, "Session")
        let replacement = Connection(url: connection.url, pin: "different-server", deviceID: connection.deviceID, privateKey: connection.privateKey)
        XCTAssertTrue(ActivityEntry.merge(jobs: [], outbox: [original], connection: replacement).isEmpty)
        XCTAssertEqual(ActivityEntry.merge(jobs: [], outbox: [original], connection: connection).first?.id, "outbox:" + original.id)
    }
    func testRestartPreservesPayloadTargetAndDeduplicationID() throws {
        let (directory, store, connection, item) = try fixture()
        _ = try store.enqueue(item)
        let loaded = try XCTUnwrap(OutboxStore(directory: directory).items().first)
        XCTAssertEqual(loaded.id, item.id)
        XCTAssertEqual(loaded.capability, item.capability)
        XCTAssertEqual(loaded.payload, item.payload)
        XCTAssertTrue(try loaded.matches(connection))
        XCTAssertEqual(try loaded.input().payload["text"] as? String, "saved content")
        let bytes = try Data(contentsOf: store.url)
        XCTAssertNil(bytes.range(of: Data(connection.privateKey.base64EncodedString().utf8)))
        XCTAssertTrue(try directory.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup == true)
    }
    func testSeparateConnectionsClaimOnceAndStaleCompletionCannotOverwrite() async throws {
        let (directory, store, _, item) = try fixture()
        _ = try store.enqueue(item)
        let winners = try await withThrowingTaskGroup(of: OutboxItem?.self) { group in
            for _ in 0..<8 { group.addTask { try OutboxStore(directory: directory).claim() } }
            var claimed: [OutboxItem] = []
            for try await result in group { if let result { claimed.append(result) } }
            return claimed
        }
        XCTAssertEqual(winners.count, 1)
        let first = try XCTUnwrap(winners.first)
        let second = try XCTUnwrap(store.claim(now: Date().addingTimeInterval(181)))
        XCTAssertEqual(first.id, second.id); XCTAssertNotEqual(first.lease, second.lease)
        try store.finish(first, error: "late", retry: false)
        XCTAssertEqual(try store.items().first?.lease, second.lease)
        try store.finish(second, error: "offline", retry: true)
        XCTAssertEqual(try store.items().first?.state, .waiting)
    }
    func testQuotaExpiryAndDeletionDoNotResurrectWork() throws {
        let (_, store, connection, item) = try fixture(maxItems: 1)
        _ = try store.enqueue(item)
        let other = try OutboxItem(input: .text("another"), capability: item.capability, connection: connection, now: Date())
        XCTAssertThrowsError(try store.enqueue(other))
        let claimed = try XCTUnwrap(store.claim())
        try store.cancel(item.id)
        try store.finish(claimed, error: "late", retry: true)
        XCTAssertEqual(try store.items().first?.state, .cancelled)
        XCTAssertNil(try store.items().first?.payload)
        try store.delete(item.id)
        try store.finish(claimed, error: "later", retry: true)
        XCTAssertTrue(try store.items().isEmpty)
        _ = try store.enqueue(other)
        XCTAssertNil(try store.claim(now: other.expires.addingTimeInterval(1)))
        let expired = try XCTUnwrap(store.items(now: other.expires.addingTimeInterval(1)).first)
        XCTAssertEqual(expired.state, .expired); XCTAssertNil(expired.payload)
        try store.retry(expired.id, now: other.expires.addingTimeInterval(1))
        XCTAssertNil(try store.claim(now: other.expires.addingTimeInterval(1)))
    }
    func testByteQuotaAndChangedContentRejectBeforeDelivery() throws {
        let (_, small, _, item) = try fixture(maxBytes: 10)
        XCTAssertThrowsError(try small.enqueue(item))
        let (_, store, connection, original) = try fixture()
        _ = try store.enqueue(original)
        let changed = try OutboxItem(id: original.id, input: .text("changed"), capability: original.capability, connection: connection, now: Date())
        XCTAssertThrowsError(try store.enqueue(changed))
    }
    func testLostReplyRetriesSameDurableInvocationAfterRestart() async throws {
        let (directory, store, connection, item) = try fixture()
        _ = try store.enqueue(item)
        let server = DeduplicatingServer()
        let delivery = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connection: { connection }, deliver: { item, _ in try await server.accept(item) }, schedule: {})
        await delivery.drain()
        XCTAssertEqual(try store.items().first?.state, .waiting)
        let restarted = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connection: { connection }, deliver: { item, _ in try await server.accept(item) }, schedule: {})
        await restarted.drain(expedite: true)
        let receipt = try XCTUnwrap(store.items().first)
        XCTAssertEqual(receipt.state, .sent); XCTAssertEqual(receipt.jobID, "one-job")
        XCTAssertNil(receipt.payload)
        let attempts = await server.ids
        XCTAssertEqual(attempts, [item.id, item.id])
        XCTAssertEqual(try store.enqueue(item).jobID, "one-job")
    }
    func testRePairingAndDisabledActionsPauseWithoutInvoking() async throws {
        let (directory, store, connection, item) = try fixture()
        _ = try store.enqueue(item)
        var replacement = connection
        replacement = Connection(url: connection.url, pin: connection.pin, deviceID: connection.deviceID, privateKey: Curve25519.Signing.PrivateKey().rawRepresentation)
        let delivery = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connection: { replacement }, deliver: { _, _ in XCTFail("Must not invoke after re-pairing"); throw URLError(.badURL) }, schedule: {})
        await delivery.drain()
        XCTAssertEqual(try store.items().first?.state, .paused)
        try store.retry(item.id)
        ShareActionPreferences.save([item.capability.id], deviceID: connection.deviceID)
        defer { ShareActionPreferences.save([], deviceID: connection.deviceID) }
        let disabled = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connection: { connection }, deliver: { _, _ in XCTFail("Disabled target invoked"); throw URLError(.badURL) }, schedule: {})
        await disabled.drain()
        XCTAssertEqual(try store.items().first?.state, .paused)
    }
    func testTLSAndPermissionErrorsPauseButNetworkErrorsRetry() async throws {
        XCTAssertTrue(ClientError.isRetryable(URLError(.notConnectedToInternet)))
        XCTAssertTrue(ClientError.isRetryable(URLError(.networkConnectionLost)))
        XCTAssertFalse(ClientError.isRetryable(URLError(.serverCertificateUntrusted)))
        XCTAssertFalse(ClientError.isRetryable(ClientError.delivery("permission denied", retryable: false)))
        let (directory, store, connection, item) = try fixture()
        _ = try store.enqueue(item)
        let delivery = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connection: { connection }, deliver: { _, _ in throw URLError(.serverCertificateUntrusted) }, schedule: {})
        await delivery.drain()
        XCTAssertEqual(try store.items().first?.state, .paused)
        await delivery.drain(expedite: true)
        XCTAssertEqual(try store.items().first?.attempts, 1)
    }
}

private actor DeduplicatingServer {
    var ids: [String] = []
    func accept(_ item: OutboxItem) throws -> Job {
        ids.append(item.id)
        // The server accepted the first request, but the response was lost.
        if ids.count == 1 { throw URLError(.networkConnectionLost) }
        return Job(id: "one-job", capability_id: item.capability.id, status: "queued", result: nil, error: nil, created_at: 0)
    }
}

extension OutboxTests {
    func testUploadProgressIsNotAcceptanceAndOldLeaseCannotOverwriteRetry() throws {
        let (_, store, _, item) = try fixture()
        _ = try store.enqueue(item)
        let first = try XCTUnwrap(store.claim())
        try store.updateProgress(first, progress: UploadProgress(sent: 100, total: 100))
        let uploaded = try XCTUnwrap(store.items().first)
        XCTAssertEqual(uploaded.state, .sending)
        XCTAssertNil(uploaded.jobID)
        XCTAssertEqual(uploaded.statusLabel, "Waiting for server acceptance")
        let retry = try XCTUnwrap(store.claim(now: Date().addingTimeInterval(181)))
        XCTAssertNil(retry.upload)
        try store.updateProgress(first, progress: UploadProgress(sent: 100, total: 100))
        XCTAssertNil(try store.items().first?.upload)
        try store.updateProgress(retry, progress: UploadProgress(sent: 25, total: 100))
        try store.updateProgress(retry, progress: UploadProgress(sent: 10, total: 100))
        XCTAssertEqual(try store.items().first?.upload?.fraction, 0.25)
        try store.cancel(item.id)
        try store.updateProgress(retry, progress: UploadProgress(sent: 100, total: 100))
        XCTAssertEqual(try store.items().first?.state, .cancelled)
    }
    func testUnknownAndOutOfRangeUploadCounts() {
        XCTAssertNil(UploadProgress(sent: 10, total: -1).fraction)
        XCTAssertEqual(UploadProgress(sent: -1, total: 100).fraction, 0)
        XCTAssertEqual(UploadProgress(sent: 120, total: 100).fraction, 1)
        XCTAssertEqual(UploadProgress(sent: 42, total: 100).label, "Uploading — 42%")
    }
}

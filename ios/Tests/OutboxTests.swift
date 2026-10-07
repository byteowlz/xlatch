import XCTest
import UIKit
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
    func testLargeFileSurvivesProviderDeletionAndRestartThenCleansUp() throws {
        let (directory, store, connection, original) = try fixture(maxBytes: 4096)
        let bytes = Data(repeating: 37, count: 9 * 1024 * 1024)
        let input = try ShareInput.file(bytes, name: "large.png", mime: "image/png")
        let source = try XCTUnwrap(input.localFile)
        defer { try? FileManager.default.removeItem(at: source) }
        let item = try OutboxItem(input: input, capability: original.capability, connection: connection, now: Date())
        let queued = try store.enqueue(item)
        try FileManager.default.removeItem(at: source)
        let restored = try XCTUnwrap(OutboxStore(directory: directory).items().first)
        let file = try XCTUnwrap(restored.input().localFile)
        XCTAssertEqual(try Data(contentsOf: file), bytes)
        XCTAssertEqual(restored.id, queued.id)
        XCTAssertLessThan(try XCTUnwrap(restored.payload).count, 1024)
        let claimed = try XCTUnwrap(store.claim(id: queued.id))
        let job = Job(id: "job", capability_id: original.capability.id, status: "queued", result: nil, error: nil, created_at: 0)
        try store.finish(claimed, job: job)
        XCTAssertFalse(FileManager.default.fileExists(atPath: file.path))
        XCTAssertNil(try store.items().first?.localFile)
    }
    func testLargeFileRetryCannotSubstituteEqualSizeContent() throws {
        let (_, store, connection, original) = try fixture()
        let first = try ShareInput.file(Data(repeating: 1, count: 5 * 1024 * 1024), name: "file.png", mime: "image/png")
        let second = try ShareInput.file(Data(repeating: 2, count: 5 * 1024 * 1024), name: "file.png", mime: "image/png")
        defer {
            for input in [first, second] { if let url = input.localFile { try? FileManager.default.removeItem(at: url) } }
        }
        let item = try OutboxItem(input: first, capability: original.capability, connection: connection, now: Date())
        let queued = try store.enqueue(item)
        defer { if let name = queued.localFile, let file = try? SharedFiles.resolve(name) { try? FileManager.default.removeItem(at: file) } }
        let changed = try OutboxItem(id: item.id, input: second, capability: original.capability, connection: connection, now: item.created)
        XCTAssertThrowsError(try store.enqueue(changed))
    }
    func testIconSurvivesOfflineCacheAndMalformedDataFallsBack() throws {
        let (_, _, _, item) = try fixture()
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: 32, height: 32))
        let data = renderer.pngData { context in
            UIColor.systemTeal.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 32, height: 32))
        }
        let darkData = renderer.pngData { context in
            UIColor.white.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 32, height: 32))
        }
        var manifest = item.capability.manifest
        manifest.icon = ActionIcon(png_base64: data.base64EncodedString(), dark_png_base64: darkData.base64EncodedString())
        let action = Capability(manifest: manifest, revision: "icon-revision", status: "active")
        let decoded = try JSONDecoder().decode(Capability.self, from: JSONEncoder().encode(action))
        XCTAssertEqual(decoded, action)
        XCTAssertNotNil(decoded.manifest.icon?.image)
        XCTAssertNotNil(decoded.manifest.icon?.image(for: .dark))
        XCTAssertNil(ActionIcon(png_base64: "invalid").image)
        XCTAssertNil(ActionIcon(png_base64: String(repeating: "A", count: 175001)).image)
        XCTAssertNil(item.capability.manifest.icon)
        let large = UIGraphicsImageRenderer(size: CGSize(width: 512, height: 512)).pngData { context in
            UIColor.systemOrange.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 512, height: 512))
        }
        let imported = try ActionIcon.imported(large)
        XCTAssertLessThanOrEqual(try XCTUnwrap(imported.image).size.width, 128)
        XCTAssertLessThanOrEqual(imported.png_base64.count, 175000)
    }
    func testChainSurvivesRestartAndCannotChangeOnRetry() throws {
        let (directory, store, connection, item) = try fixture()
        let other = Capability(manifest: Manifest(id: "second", title: "Second", description: "", accepts: ["text/plain"]), revision: "pinned", status: "active")
        let chain = [item.capability, other]
        let queued = try OutboxItem(id: item.id, input: item.input(), capability: item.capability, connection: connection, now: item.created, chain: chain)
        _ = try store.enqueue(queued)
        let loaded = try XCTUnwrap(OutboxStore(directory: directory).items().first)
        XCTAssertEqual(loaded.chain, chain)
        XCTAssertEqual(loaded.id, item.id)
        XCTAssertThrowsError(try store.enqueue(item))
        var changed = queued
        changed.chain = [item.capability, item.capability]
        XCTAssertThrowsError(try store.enqueue(changed))
    }
    func testGroupSurvivesRestartAndCannotChangeOnRetry() throws {
        let (directory, store, connection, item) = try fixture()
        let other = Capability(manifest: Manifest(id: "slides", title: "Slides", description: "", accepts: ["text/plain"]), revision: "pinned", status: "active")
        let group = [item.capability, other]
        let queued = try OutboxItem(id: item.id, input: item.input(), capability: item.capability, connection: connection, now: item.created, group: group)
        _ = try store.enqueue(queued)
        let loaded = try XCTUnwrap(OutboxStore(directory: directory).items().first)
        XCTAssertEqual(loaded.group, group)
        XCTAssertEqual(loaded.targetTitle, "Session + Slides")
        XCTAssertThrowsError(try store.enqueue(item))
        var changed = queued
        changed.group = [other, item.capability]
        XCTAssertThrowsError(try store.enqueue(changed))
        XCTAssertThrowsError(try OutboxItem(input: item.input(), capability: item.capability, connection: connection, now: Date(), chain: group, group: group))
    }
    func testActivityJoinsReceiptsAndExcludesOtherPairings() throws {
        let (_, _, connection, original) = try fixture()
        var accepted = original
        accepted.state = .sent
        accepted.jobID = "accepted-job"
        let job = Job(id: "accepted-job", capability_id: original.capability.id, status: "succeeded", result: nil, error: nil, created_at: Int64(original.created.timeIntervalSince1970))
        let rows = ActivityEntry.merge(
            jobs: [job],
            outbox: [accepted],
            connection: connection,
            capabilities: []
        )
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows.first?.status, "Completed")
        XCTAssertEqual(rows.first?.title, "Session")
        let replacement = Connection(url: connection.url, pin: "different-server", deviceID: connection.deviceID, privateKey: connection.privateKey)
        XCTAssertTrue(
            ActivityEntry.merge(
                jobs: [],
                outbox: [original],
                connection: replacement,
                capabilities: []
            ).isEmpty
        )
        let pending = try XCTUnwrap(
            ActivityEntry.merge(
                jobs: [],
                outbox: [original],
                connection: connection,
                capabilities: []
            ).first
        )
        XCTAssertEqual(pending.id, "outbox:" + original.id)
        XCTAssertEqual(pending.outboxID, original.id)
        XCTAssertTrue(pending.canRetry)
        XCTAssertTrue(pending.canStop)
        XCTAssertNil(rows.first?.outboxID)
        XCTAssertFalse(rows.first?.canRetry ?? true)
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
        let delivery = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connections: { [connection] }, deliver: { item, _ in try await server.accept(item) }, schedule: {})
        await delivery.drain()
        XCTAssertEqual(try store.items().first?.state, .waiting)
        let restarted = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connections: { [connection] }, deliver: { item, _ in try await server.accept(item) }, schedule: {})
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
        let delivery = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connections: { [replacement] }, deliver: { _, _ in XCTFail("Must not invoke after re-pairing"); throw URLError(.badURL) }, schedule: {})
        await delivery.drain()
        XCTAssertEqual(try store.items().first?.state, .paused)
        try store.retry(item.id)
        ShareActionPreferences.save([item.capability.id], deviceID: connection.deviceID)
        defer { ShareActionPreferences.save([], deviceID: connection.deviceID) }
        let disabled = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connections: { [connection] }, deliver: { _, _ in XCTFail("Disabled target invoked"); throw URLError(.badURL) }, schedule: {})
        await disabled.drain()
        XCTAssertEqual(try store.items().first?.state, .paused)
    }
    func testActionPresentationOrderAndIconsAreDeviceLocal() throws {
        let (_, _, connection, item) = try fixture()
        let second = Capability(manifest: Manifest(id: "other.action", title: "Other", description: "Other action", accepts: ["text/plain"]), revision: "other-revision", status: "active")
        let first = item.capability
        let otherDevice = UUID().uuidString
        ShareActionPreferences.saveOrder([second.id, first.id], deviceID: connection.deviceID)
        ShareActionPreferences.saveIcon(.system("waveform"), for: second.id, deviceID: connection.deviceID)
        ShareActionPreferences.saveForLaterPreparation(second.id, deviceID: connection.deviceID)
        defer {
            ShareActionPreferences.saveOrder([], deviceID: connection.deviceID)
            ShareActionPreferences.saveIcon(nil, for: second.id, deviceID: connection.deviceID)
            ShareActionPreferences.saveForLaterPreparation(nil, deviceID: connection.deviceID)
        }
        XCTAssertEqual(ShareActionPreferences.ordered([first, second], deviceID: connection.deviceID).map(\.id), [second.id, first.id])
        XCTAssertEqual(ShareActionPreferences.icon(for: second.id, deviceID: connection.deviceID), .system("waveform"))
        XCTAssertEqual(ShareActionPreferences.saveForLaterPreparation(deviceID: connection.deviceID), second.id)
        XCTAssertEqual(ShareActionPreferences.ordered([first, second], deviceID: otherDevice).map(\.id), [first.id, second.id])
        XCTAssertNil(ShareActionPreferences.icon(for: second.id, deviceID: otherDevice))
    }
    func testTLSAndPermissionErrorsPauseButNetworkErrorsRetry() async throws {
        XCTAssertTrue(ClientError.isRetryable(URLError(.notConnectedToInternet)))
        XCTAssertTrue(ClientError.isRetryable(URLError(.networkConnectionLost)))
        XCTAssertFalse(ClientError.isRetryable(URLError(.serverCertificateUntrusted)))
        XCTAssertFalse(ClientError.isRetryable(ClientError.delivery("permission denied", retryable: false)))
        let (directory, store, connection, item) = try fixture()
        _ = try store.enqueue(item)
        let delivery = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connections: { [connection] }, deliver: { _, _ in throw URLError(.serverCertificateUntrusted) }, schedule: {})
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
    func testImmediateJobObservationSurfacesFastFailure() async throws {
        var responses = [
            Job(id: "job", capability_id: "pi.session", status: "queued", result: nil, error: nil, created_at: 0),
            Job(id: "job", capability_id: "pi.session", status: "failed", result: nil, error: "executable hash mismatch", created_at: 0)
        ]
        let terminal = try await ImmediateJobObservation.wait(for: "job", attempts: 3, intervalNanoseconds: 0) { _ in
            responses.removeFirst()
        }
        XCTAssertEqual(terminal?.status, "failed")
        XCTAssertEqual(
            terminal.map(ImmediateJobObservation.failureMessage),
            "This action changed after it was approved. Reload or restart the tool that registered it, then approve the pending revision in xlatch."
        )
    }

    func testImmediateJobObservationLeavesLongJobAsynchronous() async throws {
        var fetches = 0
        let terminal = try await ImmediateJobObservation.wait(for: "job", attempts: 3, intervalNanoseconds: 0) { _ in
            fetches += 1
            return Job(id: "job", capability_id: "slow", status: "running", result: nil, error: nil, created_at: 0)
        }
        XCTAssertNil(terminal)
        XCTAssertEqual(fetches, 3)
    }

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

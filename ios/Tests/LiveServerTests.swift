import XCTest
@testable import XLatch

final class LiveServerTests: XCTestCase {
    func testPinnedPairingSignedInvocationAndResult() async throws {
        guard let code = ProcessInfo.processInfo.environment["XLATCH_TEST_TICKET"], !code.isEmpty else { throw XCTSkip("Requires a fresh test-server enrollment ticket") }
        let issued = try JSONDecoder().decode(PairingTicket.self, from: Data(code.utf8))
        // A dead first address must not prevent pairing through a pinned alternate.
        let ticket = PairingTicket(version: issued.version, url: "https://127.0.0.1:1", pin: issued.pin, token: issued.token, expires_at: issued.expires_at, urls: issued.candidateURLs)
        let previous = try CredentialStore.load()
        defer {
            if let previous { try? CredentialStore.save(previous) }
            else { try? CredentialStore.clear() }
        }
        let connection = try await APIClient.pair(ticket, name: "xlatch simulator test")
        let client = try APIClient(connection: connection)
        let capabilities = try await client.capabilities()
        let capability = try XCTUnwrap(capabilities.first(where: { $0.id == "echo" }))
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = try OutboxStore(directory: directory)
        let item = try store.enqueue(OutboxItem(input: .text("iOS signed round trip"), capability: capability, connection: connection, now: Date()))
        // Simulate a lost acknowledgement: the server accepted this ID, but Outbox still has no receipt.
        let claimed = try XCTUnwrap(store.claim())
        let reporter = UploadReporter(item: claimed, store: store)
        let first = try await client.invoke(capability, input: item.input(), key: item.id, progress: reporter.update)
        XCTAssertEqual(try store.items().first?.upload?.fraction, 1, "URLSession must report all request-body bytes uploaded")
        XCTAssertEqual(try store.items().first?.state, .sending, "Uploaded bytes do not acknowledge a job")
        try store.finish(claimed, error: "Simulated lost acknowledgement", retry: true)
        try store.retry(item.id)
        let delivery = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connections: { [connection] }, schedule: {})
        await delivery.drain()
        XCTAssertEqual(try store.items().first?.jobID, first.id)
        XCTAssertEqual(try store.items().first?.state, .sent)
        XCTAssertNil(try store.items().first?.payload)
        var finished: Job?
        for _ in 0..<50 {
            let job: Job = try await client.rpc(["op": "job", "id": first.id])
            if job.isFinished { finished = job; break }
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertEqual(finished?.status, "succeeded")
        XCTAssertEqual(finished?.result?.text, "iOS signed round trip")
        // The same server must fail when the out-of-band certificate pin is wrong.
        let wrong = Connection(url: connection.url, pin: String(repeating: "0", count: 64), deviceID: connection.deviceID, privateKey: connection.privateKey)
        do { let _: [Capability] = try await APIClient(connection: wrong).rpc(["op": "discover"]); XCTFail("Wrong certificate pin accepted") }
        catch {
            XCTAssertFalse(ClientError.isRetryable(error), "TLS pin rejection must pause Outbox, even when URLSession calls it cancellation")
        }
    }
}

extension LiveServerTests {
    func testLiveTransportRejectsWrongPinAndFallsBackToReachableOrigin() async throws {
        let environment = ProcessInfo.processInfo.environment
        guard let origin = environment["XLATCH_TEST_ORIGIN"], let pin = environment["XLATCH_TEST_PIN"] else {
            throw XCTSkip("Requires a live server origin and public certificate pin")
        }
        let route = try await ServerDiscovery.reachableOrigin(["https://127.0.0.1:1", origin], pin: pin)
        XCTAssertEqual(route.url, origin)
        let url = try XCTUnwrap(URL(string: origin))
        let session = URLSession(configuration: .ephemeral, delegate: PinnedSession(origin: url, pin: String(repeating: "0", count: 64)), delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        do {
            _ = try await session.data(from: url.appendingPathComponent("health"))
            XCTFail("Wrong certificate pin accepted")
        } catch {
            // URLSession can report explicit authentication-challenge rejection as cancellation.
            let failure = error as NSError
            XCTAssertTrue(APIClient.connectionFailure(error).contains("TLS verification failed") ||
                          (failure.domain == NSURLErrorDomain && failure.code == URLError.cancelled.rawValue),
                          "Unexpected failure: \(error)")
        }
    }
}


extension LiveServerTests {
    func testLargeUploadResumesThroughSignedClientAndDurableOutbox() async throws {
        guard let code = ProcessInfo.processInfo.environment["XLATCH_TEST_UPLOAD_TICKET"], !code.isEmpty else { throw XCTSkip("Requires an isolated upload-test server ticket") }
        let ticket = try JSONDecoder().decode(PairingTicket.self, from: Data(code.utf8))
        let connection = try await APIClient.pair(ticket, name: "large upload test")
        let client = try APIClient(connection: connection)
        let capabilities = try await client.capabilities()
        let capability = try XCTUnwrap(capabilities.first(where: { $0.id == "upload.save" }))
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = try OutboxStore(directory: directory)
        let bytes = Data(repeating: 73, count: 9 * 1024 * 1024 + 13)
        let input = try ShareInput.file(bytes, name: "ios-large.bin", mime: "application/octet-stream")
        defer { if let url = input.localFile { try? FileManager.default.removeItem(at: url) } }
        let item = try store.enqueue(OutboxItem(input: input, capability: capability, connection: connection, now: Date()))
        struct Ack: Decodable { let offset: Int }
        let _: Ack = try await client.rpc(["op":"upload", "request":["action":"begin", "id":item.id, "name":"ios-large.bin", "mime_type":"application/octet-stream", "size":bytes.count]])
        let partial: Ack = try await client.rpc(["op":"upload", "request":["action":"chunk", "id":item.id, "offset":0, "data_base64":bytes.prefix(1024 * 1024).base64EncodedString()]])
        XCTAssertEqual(partial.offset, 1024 * 1024)
        let delivery = OutboxDelivery(store: { try OutboxStore(directory: directory) }, connections: { [connection] }, schedule: {})
        await delivery.drain()
        let receipt = try XCTUnwrap(store.items().first)
        XCTAssertEqual(receipt.state, .sent, receipt.detail ?? "No error detail")
        let jobID = try XCTUnwrap(receipt.jobID)
        for _ in 0..<100 {
            let job: Job = try await client.rpc(["op":"job", "id":jobID])
            if job.isFinished { XCTAssertEqual(job.status, "succeeded", job.error ?? ""); return }
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTFail("Uploaded job did not finish")
    }
}

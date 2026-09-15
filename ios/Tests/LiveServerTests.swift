import XCTest
@testable import XLatch

final class LiveServerTests: XCTestCase {
    func testPinnedPairingSignedInvocationAndResult() async throws {
        guard let code = ProcessInfo.processInfo.environment["XLATCH_TEST_TICKET"], !code.isEmpty else { throw XCTSkip("Requires a fresh test-server enrollment ticket") }
        let issued = try JSONDecoder().decode(PairingTicket.self, from: Data(code.utf8))
        // A dead first address must not prevent pairing through a pinned alternate.
        let ticket = PairingTicket(version: issued.version, url: "https://127.0.0.1:1", pin: issued.pin, token: issued.token, expires_at: issued.expires_at, urls: issued.candidateURLs)
        let connection = try await APIClient.pair(ticket, name: "xlatch simulator test")
        let client = try APIClient(connection: connection)
        let capabilities = try await client.capabilities()
        let capability = try XCTUnwrap(capabilities.first(where: { $0.id == "echo" }))
        let first = try await client.invoke(capability, input: .text("iOS signed round trip"), key: UUID().uuidString)
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
        catch { /* Expected TLS rejection. */ }
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

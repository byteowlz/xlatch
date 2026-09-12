import XCTest
@testable import XLatch

final class LiveServerTests: XCTestCase {
    func testPinnedPairingSignedInvocationAndResult() async throws {
        guard let code = ProcessInfo.processInfo.environment["XLATCH_TEST_TICKET"], !code.isEmpty else { throw XCTSkip("Requires a fresh test-server enrollment ticket") }
        let ticket = try JSONDecoder().decode(PairingTicket.self, from: Data(code.utf8))
        let connection = try await APIClient.pair(ticket, name: "CrossLatch simulator test")
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

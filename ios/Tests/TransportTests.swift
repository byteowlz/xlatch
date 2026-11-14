import XCTest
import CryptoKit
import Security
@testable import XLatch

final class TransportTests: XCTestCase {
    private let original = "MIIBTjCB9qADAgECAgEBMAoGCCqGSM49BAMCMBYxFDASBgNVBAMMC3hsYXRjaC10ZXN0MB4XDTI2MDkxNDE0MzIwOFoXDTI2MTAxNDE0MzIwOFowFjEUMBIGA1UEAwwLeGxhdGNoLXRlc3QwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAASIkKjlc7BCBPzcQ3WAaPKq38df4V59Q/j0vW8XTNxglmCBMumd4bXfzqT/FQjcfaMaAXlyt3rfIyk0loHdTY+xozUwMzAPBgNVHREECDAGhwR/AAABMBMGA1UdJQQMMAoGCCsGAQUFBwMBMAsGA1UdDwQEAwIHgDAKBggqhkjOPQQDAgNHADBEAiAPU4zZbX7myOCk3kUhuipS1VKSeoAsTYoBEXkm8o2JsAIgKpAiYhF1Llbg1YrhXzep/vdgWl6TdyIUxAF1TG4nbt8="
    private let renewed = "MIIBTzCB9qADAgECAgECMAoGCCqGSM49BAMCMBYxFDASBgNVBAMMC3hsYXRjaC10ZXN0MB4XDTI2MDkxNDE0MzIwOFoXDTI2MTAxNDE0MzIwOFowFjEUMBIGA1UEAwwLeGxhdGNoLXRlc3QwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAASIkKjlc7BCBPzcQ3WAaPKq38df4V59Q/j0vW8XTNxglmCBMumd4bXfzqT/FQjcfaMaAXlyt3rfIyk0loHdTY+xozUwMzAPBgNVHREECDAGhwR/AAABMBMGA1UdJQQMMAoGCCsGAQUFBwMBMAsGA1UdDwQEAwIHgDAKBggqhkjOPQQDAgNIADBFAiEAo42UvitbmshJu/rURkE3vYNcAEtjLdJ7Dgo/omgEFcoCIGu677Ica3ewNU1tD6ww+hezPZ6uu+4RJIzoo8XjchCq"
    private let impostor = "MIIBTzCB9qADAgECAgEDMAoGCCqGSM49BAMCMBYxFDASBgNVBAMMC3hsYXRjaC10ZXN0MB4XDTI2MDkxNDE0MzIwOFoXDTI2MTAxNDE0MzIwOFowFjEUMBIGA1UEAwwLeGxhdGNoLXRlc3QwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAAQ/+fvpwRaksOJ38fWiLnYMlAS29znQ4AlhN4g6peqOw2YrHztuJsgnGeWK9ue5cbDL0YrIGriCGTQSZbOFNf56ozUwMzAPBgNVHREECDAGhwR/AAABMBMGA1UdJQQMMAoGCCsGAQUFBwMBMAsGA1UdDwQEAwIHgDAKBggqhkjOPQQDAgNIADBFAiEAqrmyzGYOVOm8CYOJ8WutJV8oNWbIUwxV8Y4XBQu5J8ECIFdI2A5X37LwgVlrNSAh8zVaibb1NOajqQ1fYDDVnEnN"
    private func certificate(_ fixture: String) throws -> SecCertificate {
        let data = try XCTUnwrap(Data(base64Encoded: fixture))
        return try XCTUnwrap(SecCertificateCreateWithData(nil, data as CFData))
    }
    private func hash(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
    private func accepts(_ fixture: String, pin: String, keyPin: String?, host: String = "127.0.0.1", expired: Bool = false) throws -> Bool {
        let cert = try certificate(fixture)
        var trust: SecTrust?
        XCTAssertEqual(SecTrustCreateWithCertificates(cert, SecPolicyCreateSSL(true, host as CFString), &trust), errSecSuccess)
        let checked = try XCTUnwrap(trust)
        SecTrustSetVerifyDate(checked, Date(timeIntervalSince1970: expired ? 2200000000 : 1789516800) as CFDate)
        return PinnedSession.accepts(checked, certificate: cert, host: host, pin: pin, keyPin: keyPin)
    }
    func testRenewalRequiresSameKeyAndValidHostnameAndDates() throws {
        let cert = try certificate(original)
        let pin = hash(SecCertificateCopyData(cert) as Data)
        let key = try XCTUnwrap(SecCertificateCopyKey(cert))
        let raw = try XCTUnwrap(SecKeyCopyExternalRepresentation(key, nil) as Data?)
        let keyPin = hash(raw)
        XCTAssertTrue(try accepts(original, pin: pin, keyPin: nil))
        XCTAssertFalse(try accepts(renewed, pin: pin, keyPin: nil))
        XCTAssertTrue(try accepts(renewed, pin: pin, keyPin: keyPin))
        XCTAssertFalse(try accepts(impostor, pin: pin, keyPin: keyPin))
        XCTAssertFalse(try accepts(original, pin: pin, keyPin: String(repeating: "0", count: 64)))
        XCTAssertFalse(try accepts(renewed, pin: pin, keyPin: keyPin, host: "127.0.0.2"))
        XCTAssertFalse(try accepts(renewed, pin: pin, keyPin: keyPin, expired: true))
    }
    func testClientRejectsInsecureOrCredentialBearingOrigins() {
        for url in ["http://127.0.0.1", "https://user@127.0.0.1", "https://127.0.0.1/path", "https://127.0.0.1?token=secret"] {
            XCTAssertThrowsError(try APIClient(connection: Connection(url: url, pin: "", deviceID: "", privateKey: Data())))
        }
    }
}

extension TransportTests {
    func testTailnetTransportExceptionIsScopedAndIdenticalInShareExtension() throws {
        let app = try XCTUnwrap(Bundle.main.infoDictionary?["NSAppTransportSecurity"] as? [String: Any])
        XCTAssertNil(app["NSAllowsArbitraryLoads"])
        XCTAssertEqual(app["NSAllowsLocalNetworking"] as? Bool, true)
        let domains = try XCTUnwrap(app["NSExceptionDomains"] as? [String: [String: Any]])
        XCTAssertEqual(Set(domains.keys), ["100.64.0.0/10"])
        XCTAssertEqual(domains["100.64.0.0/10"]?["NSExceptionAllowsInsecureHTTPLoads"] as? Bool, true)
        XCTAssertEqual(domains["100.64.0.0/10"]?["NSExceptionMinimumTLSVersion"] as? String, "TLSv1.2")
        let path = Bundle.main.bundleURL.appendingPathComponent("PlugIns/XLatchShare.appex/Info.plist")
        let share = try XCTUnwrap(PropertyListSerialization.propertyList(from: Data(contentsOf: path), format: nil) as? [String: Any])
        let shareATS = try XCTUnwrap(share["NSAppTransportSecurity"] as? [String: Any])
        XCTAssertEqual(app as NSDictionary, shareATS as NSDictionary)
    }
}

private final class UploadRPCProtocol: URLProtocol {
    private static let lock = NSLock()
    private static var requests = 0
    private static var failNext = false
    static func reset() { lock.withLock { requests = 0; failNext = false } }
    static func failOneRequest() { lock.withLock { failNext = true } }
    static var count: Int { lock.withLock { requests } }
    override class func canInit(with request: URLRequest) -> Bool { request.url?.host == "upload-fixture.invalid" }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.lock.lock()
        Self.requests += 1
        let fail = Self.failNext; Self.failNext = false
        Self.lock.unlock()
        if fail { client?.urlProtocol(self, didFailWithError: URLError(.networkConnectionLost)); return }
        guard let url = request.url,
              let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"]) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL)); return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(#"{"ok":true}"#.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

private actor UploadRouteCounter {
    private(set) var count = 0
    func resolve(_ connection: Connection) async throws -> ServerDiscovery.ServerRoute {
        count += 1
        return ServerDiscovery.ServerRoute(url: connection.url, addresses: [], keyPin: nil)
    }
}

extension TransportTests {
    func testChunkSequenceDiscoversRouteOnce() async throws {
        UploadRPCProtocol.reset()
        let routes = UploadRouteCounter()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UploadRPCProtocol.self]
        let connection = Connection(url: "https://upload-fixture.invalid", pin: "fixture", deviceID: UUID().uuidString, privateKey: Curve25519.Signing.PrivateKey().rawRepresentation)
        let client = try APIClient(connection: connection, configuration: configuration, routeResolver: { try await routes.resolve($0) })
        for index in 0..<12 {
            let response: [String: Bool] = try await client.rpc(["op": "upload", "request": ["action": "chunk", "offset": index * 1024 * 1024, "data_base64": String(repeating: "A", count: 1024 * 1024)]])
            XCTAssertEqual(response, ["ok": true])
        }
        let resolutions = await routes.count
        XCTAssertEqual(resolutions, 1, "Do not probe all server addresses and rewrite Keychain for each chunk")
        XCTAssertEqual(UploadRPCProtocol.count, 12)
    }
    func testFailedRequestIsNotReplayedAndNextRequestRediscoversRoute() async throws {
        UploadRPCProtocol.reset()
        let routes = UploadRouteCounter()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UploadRPCProtocol.self]
        let connection = Connection(url: "https://upload-fixture.invalid", pin: "fixture", deviceID: UUID().uuidString, privateKey: Curve25519.Signing.PrivateKey().rawRepresentation)
        let client = try APIClient(connection: connection, configuration: configuration, routeResolver: { try await routes.resolve($0) })
        let _: [String: Bool] = try await client.rpc(["op": "upload"])
        UploadRPCProtocol.failOneRequest()
        do { let _: [String: Bool] = try await client.rpc(["op": "upload"]); XCTFail("Expected the network failure") }
        catch { XCTAssertEqual((error as? URLError)?.code, .networkConnectionLost) }
        XCTAssertEqual(UploadRPCProtocol.count, 2, "A failed signed POST must not be replayed automatically")
        let _: [String: Bool] = try await client.rpc(["op": "upload"])
        let resolutions = await routes.count
        XCTAssertEqual(resolutions, 2)
        XCTAssertEqual(UploadRPCProtocol.count, 3)
    }
}

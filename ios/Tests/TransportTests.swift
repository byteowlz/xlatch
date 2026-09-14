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

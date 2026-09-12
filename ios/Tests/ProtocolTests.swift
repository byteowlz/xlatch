import XCTest
import CryptoKit
@testable import XLatch

final class ProtocolTests: XCTestCase {
    func testSigningFramePreservesExactPayload() {
        let bytes = APIClient.signingBytes(deviceID: "phone", timestamp: 123, nonce: "nonce", payload: "{\"op\":\"discover\"}")
        XCTAssertEqual(String(data: bytes, encoding: .utf8), "xlatch.rpc.v1\nphone\n123\nnonce\n{\"op\":\"discover\"}")
    }
    func testTicketRejectsHTTPAndExpiredCodes() {
        let expired = PairingTicket(version: 1, url: "https://localhost:7443", pin: String(repeating: "a", count: 64), token: String(repeating: "b", count: 64), expires_at: 0)
        XCTAssertThrowsError(try expired.validate())
        let insecure = PairingTicket(version: 1, url: "http://localhost:7443", pin: String(repeating: "a", count: 64), token: String(repeating: "b", count: 64), expires_at: Int64(Date().timeIntervalSince1970) + 60)
        XCTAssertThrowsError(try insecure.validate())
    }
    func testContentFilteringAndFileLimit() throws {
        let capability = Capability(manifest: Manifest(id: "audio", title: "Transcribe", description: "Audio to text", accepts: ["audio/*"]), revision: "a", status: "active")
        XCTAssertTrue(capability.accepts("audio/mpeg"))
        XCTAssertFalse(capability.accepts("image/png"))
        XCTAssertThrowsError(try ShareInput.file(Data(count: 4 * 1024 * 1024 + 1), name: "large.wav", mime: "audio/wav"))
    }
    func testDeviceSigningUsesRawEd25519() throws {
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: Data(repeating: 1, count: 32))
        let body = APIClient.signingBytes(deviceID: "test", timestamp: 123, nonce: "0123456789abcdef", payload: "{}")
        let signature = try key.signature(for: body)
        XCTAssertEqual(key.publicKey.rawRepresentation.count, 32)
        XCTAssertEqual(signature.count, 64)
        XCTAssertTrue(key.publicKey.isValidSignature(signature, for: body))
    }
}

import XCTest
import CryptoKit
@testable import XLatch

final class EnrollmentTests: XCTestCase {
    private let payload = "{\"server_id\":\"server\",\"policy_version\":1,\"device_id\":\"candidate\",\"name\":\"Phone\",\"public_key\":\"key\",\"grants\":[[\"echo\",\"revision\"]],\"nonce\":\"nonce\",\"expires_at\":1}"
    func testReviewAndSignatureUseTheSameExactBytes() throws {
        let pending = try PendingEnrollment(payload, serverID: "server")
        XCTAssertEqual(String(data: pending.signingBytes(approve: true), encoding: .utf8), "xlatch.enrollment.decision.v1\napprove\n" + payload)
        XCTAssertNotEqual(pending.signingBytes(approve: true), pending.signingBytes(approve: false))
        XCTAssertEqual(pending.code.count, 12)
        XCTAssertTrue(pending.review.expired)
        let changed = try PendingEnrollment(payload.replacingOccurrences(of: "Phone", with: "Other"), serverID: "server")
        XCTAssertNotEqual(changed.code, pending.code)
    }
    func testReviewRejectsWrongServerAndMalformedGrants() {
        XCTAssertThrowsError(try PendingEnrollment(payload, serverID: "another-server"))
        XCTAssertThrowsError(try PendingEnrollment(payload.replacingOccurrences(of: "[\"echo\",\"revision\"]", with: "[\"echo\"]"), serverID: "server"))
        XCTAssertThrowsError(try PendingEnrollment(payload.replacingOccurrences(of: "\"policy_version\":1", with: "\"policy_version\":2"), serverID: "server"))
    }
    func testSimulatorCannotCreateApprovalKey() async throws {
        guard !ApprovalKey.deviceSupportsApprovals else { throw XCTSkip("Physical device approval needs deliberate user interaction.") }
        let connection = Connection(url: "https://localhost", pin: "pin", deviceID: "test", privateKey: Data())
        do {
            _ = try await ApprovalKey.enable(connection: connection, serverID: "server", token: "token")
            XCTFail("Simulator must not substitute a software approval key")
        } catch { XCTAssertTrue(error.localizedDescription.contains("physical device")) }
    }
}

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
    func testBackupReviewBindsOperationIdentityAndExactBytes() throws {
        let raw = #"{"id":"review","server_id":"server","operation":"add","device_id":"backup","name":"Phone","request_key":"request","public_key":"approval","expires_at":1}"#
        let review = try PendingApprover(raw, serverID: "server")
        XCTAssertEqual(String(data: review.signingBytes(approve: true), encoding: .utf8), "xlatch.approver.decision.v1\napprove\n" + raw)
        XCTAssertNotEqual(review.signingBytes(approve: true), review.signingBytes(approve: false))
        XCTAssertThrowsError(try PendingApprover(raw, serverID: "another-server"))
        XCTAssertThrowsError(try PendingApprover(raw.replacingOccurrences(of: "add", with: "unknown"), serverID: "server"))
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

final class IdentityMigrationTests: XCTestCase {
    func testMigrationPreservesDeviceAndApprovalKeys() throws {
        let old = Connection(url:"https://localhost:7443",pin:String(repeating:"a",count:64),deviceID:"phone",privateKey:Data(repeating:7,count:32),serverID:"server")
        let ticket = ServerIdentityUpdate(purpose:"xlatch.identity",server_id:"server",url:"https://localhost:7443",urls:["https://localhost:7443"],pin:String(repeating:"b",count:64))
        let migrated = try ticket.connection(replacing: old)
        XCTAssertEqual(migrated.privateKey,old.privateKey)
        XCTAssertEqual(migrated.deviceID,old.deviceID)
        XCTAssertEqual(migrated.approvalKeyID,"\(old.pin):phone")
        XCTAssertEqual(migrated.serverID,old.serverID)
        let wrong = ServerIdentityUpdate(purpose:"xlatch.identity",server_id:"attacker",url:ticket.url,urls:ticket.urls,pin:ticket.pin)
        XCTAssertThrowsError(try wrong.connection(replacing:old))
    }
}

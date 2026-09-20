import XCTest
import CryptoKit
@testable import XLatch

final class CapabilityApprovalTests: XCTestCase {
    private let connection = Connection(url: "https://localhost", pin: "pin", deviceID: "approver", privateKey: Data(), serverID: "server")
    private func payload() -> String {
        """
        {"id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "server_id":"server","policy_version":1,"approver_id":"approver","manifest":{"id":"pi.session","title":"Send to Pi","description":"Submit text","accepts":["text/plain"],"execution":{"kind":"command","program":"/tmp/adapter","args":["one-session"],"sha256":"\(String(repeating: "b", count: 64))"},"input_schema":{"type":"object"},"output_schema":{"type":"object"},"timeout_seconds":60},"revision":"\(String(repeating: "a", count: 64))","devices":[{"id":"phone","name":"Phone","public_key":"key"}],"expires_at":1}
        """
    }
    func testCapabilityReviewSignsExactBytesAndSeparatesDecisions() throws {
        let raw = payload()
        let pending = try PendingCapabilityApproval(raw, connection: connection)
        XCTAssertEqual(pending.review.manifest["execution"]?["program"]?.text, "/tmp/adapter")
        XCTAssertTrue(pending.review.expired)
        XCTAssertEqual(String(data: pending.signingBytes(approve: true), encoding: .utf8), "xlatch.capability.decision.v1\napprove\n" + raw)
        let key = P256.Signing.PrivateKey()
        let signature = try key.signature(for: pending.signingBytes(approve: true))
        XCTAssertTrue(key.publicKey.isValidSignature(signature, for: pending.signingBytes(approve: true)))
        XCTAssertFalse(key.publicKey.isValidSignature(signature, for: pending.signingBytes(approve: false)))
        XCTAssertFalse(key.publicKey.isValidSignature(signature, for: Data("xlatch.enrollment.decision.v1\napprove\n\(raw)".utf8)))
        let changed = try PendingCapabilityApproval(raw.replacingOccurrences(of: "one-session", with: "other-session"), connection: connection)
        XCTAssertFalse(key.publicKey.isValidSignature(signature, for: changed.signingBytes(approve: true)))
    }
    func testCompositionReviewPreservesExactSignedContract() throws {
        let raw = payload().replacingOccurrences(of: "\"kind\":\"command\"", with: "\"kind\":\"compose\"")
        let pending = try PendingCapabilityApproval(raw, connection: connection)
        XCTAssertEqual(pending.review.manifest["execution"]?["kind"]?.text, "compose")
        XCTAssertEqual(String(data: pending.signingBytes(approve: true), encoding: .utf8), "xlatch.capability.decision.v1\napprove\n" + raw)
    }
    func testJobDecodesCompositionProgressWithoutBreakingLegacyJobs() throws {
        let raw = #"{"id":"parent","capability_id":"flow","status":"running","created_at":1,"steps":[{"position":0,"job_id":"child","capability_id":"transcribe","revision":"r","status":"succeeded"}]}"#
        let job = try JSONDecoder().decode(Job.self, from: Data(raw.utf8))
        XCTAssertEqual(job.steps?.first?.job_id, "child")
        XCTAssertEqual(job.steps?.first?.status, "succeeded")
    }
    func testCapabilityReviewRejectsWrongContextAndUnknownExecution() {
        for (old, new) in [("\"server\"", "\"different-server\""), ("\"approver\"", "\"another-device\""), ("\"policy_version\":1", "\"policy_version\":2"), ("\"kind\":\"command\"", "\"kind\":\"unknown\"")] {
            XCTAssertThrowsError(try PendingCapabilityApproval(payload().replacingOccurrences(of: old, with: new), connection: connection))
        }
    }
}

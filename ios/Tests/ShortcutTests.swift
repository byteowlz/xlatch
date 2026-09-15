import XCTest
import UIKit
import ImageIO
@testable import XLatch

final class ShortcutTests: XCTestCase {
    @MainActor func testTargetNeverFallsBackAcrossPairingsRevisionsOrDisabledActions() throws {
        let connection = Connection(url: "https://example.test", pin: "pin", deviceID: UUID().uuidString, privateKey: Data())
        let action = Capability(manifest: Manifest(id: "pi.session", title: "Pi", description: "", accepts: ["image/*"]), revision: "first", status: "active")
        let id = XLatchTarget.identifier(connection: connection, capability: action)
        XCTAssertEqual(try QuickSend.resolve(id, connection: connection, capabilities: [action], mime: "image/jpeg"), action)
        let revision = Capability(manifest: action.manifest, revision: "second", status: "active")
        let pending = Capability(manifest: action.manifest, revision: "first", status: "pending")
        let other = Connection(url: connection.url, pin: "pin", deviceID: "other", privateKey: Data())
        XCTAssertThrowsError(try QuickSend.resolve(id, connection: other, capabilities: [action], mime: "image/jpeg"))
        for actions in [[], [revision], [pending]] {
            XCTAssertThrowsError(try QuickSend.resolve(id, connection: connection, capabilities: actions, mime: "image/jpeg"))
        }
        XCTAssertThrowsError(try QuickSend.resolve(id, connection: connection, capabilities: [action], mime: "audio/wav"))
        ShareActionPreferences.save([action.id], deviceID: connection.deviceID)
        defer { ShareActionPreferences.save([], deviceID: connection.deviceID) }
        XCTAssertThrowsError(try QuickSend.resolve(id, connection: connection, capabilities: [action], mime: "image/jpeg"))
    }

    func testContextRetainsFileAndLabelsUntrustedSources() throws {
        let context = CapturedContext(sharedURLs: ["https://x.com/user/status/123", "file:///etc/passwd"], pageTitle: "A post", screenshotOCR: "https://wrong.example", clipboardURL: "https://clipboard.example")
        let file = try ShareInput.file(Data([1, 2, 3]), name: "screenshot.jpg", mime: "image/jpeg")
        let combined = try context.attaching(to: file)
        XCTAssertEqual(Set(combined.payload.keys), ["file", "text", "mime_type"])
        XCTAssertEqual(combined.payload["file"] as? [String: String], file.payload["file"] as? [String: String])
        XCTAssertEqual(combined.mime, "image/jpeg")
        let text = try XCTUnwrap(combined.payload["text"] as? String)
        let json = try XCTUnwrap(String(try XCTUnwrap(text.split(separator: "\n", maxSplits: 1).last)).data(using: .utf8))
        let decoded = try JSONDecoder().decode(CapturedContext.self, from: json)
        XCTAssertEqual(decoded.sharedURLs, ["https://x.com/user/status/123"])
        XCTAssertEqual(decoded.screenshotOCR, "https://wrong.example")
        XCTAssertEqual(decoded.clipboardURL, "https://clipboard.example")
    }

    func testContextBoundsEvenEscapedTextAndRejectsNonWebURLs() throws {
        let large = String(repeating: "\u{0001}", count: 100_000)
        let url = "https://example.test/" + String(repeating: "a", count: 1900)
        let context = CapturedContext(sharedURLs: Array(repeating: url, count: 100), sharedText: large, pageTitle: large, pageText: large, screenshotOCR: large, clipboardURL: url, note: large)
        XCTAssertLessThan(try context.text().count, 100_000)
        for value in ["file:///tmp/a", "javascript:alert(1)", "https://user:password@example.test", "https://example.test/\nsecret", "https://example.test/" + large] {
            XCTAssertNil(CapturedContext.webURL(value))
        }
    }

    func testScreenshotIsResizedAndInvalidDataFails() async throws {
        let data = await MainActor.run {
            let format = UIGraphicsImageRendererFormat(); format.scale = 1
            return UIGraphicsImageRenderer(size: CGSize(width: 2400, height: 1200), format: format).pngData { ctx in
                UIColor.white.setFill(); ctx.fill(CGRect(x: 0, y: 0, width: 2400, height: 1200))
            }
        }
        let capture = try await ScreenshotCapture.prepare(data, extractText: false)
        let source = try XCTUnwrap(CGImageSourceCreateWithData(capture.jpeg as CFData, nil))
        let image = try XCTUnwrap(CGImageSourceCreateImageAtIndex(source, 0, nil))
        XCTAssertEqual(image.width, 2048); XCTAssertEqual(image.height, 1024)
        XCTAssertNil(capture.ocr)
        do {
            _ = try await ScreenshotCapture.prepare(Data("not an image".utf8), extractText: false)
            XCTFail("Invalid image accepted")
        } catch { XCTAssertTrue(error.localizedDescription.contains("supported image")) }
    }
}

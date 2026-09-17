import XCTest
import UniformTypeIdentifiers
@testable import XLatch

final class ShareContentTests: XCTestCase {
    func testFilesAppURLIsSentAsBytes() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".toml")
        let bytes = Data("name = \"file share\"\n".utf8)
        try bytes.write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let provider = NSItemProvider(item: url as NSURL, typeIdentifier: UTType.fileURL.identifier)
        let input = try await ShareContentLoader.load(provider)
        let file = try XCTUnwrap(input.payload["file"] as? [String: String])
        XCTAssertEqual(file["name"], url.lastPathComponent)
        XCTAssertEqual(file["data_base64"], bytes.base64EncodedString())
        XCTAssertNil(input.payload["text"])
        XCTAssertNotEqual(input.mime, "text/uri-list")
    }
    func testGenericURLContainingFileIsSentAsBytes() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".txt")
        try Data("actual content".utf8).write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let input = try await ShareContentLoader.load(NSItemProvider(item: url as NSURL, typeIdentifier: UTType.url.identifier))
        XCTAssertNotNil(input.payload["file"])
        XCTAssertNil(input.payload["text"])
    }

    func testUnreadableFileFailsInsteadOfSharingPath() async {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        do {
            _ = try await ShareContentLoader.load(NSItemProvider(item: url as NSURL, typeIdentifier: UTType.fileURL.identifier))
            XCTFail("An unreadable file must fail, not become a link")
        } catch { }
    }

    func testFileProviderTakesPriorityOverText() {
        let text = NSItemProvider(item: "caption" as NSString, typeIdentifier: UTType.plainText.identifier)
        let file = NSItemProvider(item: URL(fileURLWithPath: "/tmp/attachment.txt") as NSURL, typeIdentifier: UTType.fileURL.identifier)
        XCTAssertTrue(ShareContentLoader.provider(in: [text, file]) === file)
    }

    func testWebURLRemainsALink() async throws {
        let url = try XCTUnwrap(URL(string: "https://example.test/post/123"))
        let provider = NSItemProvider(item: url as NSURL, typeIdentifier: UTType.url.identifier)
        let input = try await ShareContentLoader.load(provider)
        XCTAssertEqual(input.payload["text"] as? String, url.absoluteString)
        XCTAssertEqual(input.mime, "text/uri-list")
        XCTAssertNil(input.payload["file"])
    }
}

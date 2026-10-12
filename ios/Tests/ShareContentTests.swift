import XCTest
import UniformTypeIdentifiers
@testable import XLatch

final class ShareContentTests: XCTestCase {
    func testAdditionalTextPreservesURLAndDoesNotAccumulate() {
        let input = ShareInput.text("https://example.com", mime: "text/uri-list")
        XCTAssertEqual(input.addingText("Summarize this").payload["text"] as? String, "https://example.com\n\nSummarize this")
        XCTAssertEqual(input.addingText("Different note").payload["text"] as? String, "https://example.com\n\nDifferent note")
        XCTAssertEqual(input.addingText(" \n").payload["text"] as? String, "https://example.com")
        XCTAssertEqual(input.addingText("note").mime, input.mime)
    }

    func testAdditionalTextPreservesFileAndStagedPath() throws {
        let input = ShareInput(mime: "application/pdf", label: "document.pdf",
            payload: ["mime_type": "application/pdf", "file": ["name": "document.pdf", "size": 100]],
            localFile: URL(fileURLWithPath: "/tmp/staged-file"))
        let updated = input.addingText("Please review")
        XCTAssertEqual(updated.payload["text"] as? String, "Please review")
        XCTAssertEqual(updated.payload["file"] as? [String: AnyHashable], input.payload["file"] as? [String: AnyHashable])
        XCTAssertEqual(updated.localFile, input.localFile)
        XCTAssertEqual(updated.label, input.label)
        XCTAssertEqual(updated.mime, input.mime)
    }

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

    func testFileRepresentationWinsOverInaccessibleOriginalURL() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".txt")
        let bytes = Data("provider supplied bytes".utf8)
        try bytes.write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let provider = NSItemProvider(item: URL(fileURLWithPath: "/inaccessible/original.txt") as NSURL, typeIdentifier: UTType.fileURL.identifier)
        provider.registerFileRepresentation(forTypeIdentifier: UTType.plainText.identifier, fileOptions: [], visibility: .all) { completion in
            completion(url, false, nil)
            return nil
        }
        let input = try await ShareContentLoader.load(provider)
        let file = try XCTUnwrap(input.payload["file"] as? [String: String])
        XCTAssertEqual(file["data_base64"], bytes.base64EncodedString())
        XCTAssertNil(input.payload["text"])
    }

    func testFileReadStagesLargeFilesAndPreservesEmptyFiles() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: url) }
        try Data(repeating: 42, count: 4 * 1024 * 1024 + 1).write(to: url)
        let large = try ShareInput.file(at: url, mime: "application/octet-stream")
        let staged = try XCTUnwrap(large.localFile)
        defer { try? FileManager.default.removeItem(at: staged) }
        XCTAssertEqual(try Data(contentsOf: staged), try Data(contentsOf: url))
        try Data().write(to: url)
        let input = try ShareInput.file(at: url, mime: "application/octet-stream")
        let file = try XCTUnwrap(input.payload["file"] as? [String: String])
        XCTAssertEqual(file["data_base64"], "")
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


extension ShareContentTests {
    func testMultiplePhotosPreserveEveryAttachmentAndItsBytes() async throws {
        let urls = (0..<3).map { _ in FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".jpg") }
        defer { for url in urls { try? FileManager.default.removeItem(at: url) } }
        for (index, url) in urls.enumerated() { try Data([UInt8(index)]).write(to: url) }
        let providers = urls.map { NSItemProvider(item: $0 as NSURL, typeIdentifier: UTType.fileURL.identifier) }
        let inputs = try await ShareContentLoader.loadAll(providers)
        defer { for input in inputs { if let url = input.localFile { try? FileManager.default.removeItem(at: url) } } }
        XCTAssertEqual(inputs.map(\.label), urls.map(\.lastPathComponent))
        let bytes = try inputs.map { input -> Data in
            if let file = input.localFile { return try Data(contentsOf: file) }
            let file = try XCTUnwrap(input.payload["file"] as? [String: String])
            return try XCTUnwrap(Data(base64Encoded: try XCTUnwrap(file["data_base64"])))
        }
        XCTAssertEqual(bytes, [Data([0]), Data([1]), Data([2])])
    }
}


extension ShareContentTests {
    func testUnreadableSecondAttachmentDoesNotReturnOnlyTheFirst() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".jpg")
        try Data([1]).write(to: url); defer { try? FileManager.default.removeItem(at: url) }
        let providers = [url, url.appendingPathExtension("missing")].map { NSItemProvider(item: $0 as NSURL, typeIdentifier: UTType.fileURL.identifier) }
        do { _ = try await ShareContentLoader.loadAll(providers); XCTFail("Must report the unreadable item") }
        catch { XCTAssertTrue(error.localizedDescription.contains("item 2 of 2")) }
    }
    func testStagedSmallPhotoCanStillReachInlineAdapters() async throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".jpg")
        try Data([1, 2, 3]).write(to: url); defer { try? FileManager.default.removeItem(at: url) }
        let inputs = try await ShareContentLoader.loadAll([NSItemProvider(item: url as NSURL, typeIdentifier: UTType.fileURL.identifier)])
        let input = try XCTUnwrap(inputs.first)
        defer { if let file = input.localFile { try? FileManager.default.removeItem(at: file) } }
        XCTAssertNotNil(input.localFile)
        let inline = try input.inlineFile()
        XCTAssertNil(inline.localFile)
        XCTAssertEqual((inline.payload["file"] as? [String: String])?["data_base64"], Data([1, 2, 3]).base64EncodedString())
    }
}

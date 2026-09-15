import AppIntents
import UIKit
import UniformTypeIdentifiers

struct SendContentIntent: AppIntent {
    static var title: LocalizedStringResource = "Send to xlatch"
    static var description = IntentDescription("Send text, a link or a file to an approved target. Leave Target empty to use your saved quick-send target. Returns the accepted job ID.")
    static var authenticationPolicy: IntentAuthenticationPolicy = .requiresLocalDeviceAuthentication
    @Parameter(title: "Text or URL") var text: String?
    @Parameter(title: "File") var file: IntentFile?
    @Parameter(title: "Target") var target: XLatchTarget?
    @MainActor func perform() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog {
        let input: ShareInput
        if let file {
            var value = try ShareInput.file(file.data, name: file.filename, mime: file.type?.preferredMIMEType ?? "application/octet-stream")
            if let text, !text.isEmpty {
                var payload = value.payload; payload["text"] = text
                value = ShareInput(mime: value.mime, label: value.label, payload: payload)
            }
            input = value
        } else if let text, !text.isEmpty {
            input = .text(text, mime: CapturedContext.webURL(text) == nil ? "text/plain" : "text/uri-list")
        } else { throw ClientError.message("Provide text, a URL or a file to send.") }
        let id = try await QuickSend.send(input, target: target)
        return .result(value: id, dialog: "Accepted by xlatch. Check Activity for the result.")
    }
}

struct SendClipboardIntent: AppIntent {
    static var title: LocalizedStringResource = "Send clipboard to xlatch"
    static var description = IntentDescription("Send clipboard text or a link to your saved quick-send target.")
    static var authenticationPolicy: IntentAuthenticationPolicy = .requiresLocalDeviceAuthentication
    @MainActor func perform() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog {
        guard let text = UIPasteboard.general.string, !text.isEmpty else { throw ClientError.message("Copy text or a link first.") }
        let input = ShareInput.text(text, mime: CapturedContext.webURL(text) == nil ? "text/plain" : "text/uri-list")
        let id = try await QuickSend.send(input, target: nil)
        return .result(value: id, dialog: "Accepted by xlatch. Check Activity for the result.")
    }
}

struct SendScreenContextIntent: AppIntent {
    static var title: LocalizedStringResource = "Send screenshot with context to xlatch"
    static var description = IntentDescription("Accepts a screenshot from Shortcuts. Includes available shared context and optional on-device OCR. Does not capture another app's screen itself.")
    static var authenticationPolicy: IntentAuthenticationPolicy = .requiresLocalDeviceAuthentication
    @Parameter(title: "Screenshot") var screenshot: IntentFile
    @Parameter(title: "Shared URLs") var sourceURLs: [URL]?
    @Parameter(title: "Shared text") var sharedText: String?
    @Parameter(title: "Page title") var pageTitle: String?
    @Parameter(title: "Page or Reader text") var pageText: String?
    @Parameter(title: "Note") var note: String?
    @Parameter(title: "Extract visible text", default: true) var extractText: Bool
    @Parameter(title: "Include clipboard URL", default: false) var includeClipboard: Bool
    @Parameter(title: "Target") var target: XLatchTarget?
    @MainActor func perform() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog {
        let data = screenshot.data
        guard data.count <= 32 * 1024 * 1024 else { throw ClientError.message("This screenshot exceeds the 32 MB capture limit.") }
        let capture = try await ScreenshotCapture.prepare(data, extractText: extractText)
        let context = CapturedContext(sharedURLs: (sourceURLs ?? []).map(\.absoluteString), sharedText: sharedText,
            pageTitle: pageTitle, pageText: pageText, screenshotOCR: capture.ocr,
            clipboardURL: includeClipboard ? UIPasteboard.general.string.flatMap(CapturedContext.webURL) : nil, note: note)
        let input = try context.attaching(to: ShareInput.file(capture.jpeg, name: "screenshot.jpg", mime: "image/jpeg"))
        let id = try await QuickSend.send(input, target: target)
        return .result(value: id, dialog: "Screenshot accepted by xlatch. Check Activity for the result.")
    }
}

struct XLatchShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(intent: SendClipboardIntent(), phrases: ["Send clipboard with \(.applicationName)"], shortTitle: "Send clipboard", systemImageName: "doc.on.clipboard")
        AppShortcut(intent: SendContentIntent(), phrases: ["Send content with \(.applicationName)"], shortTitle: "Send content", systemImageName: "square.and.arrow.up")
    }
}

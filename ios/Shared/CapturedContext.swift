import Foundation

struct CapturedContext: Codable {
    var sharedURLs: [String] = []
    var sharedText: String? = nil
    var pageTitle: String? = nil
    var pageText: String? = nil
    var screenshotOCR: String? = nil
    var clipboardURL: String? = nil
    var note: String? = nil

    static func webURL(_ value: String) -> String? {
        guard value.utf8.count <= 2048, let url = URLComponents(string: value),
              ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
              url.host?.isEmpty == false, value.rangeOfCharacter(from: .controlCharacters) == nil, url.user == nil, url.password == nil else { return nil }
        return value
    }
    func text() throws -> String {
        var bounded = self
        bounded.sharedURLs = Array(sharedURLs.compactMap(Self.webURL).prefix(4))
        bounded.sharedText = sharedText.map { String(String.UnicodeScalarView($0.unicodeScalars.prefix(2000))) }
        bounded.pageTitle = pageTitle.map { String(String.UnicodeScalarView($0.unicodeScalars.prefix(500))) }
        bounded.pageText = pageText.map { String(String.UnicodeScalarView($0.unicodeScalars.prefix(6000))) }
        bounded.screenshotOCR = screenshotOCR.map { String(String.UnicodeScalarView($0.unicodeScalars.prefix(2000))) }
        bounded.clipboardURL = clipboardURL.flatMap(Self.webURL)
        bounded.note = note.map { String(String.UnicodeScalarView($0.unicodeScalars.prefix(500))) }
        let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        return "Captured context. URL/text fields identify their source; OCR is not a verified post link. Long fields may be truncated.\n" + String(decoding: try encoder.encode(bounded), as: UTF8.self)
    }
    func attaching(to input: ShareInput) throws -> ShareInput {
        var payload = input.payload
        payload["text"] = try text()
        return ShareInput(mime: input.mime, label: input.label, payload: payload)
    }
}

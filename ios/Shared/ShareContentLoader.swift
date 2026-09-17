import Foundation
import UniformTypeIdentifiers

enum ShareContentLoader {
    static func provider(in providers: [NSItemProvider]) -> NSItemProvider? {
        providers.first { $0.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) }
            ?? providers.first { $0.hasItemConformingToTypeIdentifier(UTType.url.identifier) || $0.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) }
            ?? providers.first
    }

    static func load(_ provider: NSItemProvider) async throws -> ShareInput {
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            let item = try await provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier)
            guard let url = item as? URL, url.isFileURL else {
                throw ClientError.message("The source app did not provide a readable file URL.")
            }
            return try readFile(url)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
            let item = try await provider.loadItem(forTypeIdentifier: UTType.url.identifier)
            if let url = item as? URL {
                return try url.isFileURL ? readFile(url) : .text(url.absoluteString, mime: "text/uri-list")
            }
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
            let item = try await provider.loadItem(forTypeIdentifier: UTType.plainText.identifier)
            if let text = item as? String { return .text(text) }
            if let data = item as? Data, let text = String(data: data, encoding: .utf8) { return .text(text) }
        }
        guard let typeID = provider.registeredTypeIdentifiers.first(where: { UTType($0)?.conforms(to: .data) == true }) else { throw ClientError.message("This content type is not supported yet.") }
        return try await withCheckedThrowingContinuation { continuation in
            provider.loadFileRepresentation(forTypeIdentifier: typeID) { url, error in
                do {
                    if let error { throw error }
                    guard let url else { throw ClientError.message("The source app did not provide a file.") }
                    // Read while the provider's temporary file is still valid.
                    let input = try readFile(url, mime: UTType(typeID)?.preferredMIMEType)
                    continuation.resume(returning: input)
                } catch { continuation.resume(throwing: error) }
            }
        }
    }

    private static func readFile(_ url: URL, mime: String? = nil) throws -> ShareInput {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        var coordinationError: NSError?
        var result: Result<ShareInput, Error>?
        NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &coordinationError) { readableURL in
            result = Result {
                let contentType = try readableURL.resourceValues(forKeys: [.contentTypeKey]).contentType
                let resolvedMIME = mime ?? contentType?.preferredMIMEType
                    ?? UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
                return try ShareInput.file(at: readableURL, mime: resolvedMIME)
            }
        }
        if let coordinationError { throw coordinationError }
        guard let result else { throw ClientError.message("The source app did not provide a readable file.") }
        return try result.get()
    }
}

import Foundation
import UniformTypeIdentifiers

enum ShareContentLoader {
    static func provider(in providers: [NSItemProvider]) -> NSItemProvider? {
        providers.first { $0.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) }
            ?? providers.first { $0.hasItemConformingToTypeIdentifier(UTType.url.identifier) || $0.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) }
            ?? providers.first
    }

    static func load(_ provider: NSItemProvider) async throws -> ShareInput {
        do { return try await loadContent(provider) }
        catch {
            throw ClientError.message(error.localizedDescription + "\n[DEBUG-c487] Types: " + provider.registeredTypeIdentifiers.joined(separator: ", "))
        }
    }

    private static func diagnostic(_ stage: String, _ error: Error) -> Error {
        let code = error as NSError
        return ClientError.message("\(error.localizedDescription)\n[DEBUG-c487] \(stage): \(code.domain)/\(code.code)")
    }

    private static func loadContent(_ provider: NSItemProvider) async throws -> ShareInput {
        let contentType = provider.registeredTypeIdentifiers.first {
            guard let type = UTType($0) else { return false }
            return type.conforms(to: .data) && !type.conforms(to: .url)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier), let contentType {
            return try await loadFile(provider, typeID: contentType)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
            // Consume file URLs inside the provider callback: its access grant may
            // no longer be valid after an async loadItem returns to our task.
            let typeID = provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier)
                ? UTType.fileURL.identifier : UTType.url.identifier
            return try await withCheckedThrowingContinuation { continuation in
                provider.loadItem(forTypeIdentifier: typeID, options: nil) { item, error in
                    do {
                        if let error { throw diagnostic("loadItem", error) }
                        guard let url = item as? URL else {
                            throw ClientError.message("The source app did not provide a readable URL.")
                        }
                        let input = try url.isFileURL ? readFile(url) : .text(url.absoluteString, mime: "text/uri-list")
                        continuation.resume(returning: input)
                    } catch { continuation.resume(throwing: error) }
                }
            }
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
            let item = try await provider.loadItem(forTypeIdentifier: UTType.plainText.identifier)
            if let text = item as? String { return .text(text) }
            if let data = item as? Data, let text = String(data: data, encoding: .utf8) { return .text(text) }
        }
        guard let typeID = provider.registeredTypeIdentifiers.first(where: { UTType($0)?.conforms(to: .data) == true }) else { throw ClientError.message("This content type is not supported yet.") }
        return try await loadFile(provider, typeID: typeID)
    }

    private static func loadFile(_ provider: NSItemProvider, typeID: String) async throws -> ShareInput {
        return try await withCheckedThrowingContinuation { continuation in
            provider.loadFileRepresentation(forTypeIdentifier: typeID) { url, error in
                do {
                    if let error { throw diagnostic("file representation", error) }
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
                // File providers can grant byte access while denying resource metadata.
                let resolvedMIME = mime
                    ?? UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
                do { return try ShareInput.file(at: readableURL, mime: resolvedMIME) }
                catch { throw diagnostic("file read; scope=\(scoped)", error) }
            }
        }
        if let coordinationError { throw diagnostic("coordination; scope=\(scoped)", coordinationError) }
        guard let result else { throw ClientError.message("The source app did not provide a readable file.") }
        return try result.get()
    }
}

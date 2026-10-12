import Foundation
import UniformTypeIdentifiers

enum ShareContentLoader {
    static func loadAll(_ providers: [NSItemProvider]) async throws -> [ShareInput] {
        let files = providers.filter { provider in
            provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier)
                || provider.registeredTypeIdentifiers.contains { id in
                    guard let type = UTType(id) else { return false }
                    return type.conforms(to: .data) && !type.conforms(to: .url) && !type.conforms(to: .text)
                }
        }
        let selected = files.isEmpty ? providers : files
        guard !selected.isEmpty, selected.count <= 100 else { throw ClientError.message("Share between 1 and 100 items at a time.") }
        var inputs: [ShareInput] = []
        do {
            // Load sequentially and spill small files to disk too: a photo batch must not accumulate base64 in memory.
            for provider in selected { inputs.append(try stage(try await load(provider))) }
            return inputs
        } catch {
            for input in inputs { if let file = input.localFile { try? FileManager.default.removeItem(at: file) } }
            throw ClientError.message("Could not read item \(inputs.count + 1) of \(selected.count). " + error.localizedDescription)
        }
    }
    private static func stage(_ input: ShareInput) throws -> ShareInput {
        guard input.localFile == nil, var metadata = input.payload["file"] as? [String: Any],
              let encoded = metadata.removeValue(forKey: "data_base64") as? String,
              let bytes = Data(base64Encoded: encoded) else { return input }
        let file = try SharedFiles.directory().appendingPathComponent(UUID().uuidString)
        do {
            try bytes.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            try SharedFiles.protect(file)
        } catch { try? FileManager.default.removeItem(at: file); throw error }
        metadata["size"] = bytes.count
        var payload = input.payload; payload["file"] = metadata
        return ShareInput(mime: input.mime, label: input.label, payload: payload, localFile: file)
    }
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

    static func readFile(_ url: URL, mime: String? = nil) throws -> ShareInput {
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

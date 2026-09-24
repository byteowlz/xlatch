import Foundation
import SwiftUI
import ImageIO
import UniformTypeIdentifiers

struct Capability: Codable, Identifiable, Hashable {
    let manifest: Manifest
    let revision: String
    let status: String
    var id: String { manifest.id }
    var contentLabel: String {
        let labels = manifest.accepts.map { mime in
            if mime.hasPrefix("image/") { return "Images" }
            if mime.hasPrefix("audio/") { return "Audio" }
            if mime.hasPrefix("video/") { return "Video" }
            switch mime {
            case "text/plain": return "Text"
            case "text/uri-list": return "Links"
            case "application/pdf": return "PDFs"
            default: return "Files"
            }
        }
        var seen = Set<String>()
        return labels.filter { seen.insert($0).inserted }.joined(separator: " · ")
    }
    func accepts(_ mime: String) -> Bool {
        manifest.accepts.contains(mime) || manifest.accepts.contains("*/*") || manifest.accepts.contains(String(mime.split(separator: "/").first ?? "") + "/*")
    }
}

struct Manifest: Codable, Hashable {
    let id: String
    let title: String
    let description: String
    let accepts: [String]
    var file_input: String? = nil
    var icon: ActionIcon? = nil
    var execution: ExecutionKind? = nil
    struct ExecutionKind: Codable, Hashable { let kind: String }
}

struct Job: Codable, Identifiable {
    let id: String
    let capability_id: String
    let status: String
    let result: JSONValue?
    let error: String?
    let created_at: Int64
    var steps: [CompositionStep]? = nil
    var isFinished: Bool { ["succeeded", "failed", "cancelled"].contains(status) }
    var statusLabel: String {
        switch status {
        case "queued": "Waiting"
        case "running": "Working"
        case "succeeded": "Ready"
        case "failed": "Failed"
        case "cancelled": "Cancelled"
        default: status.capitalized
        }
    }
}

struct CompositionStep: Codable, Identifiable {
    let position: Int
    let job_id: String
    let capability_id: String
    let revision: String
    let status: String
    let error: String?
    var id: String { job_id }
}

enum JSONValue: Codable, Equatable {
    case object([String: JSONValue]), array([JSONValue]), string(String), number(Double), bool(Bool), null
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let value = try? c.decode(Bool.self) { self = .bool(value) }
        else if let value = try? c.decode(String.self) { self = .string(value) }
        else if let value = try? c.decode(Double.self) { self = .number(value) }
        else if let value = try? c.decode([String: JSONValue].self) { self = .object(value) }
        else { self = .array(try c.decode([JSONValue].self)) }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .object(let value): try c.encode(value)
        case .array(let value): try c.encode(value)
        case .string(let value): try c.encode(value)
        case .number(let value): try c.encode(value)
        case .bool(let value): try c.encode(value)
        case .null: try c.encodeNil()
        }
    }
    var text: String? {
        if case .string(let s) = self { return s }
        if case .object(let obj) = self { return obj["text"]?.text }
        return nil
    }
    var pretty: String {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(self), let text = String(data: data, encoding: .utf8) else { return "No result" }
        return text
    }
    subscript(_ key: String) -> JSONValue? {
        if case .object(let obj) = self { return obj[key] }
        return nil
    }
}

struct PairingTicket: Codable {
    let version: Int
    let url: String
    let pin: String
    let token: String
    let expires_at: Int64
    var urls: [String]? = nil
    var candidateURLs: [String] { [url] + (urls ?? []) }
    func validate() throws {
        guard version == 1, candidateURLs.count <= 9,
              pin.count == 64, pin.allSatisfy({ $0.isHexDigit }), token.count == 64,
              expires_at >= Int64(Date().timeIntervalSince1970) else {
            throw ClientError.message("This pairing code is invalid or expired. Generate a new code on your server.")
        }
        for address in candidateURLs {
            guard let endpoint = URLComponents(string: address), endpoint.scheme == "https",
                  endpoint.host != nil, endpoint.user == nil, endpoint.password == nil,
                  endpoint.query == nil, endpoint.fragment == nil, ["", "/"].contains(endpoint.path) else {
                throw ClientError.message("This pairing code contains an invalid server address.")
            }
        }
    }
}

struct Connection: Codable {
    let url: String
    let pin: String
    let deviceID: String
    let privateKey: Data
    var urls: [String]? = nil
    var serverID: String? = nil
    var approvalKeyID: String? = nil
    var keyPin: String? = nil
}

enum ClientError: LocalizedError {
    case message(String)
    case delivery(String, retryable: Bool)
    var errorDescription: String? {
        switch self { case .message(let text), .delivery(let text, _): return text }
    }
}

struct ShareInput {
    let mime: String
    let label: String
    let payload: [String: Any]
    var localFile: URL? = nil
    static func text(_ text: String, mime: String = "text/plain") -> ShareInput {
        ShareInput(mime: mime, label: text, payload: ["text": text, "mime_type": mime])
    }
    static func file(at url: URL, mime: String) throws -> ShareInput {
        let handle = try FileHandle(forReadingFrom: url)
        defer { handle.closeFile() }
        let size = try handle.seekToEnd()
        try handle.seek(toOffset: 0)
        if size > 4 * 1024 * 1024 {
            let target = try SharedFiles.copy(handle, size: size)
            return ShareInput(mime: mime, label: url.lastPathComponent,
                payload: ["mime_type": mime, "file": ["name": url.lastPathComponent, "mime_type": mime, "size": size]], localFile: target)
        }
        let limit = 4 * 1024 * 1024
        var data = Data()
        while data.count <= limit {
            guard let chunk = try handle.read(upToCount: min(64 * 1024, limit + 1 - data.count)), !chunk.isEmpty else { break }
            data.append(chunk)
        }
        return try file(data, name: url.lastPathComponent, mime: mime)
    }
    static func file(_ data: Data, name: String, mime: String) throws -> ShareInput {
        if data.count > 4 * 1024 * 1024 {
            let target = try SharedFiles.directory().appendingPathComponent(UUID().uuidString)
            try data.write(to: target, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            try SharedFiles.protect(target)
            return ShareInput(mime: mime, label: name, payload: ["mime_type": mime, "file": ["name": name, "mime_type": mime, "size": data.count]], localFile: target)
        }
        return ShareInput(mime: mime, label: name, payload: ["mime_type": mime, "file": ["name": name, "mime_type": mime, "data_base64": data.base64EncodedString()]])
    }
}


enum ShareActionPreferences {
    private static let suite = "group.com.byteowlz.xlatch"

    static func disabled(deviceID: String) -> Set<String> {
        Set(UserDefaults(suiteName: suite)?.stringArray(forKey: "disabled-actions.\(deviceID)") ?? [])
    }
    static func save(_ disabled: Set<String>, deviceID: String) {
        UserDefaults(suiteName: suite)?.set(disabled.sorted(), forKey: "disabled-actions.\(deviceID)")
    }
    static func ordered(_ capabilities: [Capability], deviceID: String) -> [Capability] {
        let order = presentation(deviceID: deviceID).order
        let ranks = Dictionary(uniqueKeysWithValues: order.enumerated().map { ($0.element, $0.offset) })
        return capabilities.enumerated().sorted { left, right in
            let leftRank = ranks[left.element.id] ?? Int.max
            let rightRank = ranks[right.element.id] ?? Int.max
            return leftRank == rightRank ? left.offset < right.offset : leftRank < rightRank
        }.map(\.element)
    }
    static func saveOrder(_ order: [String], deviceID: String) {
        var settings = presentation(deviceID: deviceID)
        settings.order = order.reduce(into: []) { result, id in
            if !result.contains(id) { result.append(id) }
        }
        save(settings, deviceID: deviceID)
    }
    static func icon(for capabilityID: String, deviceID: String) -> ActionIconOverride? {
        presentation(deviceID: deviceID).icons[capabilityID]
    }
    static func saveIcon(_ icon: ActionIconOverride?, for capabilityID: String, deviceID: String) {
        var settings = presentation(deviceID: deviceID)
        settings.icons[capabilityID] = icon
        save(settings, deviceID: deviceID)
    }
    private static func presentation(deviceID: String) -> ActionPresentationSettings {
        guard let data = UserDefaults(suiteName: suite)?.data(forKey: "action-presentation.\(deviceID)"),
              let settings = try? JSONDecoder().decode(ActionPresentationSettings.self, from: data) else { return ActionPresentationSettings() }
        return settings
    }
    private static func save(_ settings: ActionPresentationSettings, deviceID: String) {
        guard let data = try? JSONEncoder().encode(settings) else { return }
        UserDefaults(suiteName: suite)?.set(data, forKey: "action-presentation.\(deviceID)")
    }
}

private struct ActionPresentationSettings: Codable {
    var order: [String] = []
    var icons: [String: ActionIconOverride] = [:]
}

extension ClientError {
    static func isRetryable(_ error: Error) -> Bool {
        if error is CancellationError { return true }
        if case let ClientError.delivery(_, retryable) = error { return retryable }
        let value = error as NSError
        guard value.domain == NSURLErrorDomain else { return false }
        return [.timedOut, .cannotFindHost, .cannotConnectToHost, .networkConnectionLost, .dnsLookupFailed,
                .notConnectedToInternet, .internationalRoamingOff, .dataNotAllowed, .cancelled].contains(URLError.Code(rawValue: value.code))
    }
}


struct ActionIcon: Codable, Hashable {
    let png_base64: String
    var image: UIImage? {
        guard png_base64.count <= 175000, let data = Data(base64Encoded: png_base64), data.count <= 131072,
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? Int,
              let height = properties[kCGImagePropertyPixelHeight] as? Int,
              (1...256).contains(width), (1...256).contains(height) else { return nil }
        return UIImage(data: data)
    }
    static func imported(_ data: Data) throws -> ActionIcon {
        guard data.count <= 20 * 1024 * 1024,
              let source = CGImageSourceCreateWithData(data as CFData, nil) else {
            throw ClientError.message("Choose an image smaller than 20 MB.")
        }
        for size in [128, 96, 64] {
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: size
            ]
            guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary),
                  let png = UIImage(cgImage: image).pngData(), png.count <= 131_072 else { continue }
            return ActionIcon(png_base64: png.base64EncodedString())
        }
        throw ClientError.message("That image could not be reduced to a usable action icon.")
    }
}

struct ActionIconOverride: Codable, Hashable {
    var systemName: String?
    var image: ActionIcon?
    static func system(_ name: String) -> ActionIconOverride { ActionIconOverride(systemName: name, image: nil) }
    static func custom(_ icon: ActionIcon) -> ActionIconOverride { ActionIconOverride(systemName: nil, image: icon) }
}

struct CapabilityIcon: View {
    let icon: ActionIcon?
    var override: ActionIconOverride? = nil
    var size: CGFloat = 32
    var body: some View {
        Group {
            if let image = override?.image?.image { Image(uiImage: image).resizable().scaledToFit() }
            else if let systemName = override?.systemName { Image(systemName: systemName).resizable().scaledToFit().foregroundStyle(.tint) }
            else if let image = icon?.image { Image(uiImage: image).resizable().scaledToFit() }
            else { Image(systemName: "bolt.fill").foregroundStyle(.tint) }
        }.frame(width: size, height: size).accessibilityHidden(true)
    }
}


/// Private disk-backed content shared by the app and extension, never sent as a host path.
enum SharedFiles {
    static func directory() throws -> URL {
        guard let root = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.com.byteowlz.xlatch") else { throw ClientError.message("Shared file storage is unavailable.") }
        let directory = root.appendingPathComponent("Outbox/Files", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
        try protect(directory)
        return directory
    }
    static func copy(_ source: FileHandle, size: UInt64) throws -> URL {
        let target = try directory().appendingPathComponent(UUID().uuidString)
        guard FileManager.default.createFile(atPath: target.path, contents: nil, attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication]) else { throw ClientError.message("Could not save the shared file. Check available storage.") }
        do {
            try protect(target)
            let output = try FileHandle(forWritingTo: target); defer { try? output.close() }
            var remaining = size
            while remaining > 0 {
                guard let chunk = try source.read(upToCount: Int(min(remaining, 1024 * 1024))), !chunk.isEmpty else { throw ClientError.message("The source file changed while being shared.") }
                try output.write(contentsOf: chunk)
                remaining -= UInt64(chunk.count)
            }
            try output.synchronize()
            return target
        } catch {
            try? FileManager.default.removeItem(at: target)
            throw error
        }
    }
    static func protect(_ url: URL) throws {
        try FileManager.default.setAttributes([.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication], ofItemAtPath: url.path)
        var protected = url; var values = URLResourceValues(); values.isExcludedFromBackup = true
        try protected.setResourceValues(values)
    }
    static func resolve(_ name: String) throws -> URL {
        guard UUID(uuidString: name) != nil else { throw ClientError.message("Invalid saved file identity.") }
        return try directory().appendingPathComponent(name)
    }
}

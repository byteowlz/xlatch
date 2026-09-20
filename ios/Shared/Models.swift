import Foundation
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

enum JSONValue: Codable {
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
    static func text(_ text: String, mime: String = "text/plain") -> ShareInput {
        ShareInput(mime: mime, label: text, payload: ["text": text, "mime_type": mime])
    }
    static func file(at url: URL, mime: String) throws -> ShareInput {
        let handle = try FileHandle(forReadingFrom: url)
        defer { handle.closeFile() }
        let limit = 4 * 1024 * 1024
        var data = Data()
        while data.count <= limit {
            guard let chunk = try handle.read(upToCount: min(64 * 1024, limit + 1 - data.count)), !chunk.isEmpty else { break }
            data.append(chunk)
        }
        return try file(data, name: url.lastPathComponent, mime: mime)
    }
    static func file(_ data: Data, name: String, mime: String) throws -> ShareInput {
        guard data.count <= 4 * 1024 * 1024 else { throw ClientError.message("This file is larger than the 4 MB limit in this first version.") }
        return ShareInput(mime: mime, label: name, payload: ["mime_type": mime, "file": ["name": name, "mime_type": mime, "data_base64": data.base64EncodedString()]])
    }
}


enum ShareActionPreferences {
    static func disabled(deviceID: String) -> Set<String> {
        Set(UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.stringArray(forKey: "disabled-actions.\(deviceID)") ?? [])
    }
    static func save(_ disabled: Set<String>, deviceID: String) {
        UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.set(disabled.sorted(), forKey: "disabled-actions.\(deviceID)")
    }
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

import Foundation
import CryptoKit
import Security

final class PinnedSession: NSObject, URLSessionDelegate, URLSessionTaskDelegate, @unchecked Sendable {
    private let origin: URL
    private let pin: String
    init(origin: URL, pin: String) { self.origin = origin; self.pin = pin.lowercased() }
    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge, completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              challenge.protectionSpace.host == origin.host,
              let trust = challenge.protectionSpace.serverTrust,
              let certificates = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
              let certificate = certificates.first else {
            completionHandler(.cancelAuthenticationChallenge, nil); return
        }
        let data = SecCertificateCopyData(certificate) as Data
        let fingerprint = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        guard fingerprint == pin else { completionHandler(.cancelAuthenticationChallenge, nil); return }
        SecTrustSetAnchorCertificates(trust, [certificate] as CFArray)
        SecTrustSetAnchorCertificatesOnly(trust, true)
        SecTrustSetPolicies(trust, SecPolicyCreateSSL(true, origin.host as CFString?))
        guard SecTrustEvaluateWithError(trust, nil) else { completionHandler(.cancelAuthenticationChallenge, nil); return }
        completionHandler(.useCredential, URLCredential(trust: trust))
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}

enum CredentialStore {
    static var query: [String: Any] {
        var query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "com.byteowlz.xlatch.connection", kSecAttrAccount as String: "paired-device"]
        if let group = Bundle.main.object(forInfoDictionaryKey: "XLatchKeychainGroup") as? String { query[kSecAttrAccessGroup as String] = group }
        return query
    }
    static func load() throws -> Connection? {
        var query = query; query[kSecReturnData as String] = true; query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw ClientError.message("Unlock your phone to access its pairing key. (\(status))") }
        return try JSONDecoder().decode(Connection.self, from: data)
    }
    static func save(_ connection: Connection) throws {
        let data = try JSONEncoder().encode(connection)
        let status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var insert = query; insert[kSecValueData as String] = data
            insert[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            guard SecItemAdd(insert as CFDictionary, nil) == errSecSuccess else { throw ClientError.message("Could not save the device key securely.") }
        } else if status != errSecSuccess { throw ClientError.message("Could not update the device key. (\(status))") }
    }
    static func clear() throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw ClientError.message("Could not remove the pairing key.") }
        UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.removeObject(forKey: "capabilities")
    }
}

final class APIClient {
    let connection: Connection
    private let session: URLSession
    init(connection: Connection) throws {
        guard let url = URL(string: connection.url), url.scheme == "https" else { throw ClientError.message("Invalid server address.") }
        self.connection = connection
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 20
        config.timeoutIntervalForResource = 30
        config.httpShouldSetCookies = false
        self.session = URLSession(configuration: config, delegate: PinnedSession(origin: url, pin: connection.pin), delegateQueue: nil)
    }
    deinit { session.invalidateAndCancel() }
    static func pair(_ ticket: PairingTicket, name: String) async throws -> Connection {
        try ticket.validate()
        let key = Curve25519.Signing.PrivateKey()
        let publicKey = key.publicKey.rawRepresentation.base64EncodedString()
        let message = "xlatch.pair.v1\n\(ticket.token)\n\(publicKey)\n\(name)"
        let signature = try key.signature(for: Data(message.utf8)).base64EncodedString()
        let connection = Connection(url: ticket.url, pin: ticket.pin, deviceID: "", privateKey: key.rawRepresentation)
        let client = try APIClient(connection: connection)
        struct Enrolled: Decodable { let id: String }
        let response: Enrolled = try await client.post("v1/pair", body: ["token": ticket.token, "name": name, "public_key": publicKey, "signature": signature])
        let saved = Connection(url: ticket.url, pin: ticket.pin, deviceID: response.id, privateKey: key.rawRepresentation)
        try CredentialStore.save(saved)
        return saved
    }
    func rpc<T: Decodable>(_ payload: [String: Any]) async throws -> T {
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys, .withoutEscapingSlashes])
        guard let text = String(data: data, encoding: .utf8) else { throw ClientError.message("Could not encode the request.") }
        let nonce = UUID().uuidString.replacingOccurrences(of: "-", with: "")
        let timestamp = Int64(Date().timeIntervalSince1970)
        let bytes = Self.signingBytes(deviceID: connection.deviceID, timestamp: timestamp, nonce: nonce, payload: text)
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: connection.privateKey)
        return try await post("v1/rpc", body: ["device_id": connection.deviceID, "timestamp": timestamp, "nonce": nonce, "payload": text, "signature": try key.signature(for: bytes).base64EncodedString()])
    }
    static func signingBytes(deviceID: String, timestamp: Int64, nonce: String, payload: String) -> Data {
        Data("xlatch.rpc.v1\n\(deviceID)\n\(timestamp)\n\(nonce)\n\(payload)".utf8)
    }
    func invoke(_ capability: Capability, input: ShareInput, key: String) async throws -> Job {
        try await rpc(["op": "invoke", "capability_id": capability.id, "revision": capability.revision, "input": input.payload, "idempotency_key": key])
    }
    func capabilities() async throws -> [Capability] {
        let capabilities: [Capability] = try await rpc(["op": "discover"])
        let data = try JSONEncoder().encode(capabilities)
        UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.set(data, forKey: "capabilities")
        return capabilities
    }
    static func cachedCapabilities() -> [Capability] {
        guard let data = UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.data(forKey: "capabilities"), let values = try? JSONDecoder().decode([Capability].self, from: data) else { return [] }
        return values
    }
    private func post<T: Decodable>(_ path: String, body: [String: Any]) async throws -> T {
        guard let base = URL(string: connection.url) else { throw ClientError.message("Invalid server address.") }
        var request = URLRequest(url: base.appendingPathComponent(path))
        request.httpMethod = "POST"; request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        guard let size = request.httpBody?.count, size <= 8 * 1024 * 1024 else { throw ClientError.message("This request exceeds the 8 MB limit.") }
        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse, (200..<300).contains(response.statusCode) else {
            let message = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["error"] as? String
            throw ClientError.message(message ?? "The server could not accept this request. Try again when it is reachable.")
        }
        return try JSONDecoder().decode(T.self, from: data)
    }
}

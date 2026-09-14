import Foundation

enum ServerDiscovery {
    struct ServerRoute {
        let url: String
        let addresses: [String]
        let keyPin: String?
    }
    private struct Health: Decodable {
        let name: String
        let version: Int
        let urls: [String]?
        let key_pin: String?
    }
    private enum OriginProbe {
        case reachable(ServerRoute)
        case failed(String)
    }
    private static func probe(_ address: String, pin: String, keyPin: String?) async -> OriginProbe {
        do {
            let client = try APIClient(connection: Connection(url: address, pin: pin, deviceID: "", privateKey: Data(), keyPin: keyPin))
            guard let base = URL(string: address) else { return .failed("Invalid server address") }
            var request = URLRequest(url: base.appendingPathComponent("health"))
            request.timeoutInterval = 8
            let (data, response) = try await client.session.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                return .failed("\(address): unexpected health response")
            }
            let health = try JSONDecoder().decode(Health.self, from: data)
            guard health.name == "xlatch", health.version == 1 else { throw ClientError.message("Unexpected server") }
            let addresses = health.urls ?? []
            try PairingTicket(version: 1, url: address, pin: pin, token: String(repeating: "0", count: 64), expires_at: Int64.max, urls: addresses).validate()
            if let learned = health.key_pin {
                guard learned.count == 64, learned.allSatisfy({ $0.isHexDigit }),
                      keyPin == nil || keyPin == learned else { throw ClientError.message("Server key changed") }
            }
            return .reachable(ServerRoute(url: address, addresses: addresses, keyPin: keyPin ?? health.key_pin))
        } catch {
            return .failed("\(address): \(connectionFailure(error))")
        }
    }
    static func connectionFailure(_ error: Error) -> String {
        let failure = error as NSError
        guard failure.domain == NSURLErrorDomain else { return "Connection failed (\(failure.domain), \(failure.code))" }
        switch URLError.Code(rawValue: failure.code) {
        case .timedOut: return "Timed out. The server did not respond in time."
        case .cannotConnectToHost: return "Connection refused or host unavailable."
        case .notConnectedToInternet: return "Network access unavailable. Check xlatch’s Local Network permission and VPN access."
        case .serverCertificateUntrusted, .serverCertificateHasBadDate, .serverCertificateHasUnknownRoot, .serverCertificateNotYetValid, .secureConnectionFailed, .userCancelledAuthentication:
            return "TLS verification failed. The saved certificate pin or certificate trust may not match; do not disable verification."
        default: return "\(failure.localizedDescription) (URL error \(failure.code))"
        }
    }
    static func reachableOrigin(_ addresses: [String], pin: String, keyPin: String? = nil) async throws -> ServerRoute {
        try await withThrowingTaskGroup(of: OriginProbe.self) { group in
            for address in Set(addresses) { group.addTask { await probe(address, pin: pin, keyPin: keyPin) } }
            var failures: [String] = []
            for try await result in group {
                switch result {
                case .reachable(let address): group.cancelAll(); return address
                case .failed(let reason): failures.append(reason)
                }
            }
            throw ClientError.message("Could not connect to xlatch.\n" + failures.sorted().joined(separator: "\n"))
        }
    }

}

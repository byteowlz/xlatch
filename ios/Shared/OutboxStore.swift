import Foundation
import SQLite3

/// Each mutation uses a SQLite transaction: the app and share extension are separate processes.
final class OutboxStore {
    let url: URL
    private let maxItems: Int
    private let maxBytes: Int
    init(directory: URL? = nil, maxItems: Int = 50, maxBytes: Int = 64 * 1024 * 1024) throws {
        guard let directory = directory ?? FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.com.byteowlz.xlatch")?.appendingPathComponent("Outbox", isDirectory: true) else { throw ClientError.message("The shared Outbox is unavailable.") }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
        var protectedDirectory = directory
        var values = URLResourceValues(); values.isExcludedFromBackup = true
        try protectedDirectory.setResourceValues(values)
        url = directory.appendingPathComponent("outbox.sqlite")
        self.maxItems = maxItems; self.maxBytes = maxBytes
    }
    private func transaction<T>(_ body: (OpaquePointer) throws -> T) throws -> T {
        var database: OpaquePointer?
        guard sqlite3_open_v2(url.path, &database, SQLITE_OPEN_CREATE | SQLITE_OPEN_READWRITE | SQLITE_OPEN_FULLMUTEX, nil) == SQLITE_OK, let database else {
            if let database { sqlite3_close(database) }
            throw ClientError.message("Could not open Outbox storage.")
        }
        defer { sqlite3_close(database) }
        try FileManager.default.setAttributes([.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication], ofItemAtPath: url.path)
        sqlite3_busy_timeout(database, 5000)
        try execute(database, "PRAGMA secure_delete=ON")
        try execute(database, "CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, record BLOB NOT NULL)")
        try execute(database, "BEGIN IMMEDIATE")
        do {
            let result = try body(database)
            try execute(database, "COMMIT")
            return result
        } catch {
            sqlite3_exec(database, "ROLLBACK", nil, nil, nil)
            throw error
        }
    }
    private func execute(_ db: OpaquePointer, _ sql: String) throws {
        guard sqlite3_exec(db, sql, nil, nil, nil) == SQLITE_OK else { throw ClientError.message("Could not update Outbox storage.") }
    }
    private func records(_ db: OpaquePointer) throws -> [OutboxItem] {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT record FROM items", -1, &statement, nil) == SQLITE_OK else { throw ClientError.message("Could not read Outbox.") }
        defer { sqlite3_finalize(statement) }
        var result: [OutboxItem] = []
        var status = sqlite3_step(statement)
        while status == SQLITE_ROW {
            guard let bytes = sqlite3_column_blob(statement, 0) else { throw ClientError.message("An Outbox record is unreadable.") }
            result.append(try JSONDecoder().decode(OutboxItem.self, from: Data(bytes: bytes, count: Int(sqlite3_column_bytes(statement, 0)))))
            status = sqlite3_step(statement)
        }
        guard status == SQLITE_DONE else { throw ClientError.message("Could not finish reading Outbox.") }
        return result.sorted { $0.created < $1.created }
    }
    private func save(_ item: OutboxItem, _ db: OpaquePointer) throws {
        let data = try JSONEncoder().encode(item)
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "INSERT OR REPLACE INTO items(id,record) VALUES(?1,?2)", -1, &statement, nil) == SQLITE_OK else { throw ClientError.message("Could not prepare Outbox write.") }
        defer { sqlite3_finalize(statement) }
        let status = item.id.withCString { id in
            data.withUnsafeBytes { bytes in
                defer { sqlite3_clear_bindings(statement) }
                sqlite3_bind_text(statement, 1, id, -1, nil)
                sqlite3_bind_blob(statement, 2, bytes.baseAddress, Int32(bytes.count), nil)
                return sqlite3_step(statement)
            }
        }
        guard status == SQLITE_DONE else { throw ClientError.message("Could not save Outbox state. Check available storage.") }
    }
    private func remove(_ id: String, _ db: OpaquePointer) throws {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "DELETE FROM items WHERE id=?1", -1, &statement, nil) == SQLITE_OK else { throw ClientError.message("Could not prepare Outbox deletion.") }
        defer { sqlite3_finalize(statement) }
        let status = id.withCString { value in
            defer { sqlite3_clear_bindings(statement) }
            sqlite3_bind_text(statement, 1, value, -1, nil)
            return sqlite3_step(statement)
        }
        guard status == SQLITE_DONE else { throw ClientError.message("Could not delete Outbox item.") }
    }
    private func maintain(_ db: OpaquePointer, now: Date) throws {
        for var item in try records(db) where item.expires <= now {
            if now.timeIntervalSince(item.expires) >= 86400 || item.state == .sent || item.state == .cancelled {
                try remove(item.id, db)
            } else if item.state != .expired {
                item.state = .expired; item.payload = nil; item.label = "Expired share"
                item.lease = nil; item.leaseUntil = nil
                item.detail = "Delivery expired after seven days. Content was removed. A previous attempt may have reached the server."
                try save(item, db)
            }
        }
    }
    func items(now: Date = Date()) throws -> [OutboxItem] {
        try transaction { db in try maintain(db, now: now); return try records(db) }
    }
    func enqueue(_ item: OutboxItem) throws -> OutboxItem {
        try transaction { db in
            try maintain(db, now: item.created)
            let existing = try records(db)
            if let same = existing.first(where: { $0.id == item.id }) {
                guard same.deviceID == item.deviceID, same.serverPin == item.serverPin, same.deviceKey == item.deviceKey,
                      same.capability == item.capability, same.chain == item.chain, same.payloadHash == item.payloadHash else { throw ClientError.message("This retry ID belongs to different content. Open Outbox to inspect the earlier attempt.") }
                return same
            }
            let pending = existing.filter { $0.payload != nil }
            let bytes = try pending.reduce(0) { try $0 + JSONEncoder().encode($1).count }
            guard pending.count < maxItems, try bytes + JSONEncoder().encode(item).count <= maxBytes else { throw ClientError.message("Outbox is full (50 items / 64 MB). Remove saved items before sharing more.") }
            for old in existing.filter({ [.sent, .cancelled, .expired].contains($0.state) }).dropLast(99) {
                try remove(old.id, db)
            }
            try save(item, db); return item
        }
    }
    func claim(id: String? = nil, now: Date = Date()) throws -> OutboxItem? {
        try transaction { db in
            try maintain(db, now: now)
            guard var item = try records(db).first(where: {
                (id == nil || $0.id == id) && (($0.state == .waiting && $0.nextAttempt <= now) || ($0.state == .sending && ($0.leaseUntil ?? .distantPast) <= now))
            }) else { return nil }
            item.upload = nil
            item.state = .sending; item.lease = UUID().uuidString
            item.leaseUntil = now.addingTimeInterval(180); item.attempts += 1
            try save(item, db); return item
        }
    }
    func updateProgress(_ item: OutboxItem, progress: UploadProgress) throws {
        try transaction { db in
            var statement: OpaquePointer?
            guard sqlite3_prepare_v2(db, "SELECT record FROM items WHERE id=?1", -1, &statement, nil) == SQLITE_OK else { throw ClientError.message("Could not read upload progress.") }
            defer { sqlite3_finalize(statement) }
            let data: Data? = item.id.withCString { id in
                defer { sqlite3_reset(statement); sqlite3_clear_bindings(statement) }
                sqlite3_bind_text(statement, 1, id, -1, nil)
                guard sqlite3_step(statement) == SQLITE_ROW, let bytes = sqlite3_column_blob(statement, 0) else { return nil }
                return Data(bytes: bytes, count: Int(sqlite3_column_bytes(statement, 0)))
            }
            guard let data else { return }
            var current = try JSONDecoder().decode(OutboxItem.self, from: data)
            guard current.state == .sending, current.lease == item.lease else { return }
            guard progress.sent >= (current.upload?.sent ?? 0) else { return }
            current.upload = progress
            try save(current, db)
        }
    }
    func owns(_ item: OutboxItem) throws -> Bool {
        try transaction { db in try records(db).contains { $0.id == item.id && $0.state == .sending && $0.lease == item.lease } }
    }
    func finish(_ item: OutboxItem, job: Job? = nil, error: String? = nil, retry: Bool = false, now: Date = Date()) throws {
        try transaction { db in
            guard var saved = try records(db).first(where: { $0.id == item.id && $0.lease == item.lease && $0.state == .sending }) else { return }
            saved.upload = nil
            saved.lease = nil; saved.leaseUntil = nil; saved.detail = error
            if let job { saved.state = .sent; saved.jobID = job.id; saved.payload = nil }
            else {
                saved.state = retry ? .waiting : .paused
                saved.nextAttempt = now.addingTimeInterval(min(3600, 15 * pow(2, Double(min(saved.attempts, 8)))))
            }
            try save(saved, db)
        }
    }
    func expedite(now: Date = Date()) throws {
        try transaction { db in
            for var item in try records(db) where item.state == .waiting {
                item.nextAttempt = now; try save(item, db)
            }
        }
    }
    func retry(_ id: String, now: Date = Date()) throws {
        try transaction { db in
            try maintain(db, now: now)
            guard var item = try records(db).first(where: { $0.id == id }), [.waiting, .paused].contains(item.state) else { return }
            item.state = .waiting; item.nextAttempt = now; item.detail = nil
            try save(item, db)
        }
    }
    func cancel(_ id: String) throws {
        try transaction { db in
            guard var item = try records(db).first(where: { $0.id == id }) else { return }
            item.state = .cancelled; item.payload = nil; item.lease = nil; item.leaseUntil = nil
            item.detail = "No further retries. This does not cancel work already accepted by the server."
            try save(item, db)
        }
    }
    func delete(_ id: String) throws { try transaction { try remove(id, $0) } }
}

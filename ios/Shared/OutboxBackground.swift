import BackgroundTasks
import Foundation
import OSLog

enum OutboxBackground {
    static let identifier = "com.byteowlz.xlatch.outbox"
    static func schedule() {
        do {
            let pending = try OutboxStore().items().filter { [.waiting, .sending].contains($0.state) }
            guard let earliest = pending.map({ $0.state == .sending ? $0.leaseUntil ?? $0.nextAttempt : $0.nextAttempt }).min() else { return }
            let request = BGProcessingTaskRequest(identifier: identifier)
            request.requiresNetworkConnectivity = true
            request.earliestBeginDate = max(Date().addingTimeInterval(60), earliest)
            try BGTaskScheduler.shared.submit(request)
            UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.removeObject(forKey: "outbox-background-warning")
        } catch {
            UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.set("Background retry is unavailable right now. Open xlatch to send waiting items.", forKey: "outbox-background-warning")
            Logger(subsystem: "com.byteowlz.xlatch", category: "outbox").notice("Background retry was not scheduled: \(error.localizedDescription)")
        }
    }
}

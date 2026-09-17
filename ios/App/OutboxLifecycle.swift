import BackgroundTasks
import Network
import UIKit

final class OutboxLifecycle: NSObject, UIApplicationDelegate {
    private let network = NWPathMonitor()
    func application(_ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        BGTaskScheduler.shared.register(forTaskWithIdentifier: OutboxBackground.identifier, using: nil) { task in
            let work = Task {
                await OutboxDelivery.shared.drain()
                let error = await OutboxDelivery.shared.lastError
                task.setTaskCompleted(success: !Task.isCancelled && error == nil)
            }
            task.expirationHandler = { work.cancel() }
        }
        network.pathUpdateHandler = { path in
            if path.status == .satisfied {
                Task { await OutboxDelivery.shared.drain(expedite: true) }
            }
        }
        network.start(queue: DispatchQueue(label: "com.byteowlz.xlatch.reachability"))
        return true
    }
}

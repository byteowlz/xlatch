import SwiftUI

@main struct SelectionPreview: App {
    @StateObject private var model = fixture()
    @State private var activityPhase = 0
    var body: some Scene {
        WindowGroup {
            ScrollViewReader { proxy in
                ShareView(model: model)
                    .overlay(alignment: .bottom) {
                        if ProcessInfo.processInfo.arguments.contains("--activity-test") {
                            Button("Next activity") {
                                activityPhase += 1
                                switch activityPhase {
                                case 1: model.sending = model.choices[0].id; model.uploadingID = "fixture-upload"
                                case 2: model.batchPosition = "Sending 1 of 2"; model.parkProgress = UploadProgress(sent: 512, total: 1024)
                                default: model.sending = nil; model.uploadingID = nil; model.batchPosition = nil; model.parkProgress = nil
                                }
                            }.accessibilityIdentifier("fixture-next-activity")
                        }
                    }
                    .preferredColorScheme(ProcessInfo.processInfo.arguments.contains("--light") ? .light : .dark)
                    .environment(\.dynamicTypeSize, ProcessInfo.processInfo.arguments.contains("--large-type") ? .accessibility1 : .large)
                    .task {
                        try? await Task.sleep(for: .milliseconds(400))
                        if !ProcessInfo.processInfo.arguments.contains("--top"), let last = model.choices.last { proxy.scrollTo(last.id, anchor: .bottom) }
                    }
            }
        }
    }
    @MainActor private static func fixture() -> ShareModel {
        UserDefaults(suiteName: "group.com.byteowlz.xlatch")?.set(ProcessInfo.processInfo.arguments.contains("--tiles") ? "tiles" : "list", forKey: "share-target-layout")
        let model = ShareModel(context: nil)
        let server = Connection(url: "https://fixture.invalid", pin: "fixture", deviceID: "selection-preview", privateKey: Data(), serverID: "preview")
        model.input = .text("https://example.com/shared-link", mime: "text/uri-list")
        model.connections = [server]; model.connection = server; model.loading = false
        model.catalogs[server.id] = (1...30).map { index in
            Capability(manifest: Manifest(id: "fixture.\(index)", title: "Project \(index) · Send to session", description: "", accepts: ["*/*"], execution: .init(kind: "command")), revision: "fixture", status: "active")
        }
        if ProcessInfo.processInfo.arguments.contains("--selected") {
            model.swipeSelect(model.choices[0]); model.swipeSelect(model.choices[1])
        } else if ProcessInfo.processInfo.arguments.contains("--one") {
            model.swipeSelect(model.choices[0])
        }
        return model
    }
}

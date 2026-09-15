import AppIntents
import SwiftUI

struct ShortcutSettingsView: View {
    @EnvironmentObject var model: AppModel
    @State private var selection = QuickSend.selectedID
    private var targets: [XLatchTarget] {
        guard let connection = model.connection else { return [] }
        return QuickSend.available(model.capabilities, connection: connection).map {
            XLatchTarget(connection: connection, capability: $0)
        }
    }
    var body: some View {
        Form {
            Section {
                ForEach(targets) { target in
                    Button {
                        selection = target.id; QuickSend.selectedID = target.id
                        XLatchShortcuts.updateAppShortcutParameters()
                    } label: {
                        HStack {
                            VStack(alignment: .leading) {
                                Text(target.title)
                                Text(target.server).font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            if selection == target.id { Image(systemName: "checkmark") }
                        }
                    }.tint(.primary)
                }
                if targets.isEmpty { Text("No approved targets available. Refresh the server connection first.") }
                if let selection, !targets.contains(where: { $0.id == selection }) {
                    Text("Your saved target is no longer available. Select a target again.").foregroundStyle(.orange)
                }
            } header: { Text("Quick-send target") } footer: {
                Text("Shortcuts use this target unless you select another in the action. A changed or removed target stops the shortcut; it never switches to another session.")
            }
            Section {
                ShortcutsLink()
                shortcut("xlatch Clipboard", title: "Add clipboard shortcut")
                shortcut("xlatch Screenshot", title: "Add screenshot shortcut")
                shortcut("xlatch Screen Context", title: "Add screenshot with context shortcut")
            } header: { Text("Ready-made shortcuts") } footer: {
                Text("Share the shortcut file to Shortcuts, then tap Add Shortcut. If Shortcuts is not offered, save it to Files and open it there. Your pairing keys stay in xlatch.")
            }
            Section("Back Tap") {
                Text("After adding a shortcut: Settings → Accessibility → Touch → Back Tap → Double Tap or Triple Tap → choose the shortcut.")
                Text("Your phone must be unlocked. iOS may ask for permission to access the clipboard or shared content.").foregroundStyle(.secondary)
            }
            Section("Screen context") {
                Text("The context shortcut includes the screenshot, visible text recognized on-device, and any URL/text the foreground app supplies. X may not provide a post URL through Back Tap; use its Share action for the exact link.")
                Text("Safari sharing can include page text and title. No website is fetched in the background, and clipboard context is off by default. Screenshots are resized to 2048 pixels and sent as JPEG.").foregroundStyle(.secondary)
            }
        }.navigationTitle("Shortcuts & Back Tap")
            .task { await model.refresh(); XLatchShortcuts.updateAppShortcutParameters() }
    }
    @ViewBuilder private func shortcut(_ resource: String, title: String) -> some View {
        if let url = Bundle.main.url(forResource: resource, withExtension: "shortcut") {
            ShareLink(item: url) { Label(title, systemImage: "square.and.arrow.down") }
        }
    }
}

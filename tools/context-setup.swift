import AppKit
import Foundation

func argument(_ key: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: key), index + 1 < CommandLine.arguments.count else { return nil }
    return CommandLine.arguments[index + 1]
}
func shellQuote(_ value: String) -> String { "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'" }

let manager = FileManager.default
let bundled = Bundle.main.bundleURL.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
let app = URL(fileURLWithPath: argument("--application") ?? bundled.path)
let data = URL(fileURLWithPath: argument("--data-dir") ?? manager.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/dev.silta.clearframe.desktop").path)
let services = URL(fileURLWithPath: argument("--services-dir") ?? manager.homeDirectoryForCurrentUser.appendingPathComponent("Library/Services").path)
let resource = Bundle.main.resourceURL!
let languageCode = Locale.preferredLanguages.first?.split(separator: "-").first.map(String.init) ?? "en"
let language = ["zh", "ja"].contains(languageCode) ? languageCode : "en"
let messages = try JSONDecoder().decode([String: String].self, from: Data(contentsOf: resource.appendingPathComponent("locales/\(language).json")))
let application = NSApplication.shared
application.setActivationPolicy(.regular)
application.finishLaunching()
application.activate(ignoringOtherApps: true)
let alert = NSAlert()
alert.messageText = messages["title"]!
alert.informativeText = messages["body"]!
alert.addButton(withTitle: messages["continue"]!)
alert.addButton(withTitle: messages["cancel"]!)
let checkbox = NSButton(checkboxWithTitle: messages["checkbox"]!, target: nil, action: nil)
checkbox.state = .on
checkbox.setAccessibilityIdentifier("enable-context-menu")
checkbox.frame = NSRect(x: 0, y: 0, width: 420, height: 28)
alert.accessoryView = checkbox
guard alert.runModal() == .alertFirstButtonReturn else { exit(2) }

do {
    let executable = app.appendingPathComponent("Contents/MacOS/clearframe-desktop")
    guard manager.isExecutableFile(atPath: executable.path) else { throw NSError(domain: "ClearFrame", code: 1, userInfo: [NSLocalizedDescriptionKey: "Application executable not found: \(executable.path)"]) }
    try manager.createDirectory(at: services, withIntermediateDirectories: true)
    for mode in ["single", "batch"] {
        let name = "ClearFrame \(mode.capitalized).workflow"
        let target = services.appendingPathComponent(name)
        if manager.fileExists(atPath: target.path) {
            let existing = try Data(contentsOf: target.appendingPathComponent("Contents/Info.plist"))
            let info = try PropertyListSerialization.propertyList(from: existing, format: nil) as? [String: Any]
            guard info?["CFBundleIdentifier"] as? String == "dev.silta.clearframe.desktop.context.\(mode)" else { throw NSError(domain: "ClearFrame", code: 2, userInfo: [NSLocalizedDescriptionKey: "An unrelated workflow already uses \(name). It was kept."]) }
        }
        if checkbox.state == .on {
            let temporary = services.appendingPathComponent(".clearframe-\(UUID().uuidString).workflow")
            defer { try? manager.removeItem(at: temporary) }
            try manager.copyItem(at: resource.appendingPathComponent("workflows/\(name)"), to: temporary)
            let document = temporary.appendingPathComponent("Contents/Resources/document.wflow")
            let xml = try String(contentsOf: document, encoding: .utf8)
            // Escape for both the shell literal and the enclosing XML document.
            let literal = shellQuote(executable.path).replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;").replacingOccurrences(of: ">", with: "&gt;")
            try xml.replacingOccurrences(of: "__CLEARFRAME_EXECUTABLE__", with: literal).write(to: document, atomically: true, encoding: .utf8)
            if manager.fileExists(atPath: target.path) { _ = try manager.replaceItemAt(target, withItemAt: temporary) }
            else { try manager.moveItem(at: temporary, to: target) }
        } else if manager.fileExists(atPath: target.path) { try manager.removeItem(at: target) }
    }
    try manager.createDirectory(at: data, withIntermediateDirectories: true)
    let choice: [String: Any] = ["schemaVersion": 1, "enabled": checkbox.state == .on, "application": app.path]
    try JSONSerialization.data(withJSONObject: choice).write(to: data.appendingPathComponent("context-menu-choice.json"), options: .atomic)
    if !CommandLine.arguments.contains("--no-refresh") { NSUpdateDynamicServices() }
} catch {
    let failure = NSAlert()
    failure.messageText = messages["error"]!
    failure.informativeText = error.localizedDescription
    failure.runModal()
    exit(1)
}

// A stand-in for the Apple Account sign-in alert, which CoreFollowUp's
// FollowUpUI.app draws. The alerts script runs it as a binary named
// FollowUpUI, so the window server lists that owner. It never activates and
// quits after SECONDS.
//
//   FollowUpUI SECONDS
import AppKit

let seconds = Double(CommandLine.arguments.dropFirst().first ?? "20") ?? 20
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let window = NSWindow(
  contentRect: NSRect(x: 0, y: 0, width: 420, height: 170), styleMask: [.titled],
  backing: .buffered, defer: false)
window.title = "Apple Account Sign In Requested"
window.level = .floating
let text = NSTextField(labelWithString: "Agent Screen test: your Apple Account is being used to sign in to a new device.")
text.frame = NSRect(x: 20, y: 70, width: 380, height: 60)
text.lineBreakMode = .byWordWrapping
text.maximumNumberOfLines = 3
window.contentView!.addSubview(text)
for (i, title) in ["Don't Allow", "Allow"].enumerated() {
  let button = NSButton(title: title, target: nil, action: nil)
  button.frame = NSRect(x: 200 + i * 105, y: 16, width: 100, height: 32)
  window.contentView!.addSubview(button)
}
window.center()
window.orderFrontRegardless()
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
app.run()

// An app whose main window comes seconds after its first, as WeCom's does:
// a small loading window at once, then, after LATE_MAIN_MS (3500 by
// default), the loading window closes and a large main window opens on the
// main screen, where an app restores a saved frame. LATE_BUSY_MS (0 by
// default) then keeps its main thread busy that long, as a large app
// building its main window is: Accessibility gets no answer from it
// meanwhile. SIGUSR1 hides it, as Command-H does. It never activates
// itself.
import AppKit

final class Delegate: NSObject, NSApplicationDelegate {
  var loading: NSWindow!
  var main: NSWindow?

  func applicationDidFinishLaunching(_ note: Notification) {
    loading = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 280, height: 400),
                       styleMask: [.titled], backing: .buffered, defer: false)
    loading.title = "Loading"
    loading.isReleasedWhenClosed = false
    loading.center()
    loading.makeKeyAndOrderFront(nil)
    let ms = Double(ProcessInfo.processInfo.environment["LATE_MAIN_MS"] ?? "") ?? 3500
    DispatchQueue.main.asyncAfter(deadline: .now() + ms / 1000) { [unowned self] in
      self.loading.close()
      let screen = NSScreen.screens[0].visibleFrame
      let size = NSSize(width: min(1100, screen.width), height: min(700, screen.height))
      let frame = NSRect(x: screen.minX, y: screen.maxY - size.height, width: size.width,
                         height: size.height)
      let window = NSWindow(contentRect: frame, styleMask: [.titled, .closable, .resizable],
                            backing: .buffered, defer: false)
      window.title = "Main"
      window.isReleasedWhenClosed = false
      window.setFrame(frame, display: false)
      window.makeKeyAndOrderFront(nil)
      self.main = window
      let busy = Double(ProcessInfo.processInfo.environment["LATE_BUSY_MS"] ?? "") ?? 0
      if busy > 0 {
        Thread.sleep(forTimeInterval: busy / 1000)
      }
    }
  }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
signal(SIGUSR1, SIG_IGN)
let hide = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
hide.setEventHandler { NSApp.hide(nil) }
hide.resume()
app.setActivationPolicy(.regular)
app.run()

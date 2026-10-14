// A system alert drawn by UserNotificationCenter, as macOS's own prompts
// are: "Sidescreen test alert", closing by itself after SECONDS.
//
//   alert SECONDS
import Foundation

let seconds = Double(CommandLine.arguments.dropFirst().first ?? "20") ?? 20
var response: CFOptionFlags = 0
CFUserNotificationDisplayAlert(
  seconds, kCFUserNotificationNoteAlertLevel, nil, nil, nil,
  "Sidescreen test alert" as CFString,
  "A test of system alerts on agent displays. It closes by itself." as CFString,
  "OK" as CFString, nil, nil, &response)

/**
 * Takeover through the Viewer, beside the user's Sidescreen and out of
 * their sight: the Viewer opens on a stage display, and the user's clicks
 * and keys are made inside the app, through the Viewer's own handler. The
 * user's pointer and keyboard stay untouched, so this runs at any time.
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/e2e/takeover.ts
 */
import { Control, check, failed, instance, probe, buildTools, running, sh, startTestApp, stopTestApp, waitForUnlocked } from "./lib.ts"

if (!instance) throw new Error("run with SIDESCREEN_INSTANCE=e2e")
if (running("TextEdit")) {
  console.log("skip: TextEdit is the user's")
  process.exit(0)
}
buildTools()
if (!(await waitForUnlocked())) process.exit(1)
await startTestApp({ SIDESCREEN_LANG: "en" })
const control = await Control.connect()
/** US key codes for what the test types. */
const codes: Record<string, number> = {
  a: 0, s: 1, d: 2, f: 3, h: 4, g: 5, z: 6, x: 7, c: 8, v: 9, b: 11, q: 12, w: 13, e: 14, r: 15, y: 16, t: 17,
  o: 31, u: 32, i: 34, p: 35, l: 37, j: 38, k: 40, n: 45, m: 46, " ": 49, ".": 47, ",": 43
}
const typeInViewer = async (text: string) => {
  for (const ch of text) {
    const lower = ch.toLowerCase()
    await control.call("debugViewerInput", { kind: "key", code: codes[lower], shift: ch !== lower ? 1 : 0 })
  }
}
/** The user may switch apps meanwhile; the test's own apps must never come forward. */
const stolen: string[] = []
const watchFront = () => {
  const now = probe("front")
  if (now === "TextEdit" || now === "Sidescreen") stolen.push(now)
}
const documentText = () =>
  sh("/usr/bin/osascript", ["-e", 'tell application "TextEdit" to get text of front document'], true)

try {
  const stage = await control.call("createDisplay", { sessionKey: "stage", title: "Stage", width: 1728, height: 1117 })
  await control.call("debugShow", { surface: "stage", displayId: stage.displayId })
  const id = (await control.call("createDisplay", { sessionKey: "takeover", client: "Claude Code", title: "Takeover", width: 800, height: 500 })).displayId
  await control.call("launchApp", { displayId: id, app: "TextEdit" })
  await control.call("type", { displayId: id, text: "Agent wrote this." })
  await control.call("debugShow", { surface: "viewer", displayId: id })
  await Bun.sleep(1200)
  watchFront()

  const { frame } = await control.call("debugViewer")
  const [mw, mh] = probe("main").split(" ").map(Number)
  const [fx, fy, fw, fh] = frame as number[]
  const onMain = fx < mw && fx + fw > 0 && fy < mh && fy + fh > 0
  check("the Viewer opens on the stage, not the user's screen", !onMain, JSON.stringify(frame))

  await control.call("requestHelp", { displayId: id, reason: "Please check the draft." })
  await control.call("debugViewerInput", { kind: "click", x: 0.5, y: 0.7 })
  const blocked = await control.call("type", { displayId: id, text: "x" }).then(() => false, () => true)
  check("the agent's input fails during a Takeover", blocked)
  await control.call("debugViewerInput", { kind: "click", x: 0.5, y: 0.7 })
  await typeInViewer(" User added this.")
  await Bun.sleep(400)
  watchFront()
  check("the user's keys reach the agent's app", documentText().includes("User added this."), JSON.stringify(documentText()))

  const resumed = control.call("waitForControl", { displayId: id, timeoutSeconds: 10 })
  await control.call("debugShow", { surface: "primary", displayId: id })
  check("Let Continue hands control back", (await resumed).outcome === "returned")
  await control.call("type", { displayId: id, text: " Agent again." })
  await Bun.sleep(300)
  check("the agent types again after the hand-back", documentText().endsWith("Agent again."), JSON.stringify(documentText()))
  watchFront()
  check("the test's apps never came to the front", stolen.length === 0, stolen.join(" "))
} finally {
  sh("/usr/bin/osascript", ["-e", 'tell application "TextEdit" to close every document saving no\ntell application "TextEdit" to quit'], true)
  control.close()
  await stopTestApp()
}
console.log(failed() ? `${failed()} failed` : "all passed")
process.exit(failed() ? 1 : 0)

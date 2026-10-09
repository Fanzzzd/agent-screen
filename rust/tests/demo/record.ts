/**
 * Record the README's demo: an agent working on its own display, seen in
 * the Viewer. Captures only the Viewer's frame, so nothing else on the
 * user's screen gets in.
 *
 *     bun rust/tests/demo/record.ts          waits for an idle user
 *
 * Writes docs/media/demo.gif. Needs ffmpeg. Like the e2e tests it waits
 * until the user is idle with no Agent Display open and the screen
 * unlocked, then puts the front app and the installed Sidescreen back.
 */
import { spawn, spawnSync } from "node:child_process"
import { mkdirSync } from "node:fs"
import * as path from "node:path"
import {
  Control, buildTools, reopenInstalled, restore, root, sh, snapshot, startDebugApp, stopDebugApp, waitForQuiet
} from "../e2e/lib.ts"

const media = path.join(root, "..", "docs", "media")
const movie = path.join(root, "target", "demo.mov")
const gif = path.join(media, "demo.gif")
const pause = (ms: number) => Bun.sleep(ms)
const quit = (app: string, discard = false) =>
  sh("/usr/bin/osascript", ["-e", discard
    ? `tell application "${app}" to close every document saving no\ntell application "${app}" to quit`
    : `quit app "${app}"`], true)

for (const app of ["TextEdit", "Calculator"]) {
  if (sh("/usr/bin/pgrep", ["-x", app], true)) {
    console.log(`skip: ${app} is the user's`)
    process.exit(0)
  }
}
buildTools()
if (!(await waitForQuiet())) process.exit(1)
const before = snapshot()
await startDebugApp()
const control = await Control.connect()
let recorder: ReturnType<typeof spawn> | undefined
try {
  const display = await control.call("createDisplay", {
    sessionKey: "demo", client: "Claude Code", title: "Write the launch note", width: 1280, height: 800
  })
  const id = display.displayId
  const editor = (await control.call("launchApp", { displayId: id, app: "TextEdit" })).window
  await control.call("debugShow", { surface: "viewer", displayId: id })
  await pause(1500)
  const { frame } = await control.call("debugViewer")

  recorder = spawn("/usr/sbin/screencapture", ["-v", "-x", "-V", "24", "-R", frame.map(Math.round).join(","), movie])
  await pause(1500)

  for (const word of "Hi! I'm Claude, working on my own screen. Your mouse and keyboard stay yours.".split(" ")) {
    await control.call("type", { displayId: id, text: `${word} ` })
    await pause(120)
  }
  await pause(800)

  const calculator = (await control.call("launchApp", { displayId: id, app: "Calculator" })).window
  await pause(900)
  const tree: string = (await control.call("windowState", { displayId: id, windowId: calculator.windowId })).tree
  const button = (label: string) => Number(tree.match(new RegExp(`\\[(\\d+)\\] button "${label}"`))![1])
  for (const label of ["All Clear", "1", "2", "Multiply", "3", "4", "Equals"]) {
    await control.call("elementAction", { displayId: id, windowId: calculator.windowId, element: button(label), action: "press" })
    await pause(350)
  }
  await pause(1200)

  await control.call("requestHelp", { displayId: id, reason: "Please sign in to the publishing site for me." })
  await new Promise((resolve) => recorder!.once("exit", resolve))
  recorder = undefined
  void editor

  mkdirSync(media, { recursive: true })
  const ffmpeg = spawnSync("ffmpeg", ["-y", "-loglevel", "error", "-i", movie, "-vf",
    "fps=12,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=4",
    gif], { encoding: "utf8" })
  if (ffmpeg.status !== 0) throw new Error(ffmpeg.stderr)
  console.log(gif)
} finally {
  recorder?.kill()
  quit("TextEdit", true)
  quit("Calculator")
  control.close()
  await pause(800)
  stopDebugApp()
  reopenInstalled()
  restore(before)
}

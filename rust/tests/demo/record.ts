/**
 * Record the README's demo, out of the user's sight. The agent writes in
 * TextEdit on its own display, seen in the Viewer, while "the user" types
 * in their notes over it. The agent asks for a check, the user takes over,
 * adds a line, and lets the agent continue.
 *
 *     bun rust/tests/demo/record.ts
 *
 * Runs beside the user's Sidescreen (SIDESCREEN_INSTANCE=demo). The Viewer
 * opens on a stage display that stands in for the user's screen; the
 * user's notes, pointer, clicks and keys there are made by the script, so
 * the real pointer and keyboard stay with the user. Writes
 * docs/media/demo.gif. Needs ffmpeg.
 */
import { spawn, spawnSync } from "node:child_process"
import { existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs"
import * as path from "node:path"
import { createInterface } from "node:readline"

process.env.SIDESCREEN_INSTANCE ||= "demo"
const { Control, buildTools, capture, out, probe, root, running, sh, startTestApp, stopTestApp, waitForUnlocked } =
  await import("../e2e/lib.ts")

const media = path.join(root, "..", "docs", "media")
const movie = path.join(root, "target", "demo.mov")
const gif = path.join(media, "demo.gif")
const pause = (ms: number) => Bun.sleep(ms)
/** US key codes for what the user types. */
const codes: Record<string, number> = {
  a: 0, s: 1, d: 2, f: 3, h: 4, g: 5, z: 6, x: 7, c: 8, v: 9, b: 11, q: 12, w: 13, e: 14, r: 15, y: 16, t: 17,
  "1": 18, "2": 19, o: 31, u: 32, i: 34, p: 35, l: 37, j: 38, k: 40, n: 45, m: 46, " ": 49, ".": 47, ",": 43
}
/** The whole document; windowState cuts long values short. */
const documentText = () =>
  sh("/usr/bin/osascript", ["-e", 'tell application "TextEdit" to get text of front document'], true)

if (running("TextEdit")) {
  console.log("skip: TextEdit is the user's")
  process.exit(0)
}
buildTools()
const notesTool = path.join(out, "notes")
const notesSource = new URL("notes.swift", import.meta.url).pathname
if (!existsSync(notesTool) || statSync(notesTool).mtimeMs < statSync(notesSource).mtimeMs) {
  sh("/usr/bin/swiftc", ["-O", notesSource, "-o", notesTool])
}
if (!(await waitForUnlocked())) process.exit(1)
await startTestApp({ SIDESCREEN_LANG: "en" })
const control = await Control.connect()
let recorder: ReturnType<typeof spawn> | undefined
let notes: ReturnType<typeof spawn> | undefined
try {
  // The stage: a display the size of a laptop screen, where the Viewer opens.
  const stage = await control.call("createDisplay", { sessionKey: "stage", title: "Stage", width: 1728, height: 1117 })
  await control.call("debugShow", { surface: "stage", displayId: stage.displayId })

  const display = await control.call("createDisplay", {
    sessionKey: "demo", client: "Claude Code", title: "Write the launch note", width: 800, height: 500
  })
  const id = display.displayId
  // A named document, not "Untitled 32", in 16 pt so it reads in the GIF.
  const note = path.join(out, "Launch note.rtf")
  writeFileSync(note, "{\\rtf1\\ansi{\\fonttbl\\f0\\fswiss Helvetica;}\\f0\\fs32 \\par }")
  await control.call("launchApp", { displayId: id, app: "TextEdit", open: note })
  await control.call("debugShow", { surface: "viewer", displayId: id })
  await pause(1500)
  const { frame, image } = await control.call("debugViewer")
  const [fx, fy, fw] = frame as number[]
  const [ix, iy, iw, ih] = image as number[]

  // The user's notes float over the lower right of the agent's screen.
  const [nw, nh] = [420, 150]
  notes = spawn(notesTool, [ix + iw - nw - 24, iy + ih - nh - 24, nw, nh].map(String), { stdio: ["pipe", "pipe", "inherit"] })
  const lines = createInterface({ input: notes.stdout! })[Symbol.asyncIterator]()
  await lines.next()
  const user = async (command: string) => {
    notes!.stdin!.write(`${command}\n`)
    await lines.next()
  }
  /** Move the user's pointer to a point of the agent's screen, as fractions. */
  const pointAt = (x: number, y: number) => user(`move ${ix + iw * x} ${iy + ih * y}`)

  rmSync(movie, { force: true })
  recorder = spawn("/usr/sbin/screencapture", ["-v", "-x", "-V", "45", "-R", (frame as number[]).map(Math.round).join(","), movie])
  await pause(1500)

  // Both type at once: the agent in its TextEdit, the user in their notes.
  const typing = user("type Reply to Sam, lunch at 1pm works for me.")
  for (const word of "Launch note: Sidescreen gives each agent a screen of its own, so you keep your mouse and keyboard.".split(" ")) {
    await control.call("type", { displayId: id, text: `${word} ` })
    await pause(110)
  }
  await typing
  await pause(900)

  await control.call("requestHelp", { displayId: id, reason: "Please check the draft before I publish it." })
  // Long enough to read the request.
  await pause(3000)
  // Let … Continue takes the place of Take Over, which shares its right edge.
  const header = capture("demo-header", [fx, fy, fw, 52])
  const span = probe("span", header, "52", "255", "159", "10").split(" ").map(Number)
  if (span.length !== 2) throw new Error("no Take Over button in the Viewer's header")
  const button = [fx + span[1] / 2 - 30, fy + 26]

  // The user clicks into the agent's screen, below the text, and adds a line.
  await pointAt(0.35, 0.55)
  await control.call("debugViewerInput", { kind: "click", x: 0.35, y: 0.55 })
  await pause(500)
  await control.call("debugViewerInput", { kind: "click", x: 0.35, y: 0.55 })
  await pause(600)
  // The agent waits out the Takeover.
  const resumed = control.call("waitForControl", { displayId: id, timeoutSeconds: 60 })
  for (const ch of "Looks good, ship it.") {
    const lower = ch.toLowerCase()
    await control.call("debugViewerInput", { kind: "key", code: codes[lower], shift: ch !== lower ? 1 : 0 })
    await pause(70)
  }
  await pause(900)
  await user(`move ${button[0]} ${button[1]}`)
  await pause(300)
  await control.call("debugShow", { surface: "primary", displayId: id })
  await resumed
  await pointAt(0.6, 0.45)
  await control.call("type", { displayId: id, text: " Publishing now." })
  await pause(2000)
  const done = Date.now()
  const text = documentText()
  console.log("TextEdit:", JSON.stringify(text))
  if (!text.includes("Looks good, ship it.") || !text.endsWith("Publishing now.")) throw new Error("the story didn't reach TextEdit")

  // screencapture starts late and runs its full length: cut where the story ends.
  await new Promise((resolve) => recorder!.once("exit", resolve))
  recorder = undefined
  const length = 45 - (Date.now() - done) / 1000
  if (!existsSync(movie)) throw new Error("screencapture recorded nothing")
  mkdirSync(media, { recursive: true })
  const ffmpeg = spawnSync("ffmpeg", ["-y", "-loglevel", "error", "-t", length.toFixed(1), "-i", movie, "-vf",
    "fps=12,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=4",
    gif], { encoding: "utf8" })
  if (ffmpeg.status !== 0) throw new Error(ffmpeg.stderr)
  console.log(gif)
} finally {
  recorder?.kill()
  notes?.kill()
  sh("/usr/bin/osascript", ["-e", 'tell application "TextEdit" to close every document saving no\ntell application "TextEdit" to quit'], true)
  control.close()
  await stopTestApp()
}

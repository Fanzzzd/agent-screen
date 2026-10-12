/**
 * Reliability checks from the launch reviews: what an agent gets back, and
 * that nothing lands on the user's screen. Run with the app open and
 * TextEdit and Calculator not in use:
 *
 *     bun rust/tests/mcp/robust.ts
 *
 * With SIDESCREEN_INSTANCE=e2e it starts the debug app beside the user's
 * and quits it afterwards, so it can run while the user works.
 */
import { execFileSync } from "node:child_process"
import { existsSync } from "node:fs"
import { McpClient, instance } from "./client.ts"
import { spawn } from "node:child_process"
import { Control, buildTools, startTestApp, stopTestApp, waitForIdle } from "../e2e/lib.ts"

let failures = 0
const check = (label: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}${detail ? `: ${detail}` : ""}`)
  if (!ok) failures++
}
const running = (name: string) => {
  try {
    execFileSync("/usr/bin/pgrep", ["-x", name])
    return true
  } catch {
    return false
  }
}
const quit = (name: string, discard = false) => {
  const script = discard
    ? `tell application "${name}" to close every document saving no\ntell application "${name}" to quit`
    : `quit app "${name}"`
  try {
    execFileSync("/usr/bin/osascript", ["-e", script])
  } catch {}
}
/** On-screen windows on a display ("main" or an id), from tools/onscreen.swift. */
const lister = new URL("../../target/e2e/onscreen", import.meta.url).pathname
if (!existsSync(lister)) {
  execFileSync("/usr/bin/swiftc", ["-O", new URL("tools/onscreen.swift", import.meta.url).pathname, "-o", lister])
}
const windowsOn = (display: string | number) =>
  execFileSync(lister, [String(display)]).toString().split("\n").filter(Boolean).map((l) => JSON.parse(l))
/** Windows of an app on the user's main display, which must stay empty. */
const onMainDisplay = (app: string) => {
  const found = windowsOn("main").filter((w) => w.owner === app && w.layer === 0)
  for (const w of found) console.log(`  on main: ${w.name} at ${w.x},${w.y}`)
  return found.length
}

if (running("TextEdit") || running("Calculator")) {
  console.log("skip: TextEdit or Calculator is the user's")
  process.exit(0)
}

// Launching windows can show them on the user's screen for a moment.
buildTools()
if (instance && !(await waitForIdle())) process.exit(1)
if (instance) await startTestApp()
const flashTool = new URL("../../target/e2e/flash", import.meta.url).pathname
if (!existsSync(flashTool)) {
  execFileSync("/usr/bin/swiftc", ["-O", new URL("tools/flash.swift", import.meta.url).pathname, "-o", flashTool])
}
/**
 * Watch the main display for TextEdit windows while `work` runs. A starting
 * app is hidden until its window is on the display, so it never shows on the
 * user's screen for more than two frames. A running app's new window shows
 * there until the catch moves it: flash.ts measured 29 ms median, 55 ms p90
 * and 138 ms at most over 30 launches.
 */
const flashLimit = 34
const runningFlashLimit = 150
const watchFlash = async (work: () => Promise<unknown>) => {
  const watcher = spawn(flashTool, ["TextEdit"], { stdio: ["pipe", "pipe", "inherit"] })
  let out = ""
  watcher.stdout!.on("data", (d) => (out += d))
  await Bun.sleep(100)
  await work()
  watcher.stdin!.end()
  await new Promise((r) => watcher.once("exit", r))
  return JSON.parse(out) as { windows: number; ms: number }
}
const client = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `robust-${Date.now()}`})
const structured = (r: any) => r.result?.structuredContent
const textOf = (r: any) => r.result?.content?.find((c: any) => c.type === "text")?.text ?? ""
const imageBytes = (r: any) => {
  const image = r.result?.content?.find((c: any) => c.type === "image")
  return image ? Buffer.from(image.data, "base64").length : 0
}

const { display_id } = structured(await client.request("tools/call", { name: "create_display", arguments: { title: "Robust" } }))
try {
  // An empty display: every screenshot succeeds, small, with no wallpaper.
  const sizes: number[] = []
  const errors: string[] = []
  for (let i = 0; i < 20; i++) {
    const shot = await client.request("tools/call", { name: "computer", arguments: { display_id, action: "screenshot" } })
    if (shot.result?.isError) {
      errors.push(textOf(shot))
      if (errors.length === 1) console.log("  on the display:", JSON.stringify(windowsOn(display_id)))
    }
    else {
      sizes.push(imageBytes(shot))
      const image = shot.result.content.find((c: any) => c.type === "image")
      // Keep a large one to see what got in.
      if (imageBytes(shot) >= 30_000) await Bun.write(`/tmp/robust-large-${i}.jpg`, Buffer.from(image.data, "base64"))
    }
  }
  check("20 screenshots of an empty display succeed", errors.length === 0, `${errors.length} failed${errors[0] ? `, first: ${errors[0]}` : ""}`)
  check("an empty display's screenshot is plain", sizes.length > 0 && Math.max(...sizes) < 30_000, `largest ${Math.max(...sizes)} bytes`)

  // TextEdit from not running: a document window on the agent's display.
  let launched: any
  const firstFlash = await watchFlash(async () => {
    launched = await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit" } })
  })
  check("TextEdit's first window shows on the user's screen 34 ms at most", firstFlash.ms <= flashLimit, `${firstFlash.windows} windows for ${firstFlash.ms} ms`)
  const window = structured(launched)?.window
  check("TextEdit's first launch opens a document window", window?.app === "TextEdit", textOf(launched))
  check("no TextEdit window on the user's screen", onMainDisplay("TextEdit") === 0)
  const docText = async (id: number) =>
    textOf(await client.request("tools/call", { name: "get_window_state", arguments: { display_id, window_id: id } }))
      .match(/textarea[^\n]*value="([^"]*)"/)?.[1] ?? ""
  if (window) {
    check("the new document starts empty, with nothing restored", (await docText(window.windowId)) === "", JSON.stringify(await docText(window.windowId)))
    // The judge's sequence: type, select all, type over it, with no click.
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "type", text: "one two" } })
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "key", text: "cmd+a" } })
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "type", text: "X" } })
    check("type, cmd+a, type replaces the text", (await docText(window.windowId)) === "X", JSON.stringify(await docText(window.windowId)))
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "key", text: "cmd+a" } })
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "key", text: "BackSpace" } })
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "type", text: "Hello from an agent" } })
    const state = await client.request("tools/call", { name: "get_window_state", arguments: { display_id, window_id: window.windowId } })
    check("typing reaches the document", textOf(state).includes("Hello from an agent"))

    // Menu shortcuts reach an app in the background.
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "key", text: "cmd+a" } })
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "type", text: "Replaced" } })
    const replaced = await client.request("tools/call", { name: "get_window_state", arguments: { display_id, window_id: window.windowId } })
    const value = textOf(replaced).match(/textarea[^\n]*value="([^"]*)"/)?.[1]
    check("cmd+a selects all in a background app", value === "Replaced", JSON.stringify(value))

    // The user takes over in the middle of a long type: it stops there,
    // and the agent hears how far it got.
    {
      const viewer = await Control.connect()
      const long = "x".repeat(2000)
      const typing = client.request("tools/call", { name: "computer", arguments: { display_id, action: "type", text: long } })
      await Bun.sleep(400)
      await viewer.call("debugTakeover", { displayId: display_id, active: true })
      const stopped = await typing
      await viewer.call("debugTakeover", { displayId: display_id, active: false })
      viewer.close()
      check("a takeover stops the agent's typing partway", /took over display \d+ after \d+ of 2000 characters/.test(textOf(stopped)), textOf(stopped).slice(0, 140))
      const typed = Number(textOf(stopped).match(/after (\d+) of/)?.[1] ?? -1)
      check("it stopped partway, not at the start or the end", typed > 0 && typed < 2000, String(typed))
    }
  }

  // Two sessions launching the same app at once each get their own window.
  {
    const other = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `robust-other-${Date.now()}` })
    const second = structured(await other.request("tools/call", { name: "create_display", arguments: { title: "Robust 2" } })).display_id
    const [a, b] = await Promise.all([
      client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit" } }),
      other.request("tools/call", { name: "launch_app", arguments: { display_id: second, app: "TextEdit" } }),
    ])
    const wa = structured(a)?.window?.windowId
    const wb = structured(b)?.window?.windowId
    check("concurrent launches get distinct windows", wa !== undefined && wb !== undefined && wa !== wb, `${wa} ${wb} ${textOf(a).slice(0, 80)} ${textOf(b).slice(0, 80)}`)
    const onSecond = (structured(await other.request("tools/call", { name: "list_windows", arguments: { display_id: second } })).windows as any[]).map((w) => w.windowId)
    check("the second session's window is on its display", onSecond.includes(wb), JSON.stringify(onSecond))
    check("no window on the user's screen after concurrent launches", onMainDisplay("TextEdit") === 0)
    await other.request("tools/call", { name: "end_display", arguments: { display_id: second } })
    other.close()
  }

  // Launched again while running: a second document, still not on the user's screen.
  let again: any
  const againFlash = await watchFlash(async () => {
    again = await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit" } })
  })
  check("a running app's new window shows on the user's screen 150 ms at most", againFlash.ms <= runningFlashLimit, `${againFlash.windows} windows for ${againFlash.ms} ms`)
  check("TextEdit launched again opens a window on the display", structured(again)?.window?.app === "TextEdit", textOf(again))
  check("still no TextEdit window on the user's screen", onMainDisplay("TextEdit") === 0)

  // A file opened in the running app: the window showing it is the agent's.
  const file = `/tmp/sidescreen robust ${Date.now()}.txt`
  await Bun.write(file, "robust\n")
  const opened = await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit", open: file } })
  const fileWindow = structured(opened)?.window
  check("a file opened in a running app lands on the display", fileWindow?.title?.startsWith("sidescreen robust"), textOf(opened).slice(0, 160))
  check("the file's window isn't on the user's screen", onMainDisplay("TextEdit") === 0)
  execFileSync("/bin/rm", ["-f", file])
  // The same with a file URL.
  const urlFile = `/tmp/sidescreen url ${Date.now()}.txt`
  await Bun.write(urlFile, "robust\n")
  const byUrl = await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit", open: `file://${encodeURI(urlFile)}` } })
  check("a file:// URL opens like its path", structured(byUrl)?.window?.title?.startsWith("sidescreen url"), textOf(byUrl).slice(0, 160))
  execFileSync("/bin/rm", ["-f", urlFile])

  // Copy and paste between two documents, through accessibility.
  {
    const docs = (structured(await client.request("tools/call", { name: "list_windows", arguments: { display_id } })).windows as any[])
      .filter((w) => w.app === "TextEdit")
    const area = async (id: number) =>
      Number(textOf(await client.request("tools/call", { name: "get_window_state", arguments: { display_id, window_id: id } }))
        .match(/\[(\d+)\] textarea/)?.[1])
    const [from, to] = docs
    const long = "Copied text that runs well past the eighty characters the window tree keeps of any value."
    const source = await area(from.windowId)
    await client.request("tools/call", { name: "act_on_element", arguments: { display_id, window_id: from.windowId, element: source, action: "set_value", value: long } })
    const read = textOf(await client.request("tools/call", { name: "act_on_element", arguments: { display_id, window_id: from.windowId, element: source, action: "read_text" } }))
    check("read_text returns the whole value", read.includes(long), read.slice(0, 120))
    const target = await area(to.windowId)
    await client.request("tools/call", { name: "act_on_element", arguments: { display_id, window_id: to.windowId, element: target, action: "set_value", value: "Before. After." } })
    const pasted = await client.request("tools/call", { name: "act_on_element", arguments: { display_id, window_id: to.windowId, element: target, action: "insert_text", value: "Pasted. " } })
    const after = textOf(await client.request("tools/call", { name: "act_on_element", arguments: { display_id, window_id: to.windowId, element: target, action: "read_text" } }))
    check("insert_text pastes into a background app", !pasted.result?.isError && after.includes("Pasted."), after.slice(0, 120))
  }

  // set_value on something without a settable value fails.
  await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "Calculator" } })
  const calc = (structured(await client.request("tools/call", { name: "list_windows", arguments: { display_id } })).windows as any[])
    .find((w) => w.app === "Calculator")
  const tree = textOf(await client.request("tools/call", { name: "get_window_state", arguments: { display_id, window_id: calc.windowId } }))
  const two = Number(tree.match(/\[(\d+)\] button "2"/)?.[1])
  const set = await client.request("tools/call", {
    name: "act_on_element",
    arguments: { display_id, window_id: calc.windowId, element: two, action: "set_value", value: "9" }
  })
  check("set_value on a button fails", set.result?.isError === true, textOf(set).slice(0, 120))

  // place_window takes only strays of apps this session started.
  const strays = structured(await client.request("tools/call", { name: "list_windows", arguments: { display_id, strays: true } }))
  check("list_windows strays lists none when every window is on the display", Array.isArray(strays?.windows) && strays.windows.length === 0, JSON.stringify(strays))
  const notOurs = await client.request("tools/call", { name: "place_window", arguments: { display_id, window_id: 1 } })
  check("place_window refuses a window that isn't a stray of the session's apps", notOurs.result?.isError === true, textOf(notOurs).slice(0, 120))

  // Calculator runs and has one window: a second launch makes none, says
  // so soon, and takes nothing.
  const startedAgain = Date.now()
  const second = structured(await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "Calculator" } }))
  const tookMs = Date.now() - startedAgain
  check("launch_app on a running single-window app says no_new_window within 1.5 s",
    second?.window === null && second?.reason === "no_new_window" && tookMs < 1500, `${JSON.stringify(second)?.slice(0, 120)} in ${tookMs} ms`)

  // Listing the tools tells the running app an agent loaded them, for the
  // setup window.
  await client.request("tools/list", {})
  await Bun.sleep(500)
  {
    const probe = await Control.connect()
    const ready = await probe.call("debugAgentReady")
    probe.close()
    check("listing the tools tells the app which agent loaded them", ready?.agent === "smoke", JSON.stringify(ready))
  }

  // Window ownership: a window the agent's action opens goes to the display;
  // one that opens with no action near it stays put, and the agent may
  // place it.
  {
    const list = async () =>
      (structured(await client.request("tools/call", { name: "list_windows", arguments: { display_id } })).windows as any[])
        .filter((w) => w.app === "TextEdit")
    const shown = await list()
    const before = shown.length
    const front = shown[shown.length - 1]
    await client.request("tools/call", { name: "computer", arguments: { display_id, action: "left_click", coordinate: [front.x + Math.round(front.width / 2), front.y + 10] } })
    const mainAtKey = onMainDisplay("TextEdit")
    const key = await client.request("tools/call", { name: "computer", arguments: { display_id, action: "key", text: "cmd+n" } })
    await Bun.sleep(1000)
    const after = (await list()).length
    check("a window the agent's key press opens lands on the display", after === before + 1,
      `${before} -> ${after} on the display, ${mainAtKey} -> ${onMainDisplay("TextEdit")} on the user's screen; ${textOf(key).slice(0, 80)}; before ${shown.map((w) => w.title).join("|")}; after ${(await list()).map((w) => w.title).join("|")}`)
    await Bun.sleep(3500)
    const mainBefore = onMainDisplay("TextEdit")
    execFileSync("/usr/bin/osascript", ["-e", 'tell application "TextEdit" to make new document'])
    await Bun.sleep(1500)
    check("a window that opens long after the agent's last action stays put", onMainDisplay("TextEdit") === mainBefore + 1)
    const moved = await client.request("tools/call", { name: "computer", arguments: { display_id, action: "mouse_move", coordinate: [1270, 790] } })
    check("the agent's next screenshot names the window that stayed", textOf(moved).includes("call place_window with that id"), textOf(moved).slice(0, 200))
    const strays = structured(await client.request("tools/call", { name: "list_windows", arguments: { display_id, strays: true } }))
    const late = strays?.windows?.find((w: any) => w.app === "TextEdit")
    check("that window is a stray the agent may place", !!late, JSON.stringify(strays).slice(0, 160))
    if (late) {
      const placed = await client.request("tools/call", { name: "place_window", arguments: { display_id, window_id: late.windowId } })
      check("place_window moves it", placed.result?.isError !== true, textOf(placed).slice(0, 120))
    }
  }

  // The user takes a TextEdit window back: TextEdit is theirs from then on.
  // A window they make next stays on their screen and isn't a stray.
  const control = await Control.connect()
  try {
    const mine = (structured(await client.request("tools/call", { name: "list_windows", arguments: { display_id } })).windows as any[])
      .find((w) => w.app === "TextEdit")
    await control.call("debugTakeBack", { displayId: display_id, windowId: mine.windowId })
    execFileSync("/usr/bin/osascript", ["-e", 'tell application "TextEdit" to make new document'])
    await Bun.sleep(1500)
    const shownAfter = (structured(await client.request("tools/call", { name: "list_windows", arguments: { display_id } })).windows as any[])
      .filter((w) => w.app === "TextEdit").map((w) => `${w.windowId} ${w.title}`)
    check("after a take-back, a window the user makes stays on their screen", onMainDisplay("TextEdit") >= 2, `taken back ${mine.windowId}; on the display: ${shownAfter.join(", ")}`)
    const after = structured(await client.request("tools/call", { name: "list_windows", arguments: { display_id, strays: true } }))
    check("after a take-back, strays lists none of the user's windows", after?.windows?.length === 0, JSON.stringify(after).slice(0, 160))
    const back = await client.request("tools/call", { name: "place_window", arguments: { display_id, window_id: mine.windowId } })
    check("place_window refuses the window taken back", back.result?.isError === true && textOf(back).includes("took that window back"), textOf(back).slice(0, 120))
  } finally {
    control.close()
  }
} finally {
  await client.request("tools/call", { name: "end_display", arguments: { display_id } })
  quit("TextEdit", true)
  quit("Calculator")
  client.close()
}
if (instance) await stopTestApp()
console.log(failures ? `${failures} failed` : "all passed")
process.exit(failures ? 1 : 0)

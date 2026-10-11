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
import { startTestApp, stopTestApp } from "../e2e/lib.ts"

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

if (instance) await startTestApp()
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
  const launched = await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit" } })
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
  const again = await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit" } })
  check("TextEdit launched again opens a window on the display", structured(again)?.window?.app === "TextEdit", textOf(again))
  check("still no TextEdit window on the user's screen", onMainDisplay("TextEdit") === 0)

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
} finally {
  await client.request("tools/call", { name: "end_display", arguments: { display_id } })
  quit("TextEdit", true)
  quit("Calculator")
  client.close()
}
if (instance) await stopTestApp()
console.log(failures ? `${failures} failed` : "all passed")
process.exit(failures ? 1 : 0)

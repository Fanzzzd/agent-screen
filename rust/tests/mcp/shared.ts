/**
 * An app the user and an agent both use: keys go to the app's key window,
 * so the agent's typing must reach only its window. Opens one TextEdit
 * window on the user's screen, behind their front app, and closes it at
 * the end.
 *
 * Not covered: a window a shortcut opens. TextEdit in the background
 * keeps its first window key whatever focus record it gets, so keys after
 * cmd+n reach the earlier window until the agent clicks the new one; the
 * computer tool's description says so.
 *
 * Not covered either: handing key focus back to the user's window. Only a real
 * click makes a background app's other window key (AXRaise doesn't), and
 * that click would bring TextEdit to the front, taking the user's focus.
 *
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/mcp/shared.ts
 */
import { execFileSync } from "node:child_process"
import { McpClient, instance } from "./client.ts"
import { buildTools, startTestApp, stopTestApp, waitForIdle } from "../e2e/lib.ts"

let failures = 0
const check = (label: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}${detail ? `: ${detail}` : ""}`)
  if (!ok) failures++
}
const osa = (script: string) => execFileSync("/usr/bin/osascript", ["-e", script]).toString().trim()
const textOf = (name: string) =>
  osa(`tell application "System Events" to tell process "TextEdit" to get value of text area 1 of scroll area 1 of window "${name}"`)

try {
  execFileSync("/usr/bin/pgrep", ["-x", "TextEdit"])
  console.log("skip: TextEdit is the user's")
  process.exit(0)
} catch {}

buildTools()
if (instance && !(await waitForIdle())) process.exit(1)
if (instance) await startTestApp()
const client = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `shared-${Date.now()}` })
const structured = (r: any) => r.result?.structuredContent
const said = (r: any) => r.result?.content?.find((c: any) => c.type === "text")?.text ?? ""
const { display_id } = structured(await client.request("tools/call", { name: "create_display", arguments: { title: "Shared" } }))
const type = (text: string) =>
  client.request("tools/call", { name: "computer", arguments: { display_id, action: "type", text } })
try {
  const launched = structured(await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit" } }))
  const agent = launched?.window?.title
  check("the agent has a TextEdit window", !!agent, JSON.stringify(launched))
  await type("first ")
  // Long after the agent's last action, so it stays on the user's screen.
  await Bun.sleep(3500)
  osa(`tell application "TextEdit" to make new document`)
  await Bun.sleep(800)
  const user = osa(`tell application "System Events" to tell process "TextEdit" to get name of every window`)
    .split(", ").find((n) => n !== agent)!

  const typed = await type("second")
  check("the agent's typing succeeds", typed.result?.isError !== true, said(typed).slice(0, 160))
  check("it lands in the agent's window", textOf(agent).toLowerCase() === "first second", JSON.stringify(textOf(agent)))
  check("none of it lands in the user's window", textOf(user) === "", JSON.stringify(textOf(user)))

  await client.request("tools/call", { name: "computer", arguments: { display_id, action: "key", text: "return" } })
  await type("third")
  check("the next call reaches the agent's window again", textOf(agent).toLowerCase() === "first second\nthird", JSON.stringify(textOf(agent)))
  check("still nothing in the user's window", textOf(user) === "", JSON.stringify(textOf(user)))

} finally {
  await client.request("tools/call", { name: "end_display", arguments: { display_id } })
  try {
    osa(`tell application "TextEdit" to close every document saving no\ntell application "TextEdit" to quit`)
  } catch {
    // TextEdit started for this test only.
    execFileSync("/usr/bin/pkill", ["-x", "TextEdit"])
  }
  client.close()
  if (instance) await stopTestApp()
}
console.log(failures ? `${failures} failed` : "all passed")
process.exit(failures ? 1 : 0)

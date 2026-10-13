/**
 * A file Open panel on an Agent Display. AppKit draws the panel in a
 * separate process (the Open and Save Panel Service); the app owns only an
 * empty window in front of it. The agent's keys, clicks and window state
 * must reach the panel. Opens Preview on the agent's display and quits it.
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/mcp/openpanel.ts
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

try {
  execFileSync("/usr/bin/pgrep", ["-x", "Preview"])
  console.log("skip: Preview is the user's")
  process.exit(0)
} catch {}

buildTools()
if (instance && !(await waitForIdle())) process.exit(1)
if (instance) await startTestApp()
const client = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `openpanel-${Date.now()}` })
const structured = (r: any) => r.result?.structuredContent
const said = (r: any) => r.result?.content?.find((c: any) => c.type === "text")?.text ?? ""
const call = (name: string, args: any) => client.request("tools/call", { name, arguments: args })
execFileSync("/bin/cp", ["/tmp/openpanel.png", "/tmp/openpanel-pick.png"])
const { display_id } = structured(await call("create_display", { title: "Open panel" }))
const computer = (args: any) => call("computer", { display_id, ...args })
const panel = async () => {
  const listed = structured(await call("list_windows", { display_id }))
  return (listed?.windows ?? []).find((w: any) => w.title === "Open")
}
const waitFor = async (want: boolean) => {
  for (let i = 0; i < 30; i++) {
    if (!!(await panel()) === want) return true
    await Bun.sleep(200)
  }
  return false
}
try {
  const launched = structured(await call("launch_app", { display_id, app: "Preview", open: "/tmp/openpanel.png" }))
  check("Preview opens a window", !!launched?.window, JSON.stringify(launched).slice(0, 200))
  await computer({ action: "key", text: "cmd+o" })
  check("cmd+o shows the Open panel", await waitFor(true))
  const open = await panel()

  const state = await call("get_window_state", { display_id, window_id: open?.windowId })
  check("its window state reads", state.result?.isError !== true, said(state).slice(0, 200))
  check("and shows its buttons", /Cancel/.test(JSON.stringify(state.result)), said(state).slice(0, 200))

  await computer({ action: "key", text: "escape" })
  check("escape closes it", await waitFor(false))

  await computer({ action: "key", text: "cmd+o" })
  check("cmd+o shows it again", await waitFor(true))
  const again = await panel()
  // Cancel sits at the bottom right of the panel.
  const shot = structured(await computer({ action: "screenshot" }))
  const scale = shot?.scale ?? 1
  const at = [again.x + again.width - 140, again.y + again.height - 25].map((v: number) => Math.round(v * scale))
  const clicked = await computer({ action: "left_click", coordinate: at })
  check("a click on Cancel succeeds", clicked.result?.isError !== true, said(clicked).slice(0, 160))
  check("and closes it", await waitFor(false))

  // Pick a file by path, as an agent uploading one would.
  await computer({ action: "key", text: "cmd+o" })
  check("cmd+o shows it once more", await waitFor(true))
  const snap = async (name: string) => {
    const r = await computer({ action: "screenshot" })
    const img = r.result?.content?.find((c: any) => c.type === "image")?.data
    if (img && process.env.SNAP) await Bun.write(`/tmp/op-${name}.png`, Buffer.from(img, "base64"))
  }
  await computer({ action: "key", text: "cmd+shift+g" })
  await Bun.sleep(600)
  await snap("1-goto")
  await computer({ action: "type", text: "/tmp/openpanel-pick.png" })
  await Bun.sleep(400)
  await snap("2-typed")
  await computer({ action: "key", text: "return" })
  await Bun.sleep(600)
  await snap("3-return")
  await computer({ action: "key", text: "return" })
  let picked = false
  for (let i = 0; i < 30 && !picked; i++) {
    const listed = structured(await call("list_windows", { display_id }))
    picked = (listed?.windows ?? []).some((w: any) => w.title.startsWith("openpanel-pick"))
    if (!picked) await Bun.sleep(200)
  }
  check("go to folder, a path and return open the file", picked)
} finally {
  await call("end_display", { display_id })
  try {
    osa(`tell application "Preview" to quit`)
  } catch {
    execFileSync("/usr/bin/pkill", ["-x", "Preview"])
  }
  client.close()
  if (instance) await stopTestApp()
}
console.log(failures ? `${failures} failed` : "all passed")

/**
 * How long a running app's new window shows on the user's screen before it
 * moves: TextEdit launched again RUNS times (default 15), with the
 * distribution. TextEdit must not be in use.
 *
 *     SIDESCREEN_INSTANCE=e2e E2E_NOW=1 bun rust/tests/mcp/flash.ts
 *     FILES=1 SIDESCREEN_RUNNING_TURN_MS=0 ...   a file window that comes late
 */
import { execFileSync, spawn } from "node:child_process"
import { existsSync } from "node:fs"
import { McpClient, instance } from "./client.ts"
import { buildTools, startTestApp, stopTestApp, waitForIdle } from "../e2e/lib.ts"

try {
  execFileSync("/usr/bin/pgrep", ["-x", "TextEdit"])
  console.log("skip: TextEdit is the user's")
  process.exit(0)
} catch {}
buildTools()
if (instance && !(await waitForIdle())) process.exit(1)
if (instance) await startTestApp()
const flashTool = new URL("../../target/e2e/flash", import.meta.url).pathname
if (!existsSync(flashTool)) {
  execFileSync("/usr/bin/swiftc", ["-O", new URL("tools/flash.swift", import.meta.url).pathname, "-o", flashTool])
}
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
const client = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `flash-${Date.now()}` })
const display_id = (await client.request("tools/call", { name: "create_display", arguments: { title: "Flash" } })).result
  .structuredContent.display_id
const times: number[] = []
try {
  await client.request("tools/call", { name: "launch_app", arguments: { display_id, app: "TextEdit" } })
  for (let i = 0; i < Number(process.env.RUNS || 15); i++) {
    // FILES=1: open a new file each time. With SIDESCREEN_RUNNING_TURN_MS=0
    // each one takes the path of a file that opens after the 4 s turn.
    const file = process.env.FILES === "1" ? `/tmp/sidescreen flash ${Date.now()}.txt` : undefined
    if (file) await Bun.write(file, "flash\n")
    const flash = await watchFlash(async () => {
      const reply = await client.request("tools/call", {
        name: "launch_app",
        arguments: { display_id, app: "TextEdit", ...(file ? { open: file } : {}) }
      })
      // A late file window moves after the reply: give it the time.
      if (file && !reply.result?.structuredContent?.window) await Bun.sleep(1500)
    })
    times.push(flash.ms)
    if (file) execFileSync("/bin/rm", ["-f", file])
  }
} finally {
  await client.request("tools/call", { name: "end_display", arguments: { display_id } })
  try {
    execFileSync("/usr/bin/osascript", ["-e", 'tell application "TextEdit" to close every document saving no\ntell application "TextEdit" to quit'])
  } catch {}
  client.close()
}
if (instance) await stopTestApp()
const sorted = [...times].sort((a, b) => a - b)
console.log(`flash ms: median ${sorted[Math.floor(sorted.length / 2)]}, max ${sorted.at(-1)}, over 34: ${times.filter((t) => t > 34).length}/${times.length} (${times.join(" ")})`)

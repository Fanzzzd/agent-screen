/**
 * End-to-end smoke test: acts as an MCP client over stdio against the real
 * menu bar app. Run with the app open:
 *
 *     bun scripts/smoke.ts [app]
 *
 * Saves each screenshot to /tmp/sidescreen-smoke-N.jpg.
 */
import { spawn } from "node:child_process"
import { writeFileSync } from "node:fs"

const app = process.argv[2] ?? "Calculator"
const server = spawn("bun", ["src/main.ts"], {
  cwd: new URL("..", import.meta.url).pathname,
  env: { ...process.env, CLAUDE_CODE_SESSION_ID: `smoke-${Date.now()}`, CLAUDE_PROJECT_DIR: "/tmp/smoke" },
  stdio: ["pipe", "pipe", "inherit"]
})

let buffer = ""
let nextId = 1
const pending = new Map<number, (message: any) => void>()
server.stdout.setEncoding("utf8")
server.stdout.on("data", (chunk: string) => {
  buffer += chunk
  let newline: number
  while ((newline = buffer.indexOf("\n")) >= 0) {
    const message = JSON.parse(buffer.slice(0, newline))
    buffer = buffer.slice(newline + 1)
    pending.get(message.id)?.(message)
  }
})

const request = (method: string, params: unknown = {}) =>
  new Promise<any>((resolve) => {
    const id = nextId++
    pending.set(id, resolve)
    server.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n")
  })

let shots = 0
const call = async (name: string, args: Record<string, unknown>) => {
  const reply = await request("tools/call", { name, arguments: args })
  if (reply.error) throw new Error(`${name}: ${JSON.stringify(reply.error)}`)
  const result = reply.result
  for (const item of result.content) {
    if (item.type === "image") {
      const file = `/tmp/sidescreen-smoke-${++shots}.jpg`
      writeFileSync(file, Buffer.from(item.data, "base64"))
      console.log(`  ${name}: image → ${file}`)
    } else {
      console.log(`  ${name}${result.isError ? " (error)" : ""}: ${item.text}`)
    }
  }
  return result
}

const init = await request("initialize", {
  protocolVersion: "2025-06-18",
  capabilities: {},
  clientInfo: { name: "smoke", version: "0" }
})
console.log("protocol:", init.result?.protocolVersion)
server.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n")

const tools = await request("tools/list")
console.log("tools:", tools.result.tools.map((tool: any) => tool.name).join(", "))

const created = await call("create_display", { title: "Smoke test" })
const displayId = created.structuredContent.display_id
await call("create_display", {})  // must reuse
await Bun.sleep(1500)
await call("launch_app", { display_id: displayId, app })
await call("list_windows", { display_id: displayId })
await call("computer", { display_id: displayId, action: "type", text: "7+8=" })
await call("computer", { display_id: displayId, action: "key", text: "cmd+nope" })
await call("computer", { display_id: displayId, action: "left_click", coordinate: [5000, 10] })
await call("end_display", { display_id: displayId })
await call("computer", { display_id: displayId, action: "screenshot" })

server.stdin.end()
server.kill()

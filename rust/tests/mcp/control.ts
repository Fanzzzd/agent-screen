/**
 * Control-state test against the running debug app: Takeover refusal and
 * wait_for_control. Needs no Screen Recording or Accessibility grant.
 *
 *     bun rust/tests/mcp/control.ts
 */
import * as net from "node:net"
import { McpClient, socketPath } from "./client.ts"

/** A second control connection standing in for the user's Viewer. */
const debug = (method: string, params: Record<string, unknown>) =>
  new Promise<any>((resolve, reject) => {
    const socket = net.createConnection(socketPath)
    socket.setEncoding("utf8")
    let buffer = ""
    socket.on("data", (chunk: string) => {
      buffer += chunk
      if (!buffer.includes("\n")) return
      socket.end()
      const reply = JSON.parse(buffer)
      reply.error ? reject(new Error(reply.error.message)) : resolve(reply.result)
    })
    socket.write(JSON.stringify({ id: 1, method, params }) + "\n")
  })

let failures = 0
const check = (label: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}${detail ? `: ${detail}` : ""}`)
  if (!ok) failures++
}

const client = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `control-${Date.now()}` })
const created = await client.call("create_display", { title: "Control test" })
const display_id = created.structuredContent.display_id

await debug("debugTakeover", { displayId: display_id, active: true })
const refused = await client.call("computer", { display_id, action: "left_click", coordinate: [100, 100] })
check("input refused during Takeover", refused.isError === true && refused.content[0].text.startsWith("takeover"),
  refused.content[0].text)

const timedOut = await client.call("wait_for_control", { display_id, timeout_seconds: 1 })
check("wait_for_control times out", timedOut.structuredContent?.control === "user")

const started = Date.now()
setTimeout(() => debug("debugTakeover", { displayId: display_id, active: false }), 800)
const returned = await client.call("wait_for_control", { display_id, timeout_seconds: 10 })
check("wait_for_control returns on hand-back", returned.structuredContent?.control === "agent",
  `${Date.now() - started} ms`)

const immediate = await client.call("wait_for_control", { display_id })
check("wait_for_control returns at once with control", immediate.structuredContent?.control === "agent")

// A Help Request holds the agent until the user takes over and hands back.
const help = client.call("request_help", { display_id, reason: "Sign in to the test site", timeout_seconds: 10 })
await Bun.sleep(500)
const stillWaiting = await client.call("wait_for_control", { display_id, timeout_seconds: 1 })
check("Help Request holds the agent before any Takeover", stillWaiting.structuredContent?.control === "user")
await debug("debugTakeover", { displayId: display_id, active: true })
await Bun.sleep(300)
await debug("debugTakeover", { displayId: display_id, active: false })
const helped = await help
check("request_help returns when the Takeover ends", helped.structuredContent?.control === "agent")

const noWindow = await client.call("get_window_state", { display_id, window_id: 1 })
check("get_window_state rejects a window not on the display", noWindow.content[0].text.startsWith("not_found"))

const unknown = await client.call("set_disposition", { display_id, window_id: 1, disposition: "keep" })
check("set_disposition rejects a window not on the display", unknown.isError === true)

await debug("debugTakeover", { displayId: display_id, active: true })
setTimeout(() => client.call("end_display", { display_id }), 500)
const ended = await client.call("wait_for_control", { display_id, timeout_seconds: 10 })
check("wait_for_control returns when the display ends", ended.structuredContent?.control === "none")

const afterEnd = await client.call("set_disposition", { display_id, window_id: 1, disposition: "close" })
check("an ended display refuses the agent", afterEnd.content[0].text.startsWith("ended"))

client.close()
process.exit(failures === 0 ? 0 : 1)

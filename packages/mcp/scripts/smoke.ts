/**
 * End-to-end smoke test: acts as an MCP client over stdio against the real
 * menu bar app. Run with the app open:
 *
 *     bun scripts/smoke.ts [app]
 *
 * Saves each screenshot to /tmp/sidescreen-smoke-N.jpg.
 */
import { McpClient } from "./client.ts"

const app = process.argv[2] ?? "Calculator"
const client = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `smoke-${Date.now()}` })

const tools = await client.request("tools/list")
console.log("tools:", tools.result.tools.map((tool: any) => tool.name).join(", "))

const created = await client.call("create_display", { title: "Smoke test" })
const display_id = created.structuredContent.display_id
await client.call("create_display", {}) // must reuse
await Bun.sleep(1500)
await client.call("launch_app", { display_id, app })
await client.call("list_windows", { display_id })
await client.call("computer", { display_id, action: "type", text: "7+8=" })
await client.call("computer", { display_id, action: "key", text: "cmd+nope" })
await client.call("computer", { display_id, action: "left_click", coordinate: [5000, 10] })
await client.call("end_display", { display_id })
await client.call("computer", { display_id, action: "screenshot" })

client.close()

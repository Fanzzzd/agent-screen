/**
 * End-to-end smoke test: acts as an MCP client over stdio against the real
 * menu bar app. Run with the app open:
 *
 *     bun rust/tests/mcp/smoke.ts [app]
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
await client.call("computer", { display_id, action: "type", text: "7+8=" })

// Per-window tools: 2 × 3 through accessibility presses.
const launched = (await client.call("list_windows", { display_id })).structuredContent.windows[0]
const window_id = launched.windowId
// An element keeps its number while it stays in the window.
let state = await client.call("get_window_state", { display_id, window_id })
const element = (label: string) =>
  Number(state.content[0].text.match(new RegExp(`\\[(\\d+)\\] button "${label}"`))?.[1])
// Calculator remembers its sidebar; close it if an earlier run left it open.
if (launched.width > 300) {
  state = await client.call("act_on_element", { display_id, window_id, element: element("Hide Sidebar"), action: "press" })
}
const before = state
for (const label of ["All Clear", "2", "Multiply", "3", "Equals"]) {
  state = await client.call("act_on_element", { display_id, window_id, element: element(label), action: "press" })
}
const after = state
const shown = (s: any) => s.content[0].text.match(/"Edit field".*\n.*value="\W*(\d+)"/)?.[1]
const result = shown(after)
console.log(`act_on_element 2 × 3 → ${result} ${result === "6" ? "ok" : "FAIL"}`)
const two = before.content[0].text.match(/\[(\d+)\] button "2"/)?.[1]
console.log(`button 2 keeps its number: ${two} → ${element("2")} ${two === String(element("2")) ? "ok" : "FAIL"}`)

// The tree's centers are where the computer tool clicks.
const center = state.content[0].text.match(/button "5"[^\n]* at \((\d+),(\d+)\)/)
await client.call("act_on_element", { display_id, window_id, element: element("All Clear"), action: "press" })
await client.call("computer", { display_id, action: "left_click", coordinate: [Number(center[1]), Number(center[2])] })
const clicked = shown(await client.call("get_window_state", { display_id, window_id }))
console.log(`clicking the tree's center of 5 → ${clicked} ${clicked === "5" ? "ok" : "FAIL"}`)
await client.call("set_disposition", { display_id, window_id, disposition: "keep" })
await client.call("list_windows", { display_id })

await client.call("computer", { display_id, action: "key", text: "cmd+nope" })
await client.call("computer", { display_id, action: "left_click", coordinate: [5000, 10] })
await client.call("end_display", { display_id })
await client.call("computer", { display_id, action: "screenshot" })

client.close()

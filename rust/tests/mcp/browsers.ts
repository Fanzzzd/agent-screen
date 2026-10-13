/**
 * Web pages in a browser on an Agent Display, through Safari, which it
 * starts and quits:
 * - the first click after launch_app lands, in an app just started (whose
 *   first click only makes its window key) and on a page whose script
 *   arrives late (launch_app waits for the page to load);
 * - a URL opened while Safari runs goes to the new window, not the one
 *   Safari had active;
 * - a click on an item of a page's select menu chooses it.
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/mcp/browsers.ts
 */
import { execFileSync } from "node:child_process"
import { mkdtempSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { McpClient, instance } from "./client.ts"
import { buildTools, startTestApp, stopTestApp, waitForIdle } from "../e2e/lib.ts"

let failures = 0
const check = (label: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}${detail ? `: ${detail}` : ""}`)
  if (!ok) failures++
}
const quitSafari = () => {
  try {
    execFileSync("/usr/bin/osascript", ["-e", 'tell application "Safari" to quit'])
  } catch {}
  for (let i = 0; i < 30; i++) {
    try {
      execFileSync("/usr/bin/pgrep", ["-x", "Safari"])
    } catch {
      return
    }
    Bun.sleepSync(200)
  }
}

try {
  execFileSync("/usr/bin/pgrep", ["-x", "Safari"])
  console.log("skip: Safari is the user's")
  process.exit(0)
} catch {}

// A button that works only once its script has come, 1.5 s late; a select.
const server = Bun.serve({
  port: 0,
  async fetch(req) {
    const url = new URL(req.url)
    if (url.pathname === "/late.js") {
      await Bun.sleep(1500)
      return new Response("let n=0;b.addEventListener('click',()=>{document.title='clicked '+(++n)})", {
        headers: { "content-type": "text/javascript" }
      })
    }
    return new Response(
      `<!doctype html><title>ready</title><style>html,body{margin:0;height:100%}button{width:100vw;height:100vh}</style><button id=b>click me</button><script src="/late.js?${Math.random()}"></script>`,
      { headers: { "content-type": "text/html" } }
    )
  }
})
const dir = mkdtempSync(path.join(tmpdir(), "sidescreen-browsers-"))
writeFileSync(path.join(dir, "select.html"), `<!doctype html><title>picked none</title>
<style>body{margin:40px}select{font-size:24px;width:400px}</style>
<select id=s onchange="document.title='picked '+s.value"><option value="">Select...</option><option>Apple</option><option>Banana</option><option>Cherry</option><option>Watermelon</option></select>`)

buildTools()
if (instance && !(await waitForIdle())) process.exit(1)
if (instance) await startTestApp()
const client = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `browsers-${Date.now()}` })
const structured = (r: any) => r.result?.structuredContent
const said = (r: any) => r.result?.content?.find((c: any) => c.type === "text")?.text ?? ""
const call = (name: string, args: any) => client.request("tools/call", { name, arguments: args })
const { display_id } = structured(await call("create_display", { title: "Browsers" }))
const shot = structured(await call("computer", { display_id, action: "screenshot" }))
const display = structured(await call("list_windows", { display_id }))
const scale = (shot?.width ?? 1280) / (display?.display?.width ?? 1280)
const titleOf = async (windowId: number) =>
  ((structured(await call("list_windows", { display_id }))?.windows ?? []).find((w: any) => w.windowId === windowId)?.title ?? "") as string
const waitTitle = async (windowId: number, want: RegExp) => {
  let title = ""
  for (let i = 0; i < 15 && !want.test(title); i++) {
    title = await titleOf(windowId)
    if (!want.test(title)) await Bun.sleep(200)
  }
  return title
}
const clickMiddle = (w: any) =>
  call("computer", {
    display_id,
    action: "left_click",
    coordinate: [Math.round((w.x + w.width / 2) * scale), Math.round((w.y + w.height * 0.6) * scale)]
  })

try {
  // Safari isn't running: started here, its first window drops a click.
  const first = structured(await call("launch_app", { display_id, app: "Safari", open: `http://127.0.0.1:${server.port}/` }))
  check("Safari starts with the page", !!first?.window, JSON.stringify(first).slice(0, 160))
  await clickMiddle(first.window)
  check("the first click after launch_app lands", /clicked 1/.test(await waitTitle(first.window.windowId, /clicked/)))

  // Safari runs now: the URL must go to the new window.
  const second = structured(await call("launch_app", { display_id, app: "Safari", open: `http://127.0.0.1:${server.port}/` }))
  check("a running Safari opens a new window", !!second?.window && second.window.windowId !== first.window.windowId)
  await clickMiddle(second.window)
  check("the URL went to the new window, and its first click lands", /clicked 1/.test(await waitTitle(second.window.windowId, /clicked/)))
  check("the first window kept its page", /clicked 1/.test(await titleOf(first.window.windowId)))

  const third = structured(await call("launch_app", { display_id, app: "Safari", open: `file://${dir}/select.html` }))
  const state = said(await call("get_window_state", { display_id, window_id: third.window.windowId }))
  const select = state.match(/popupbutton[^\n]*at \((\d+),(\d+)\)/i)
  check("the select is in the window state", !!select, state.split("\n").slice(0, 3).join(" | "))
  if (select) {
    await call("computer", { display_id, action: "left_click", coordinate: [Number(select[1]), Number(select[2])] })
    await Bun.sleep(800)
    // The open menu lists the options under the select, one row each.
    const menu = said(await call("computer", { display_id, action: "screenshot" }))
    const rows = 4
    const item = [Number(select[1]), Number(select[2]) + Math.round(rows * 24 * scale)]
    await call("computer", { display_id, action: "left_click", coordinate: item })
    check("a click on a menu item chooses it", /picked \w+/.test(await waitTitle(third.window.windowId, /picked \w+/)), menu.slice(0, 60))
  }
} finally {
  await call("end_display", { display_id })
  quitSafari()
  client.close()
  server.stop()
  if (instance) await stopTestApp()
}
console.log(failures ? `${failures} failed` : "all passed")

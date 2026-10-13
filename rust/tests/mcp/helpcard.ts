/**
 * The Help Card: it drops from the menu bar icon for a Help Request, Later
 * puts it away, a new request brings it back, and Take Over or the Viewer
 * showing that display hides it. Captures the card and the Viewer header
 * under target/e2e. The card shows on the user's screen for a few seconds.
 *
 *     SIDESCREEN_INSTANCE=e2e E2E_NOW=1 bun rust/tests/mcp/helpcard.ts
 */
import { McpClient, instance } from "./client.ts"
import { Control, capture, check, failed, pidOf, startTestApp, windowsOf } from "../e2e/lib.ts"

if (!instance) throw new Error("set SIDESCREEN_INSTANCE")
await startTestApp()
const pid = pidOf("Sidescreen Dev.app/Contents/MacOS/Sidescreen")!
const control = await Control.connect()
const show = (surface: string, displayId: number) => control.call("debugShow", { surface, displayId })

/** The card: the app's 320-pt-wide window, if it is on screen. */
const card = () => windowsOf(pid).find((w) => w.width === 320 && w.layer > 0)

const client = await McpClient.start({ CLAUDE_CODE_SESSION_ID: `helpcard-${Date.now()}` })
const display_id = (await client.call("create_display", { title: "Book a flight" })).structuredContent.display_id
try {
  check("no card without a Help Request", card() === undefined)

  await client.call("request_help", { display_id, reason: "Sign in to United. It wants a code sent to your phone.", timeout_seconds: 1 })
  await Bun.sleep(600)
  const shown = card()
  check("a Help Request shows the card", shown !== undefined, JSON.stringify(shown))
  if (shown) console.log("     card:", capture("helpcard", [shown.x, shown.y, shown.width, shown.height]))

  await show("card-later", display_id)
  await Bun.sleep(300)
  check("Later puts the card away", card() === undefined)

  await client.call("request_help", { display_id, reason: "Enter the code from your phone", timeout_seconds: 1 })
  await Bun.sleep(600)
  check("a new reason brings the card back", card() !== undefined)

  await show("card-open", display_id)
  await Bun.sleep(600)
  check("the Viewer on that display hides the card", card() === undefined)
  const viewer = windowsOf(pid).find((w) => w.width > 400 && w.layer > 0)
  if (viewer) console.log("     header:", capture("viewer-header", [viewer.x, viewer.y, viewer.width, 60]))
  await show("collapse", display_id).catch(() => {})
  await Bun.sleep(500)

  await show("card-take-over", display_id)
  await Bun.sleep(600)
  check("Take Over hides the card", card() === undefined)
  await control.call("debugTakeover", { displayId: display_id, active: false })
  await Bun.sleep(300)
  check("the Takeover's end answers the request: no card", card() === undefined)
} finally {
  await client.call("end_display", { display_id }).catch(() => {})
  client.close()
  control.close()
}
process.exit(failed() ? 1 : 0)

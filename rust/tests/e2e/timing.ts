/**
 * How long createDisplay takes to answer, over a few displays.
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/e2e/timing.ts    beside the user's app
 *     bun rust/tests/e2e/timing.ts                            in its place, once the user is idle
 */
import {
  Control, buildTools, instance, reopenInstalled, restore, snapshot, startDebugApp, startTestApp, stopDebugApp,
  stopTestApp, waitForQuiet, waitForUnlocked
} from "./lib.ts"

buildTools()
if (!(await (instance ? waitForUnlocked() : waitForQuiet()))) process.exit(1)
const before = instance ? undefined : snapshot()
await (instance ? startTestApp() : startDebugApp())
const control = await Control.connect()
const times: number[] = []
try {
  for (let i = 0; i < 5; i++) {
    const start = performance.now()
    const display = await control.call("createDisplay", { sessionKey: `timing-${i}`, title: "Timing" })
    times.push(Math.round(performance.now() - start))
    await control.call("endDisplay", { displayId: display.displayId })
    await Bun.sleep(1500)
  }
  console.log("createDisplay ms:", times.join(" "))
} finally {
  control.close()
  if (instance) {
    await stopTestApp()
  } else {
    await Bun.sleep(800)
    stopDebugApp()
    reopenInstalled()
    restore(before!)
  }
}

/**
 * How long createDisplay takes to answer, over a few displays.
 *
 *     bun rust/tests/e2e/timing.ts
 */
import { Control, buildTools, reopenInstalled, restore, snapshot, startDebugApp, stopDebugApp, waitForQuiet } from "./lib.ts"

buildTools()
if (!(await waitForQuiet())) process.exit(1)
const before = snapshot()
await startDebugApp()
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
  await Bun.sleep(800)
  stopDebugApp()
  reopenInstalled()
  restore(before)
}

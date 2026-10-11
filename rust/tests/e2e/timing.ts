/**
 * How long createDisplay takes to answer: median and p90 over 10 displays.
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/e2e/timing.ts    beside the user's app
 *     bun rust/tests/e2e/timing.ts                            in its place, once the user is idle
 */
import {
  Control, buildTools, instance, sh, reopenInstalled, restore, snapshot, startDebugApp, startTestApp, stopDebugApp,
  stopTestApp, waitForQuiet, waitForUnlocked
} from "./lib.ts"

buildTools()
if (!(await (instance ? waitForUnlocked() : waitForQuiet()))) process.exit(1)
const before = instance ? undefined : snapshot()
await (instance ? startTestApp() : startDebugApp())
const control = await Control.connect()
const times: number[] = []
try {
  for (let i = 0; i < Number(process.env.RUNS || 10); i++) {
    const start = performance.now()
    const display = await control.call("createDisplay", { sessionKey: `timing-${i}`, title: "Timing" })
    times.push(Math.round(performance.now() - start))
    await control.call("endDisplay", { displayId: display.displayId })
    await Bun.sleep(1500)
  }
  const sorted = [...times].sort((a, b) => a - b)
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)]
  const aerospace = sh("/usr/bin/pgrep", ["-x", "AeroSpace"], true) ? "with AeroSpace" : "without AeroSpace"
  console.log(`createDisplay ms ${aerospace}: median ${at(0.5)}, p90 ${at(0.9)} (${times.join(" ")})`)
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

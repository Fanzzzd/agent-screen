/**
 * How long createDisplay takes to answer, median and p90 over 10 displays,
 * and launchApp for TextEdit, cold and already running, over 5 launches.
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/e2e/timing.ts    beside the user's app
 *     bun rust/tests/e2e/timing.ts                            in its place, once the user is idle
 */
import {
  Control,
  buildTools,
  instance,
  running,
  sh,
  reopenInstalled,
  restore,
  snapshot,
  startDebugApp,
  startTestApp,
  stopDebugApp,
  stopTestApp,
  waitForQuiet,
  waitForUnlocked,
} from "./lib.ts";

buildTools();
if (!(await (instance ? waitForUnlocked() : waitForQuiet()))) process.exit(1);
const before = instance ? undefined : snapshot();
await (instance ? startTestApp() : startDebugApp());
const control = await Control.connect();
const times: number[] = [];
const report = (what: string, values: number[]) => {
  const sorted = values.toSorted((a, b) => a - b);
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)];
  console.log(`${what}: median ${at(0.5)}, p90 ${at(0.9)} (${values.join(" ")})`);
};
const quitTextEdit = () =>
  sh(
    "/usr/bin/osascript",
    [
      "-e",
      'tell application "TextEdit" to close every document saving no\ntell application "TextEdit" to quit',
    ],
    true,
  );
try {
  for (let i = 0; i < Number(process.env.RUNS || 10); i++) {
    const start = performance.now();
    const display = await control.call<{ displayId: number }>("createDisplay", {
      sessionKey: `timing-${i}`,
      title: "Timing",
    });
    times.push(Math.round(performance.now() - start));
    await control.call("endDisplay", { displayId: display.displayId });
    await Bun.sleep(1500);
  }
  const aerospace = sh("/usr/bin/pgrep", ["-x", "AeroSpace"], true)
    ? "with AeroSpace"
    : "without AeroSpace";
  report(`createDisplay ms ${aerospace}`, times);

  if (running("TextEdit")) {
    console.log("skip launchApp: TextEdit is the user's");
  } else {
    const cold: number[] = [];
    const warm: number[] = [];
    for (let i = 0; i < Number(process.env.LAUNCHES || 5); i++) {
      const display = await control.call<{ displayId: number }>("createDisplay", {
        sessionKey: `launch-${i}`,
        title: "Timing",
      });
      for (const into of [cold, warm]) {
        const start = performance.now();
        await control.call("launchApp", { displayId: display.displayId, app: "TextEdit" });
        into.push(Math.round(performance.now() - start));
      }
      quitTextEdit();
      await Bun.sleep(1000);
      await control.call("endDisplay", { displayId: display.displayId });
      await Bun.sleep(1500);
    }
    report(`launchApp TextEdit cold ms ${aerospace}`, cold);
    report(`launchApp TextEdit running ms ${aerospace}`, warm);
  }
} finally {
  control.close();
  if (instance) {
    await stopTestApp();
  } else {
    await Bun.sleep(800);
    stopDebugApp();
    reopenInstalled();
    restore(before!);
  }
}

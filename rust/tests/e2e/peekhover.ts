/**
 * Peek opened by hovering the menu bar icon closes once the pointer has
 * left the icon and Peek, however it left. Hover is reported to the app
 * the way AppKit and GPUI report it, so the user's pointer is untouched
 * and this runs at any time. Peek opens on a stage display.
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/e2e/peekhover.ts
 */
import {
  Control,
  buildTools,
  check,
  failed,
  instance,
  pidOf,
  startTestApp,
  stopTestApp,
  windowsOf,
} from "./lib.ts";

if (!instance) throw new Error("run with SIDESCREEN_INSTANCE=e2e");
buildTools();
await startTestApp({ SIDESCREEN_LANG: "en" });
const control = await Control.connect();
const pid = pidOf("Sidescreen Dev.app/Contents/MacOS/Sidescreen");
if (!pid) throw new Error("no test app");

/** Each call waits 900 ms in the app, longer than Peek's hover delays. */
const show = (surface: string, displayId = 0) => control.call("debugShow", { surface, displayId });

try {
  const stage = await control.call<{ displayId: number }>("createDisplay", {
    sessionKey: "stage",
    title: "Stage",
    width: 1728,
    height: 1117,
  });
  await show("stage", stage.displayId);
  const before = new Set(windowsOf(pid).map((w) => w.id));
  const peekShown = () => windowsOf(pid).some((w) => !before.has(w.id));

  await show("icon-hover", 1);
  check("hovering the icon opens Peek", peekShown());
  await show("icon-hover", 0);
  check("leaving the icon closes it", !peekShown());

  // The pointer goes onto Peek, and Escape closes it there. The pointer
  // then leaves Peek's spot, which is gone, so nothing reports that.
  await show("icon-hover", 1);
  await show("icon-hover", 0);
  await show("card-hover", 1);
  await show("hide");
  check("Escape closes Peek", !peekShown());
  await show("icon-hover", 1);
  check("hovering the icon opens it again", peekShown());
  await show("icon-hover", 0);
  check("leaving the icon closes it again", !peekShown());
} finally {
  control.close();
  await stopTestApp();
}
console.log(failed() ? `${failed()} failed` : "all passed");
process.exit(failed() ? 1 : 0);

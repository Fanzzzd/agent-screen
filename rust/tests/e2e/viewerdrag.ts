/**
 * The user drags a window by its title bar in the Viewer: the window moves
 * on the Agent Display, as far as the drag went, and stays there. Posted
 * events can't do that: WindowServer moves a window being dragged to the
 * real pointer, on the user's screen. The Viewer opens on a stage display
 * and the drag is made through its own handler, so the user's pointer is
 * untouched and this runs at any time.
 *
 *     SIDESCREEN_INSTANCE=e2e bun rust/tests/e2e/viewerdrag.ts
 */
import {
  type Rect,
  Control,
  check,
  failed,
  instance,
  running,
  sh,
  startTestApp,
  stopTestApp,
} from "./lib.ts";

if (!instance) throw new Error("run with SIDESCREEN_INSTANCE=e2e");
if (running("Calculator")) {
  console.log("skip: Calculator is running, and may be the user's");
  process.exit(0);
}
await startTestApp({ SIDESCREEN_LANG: "en" });
const control = await Control.connect();

type Listed = {
  windowId: number;
  app: string;
  x: number;
  y: number;
  width: number;
  height: number;
};
const calculator = async (displayId: number) =>
  (await control.call<{ windows: Listed[] }>("listWindows", { displayId })).windows.find(
    (w) => w.app === "Calculator",
  );

try {
  const stage = await control.call<{ displayId: number }>("createDisplay", {
    sessionKey: "stage",
    title: "Stage",
    width: 1728,
    height: 1117,
  });
  await control.call("debugShow", { surface: "stage", displayId: stage.displayId });
  const display = await control.call<{ displayId: number; width: number; height: number }>(
    "createDisplay",
    {
      sessionKey: "viewerdrag",
      client: "Claude Code",
      title: "Viewer drag",
      width: 900,
      height: 600,
    },
  );
  const id = display.displayId;
  await control.call("launchApp", { displayId: id, app: "Calculator" });
  await control.call("debugShow", { surface: "viewer", displayId: id });
  await Bun.sleep(1200);
  const { image } = await control.call<{ image: Rect }>("debugViewer");
  const before = await calculator(id);
  check("Calculator opened on the display", before !== undefined);
  if (before) {
    const [w, h] = [display.width, display.height];
    // Left of the toolbar's buttons: SwiftUI's background there, not the
    // window itself, is what Accessibility finds.
    const from = {
      x: (before.x + before.width * 0.17) / w,
      y: (before.y + before.height * 0.03) / h,
    };
    const by = { x: 0.15, y: 0.1 };
    await control.call("debugViewerInput", {
      kind: "drag",
      x: from.x,
      y: from.y,
      toX: from.x + by.x,
      toY: from.y + by.y,
    });
    await Bun.sleep(1500);
    const after = await calculator(id);
    check("the window stays on the Agent Display", after !== undefined, JSON.stringify(after));
    if (after) {
      const moved = { x: (after.x - before.x) / w, y: (after.y - before.y) / h };
      check(
        "it moved as far as the drag went",
        Math.abs(moved.x - by.x) < 0.02 && Math.abs(moved.y - by.y) < 0.02,
        `${JSON.stringify(before)} -> ${JSON.stringify(after)}, image ${JSON.stringify(image)}`,
      );
    }
  }
} finally {
  sh("/usr/bin/osascript", ["-e", 'tell application "Calculator" to quit'], true);
  control.close();
  await stopTestApp();
}
console.log(failed() ? `${failed()} failed` : "all passed");
process.exit(failed() ? 1 : 0);

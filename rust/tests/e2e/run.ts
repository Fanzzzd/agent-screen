/**
 * End-to-end tests of the user's side: real pointer input on the Viewer, the
 * menu bar strip and the permission setup.
 *
 *     bun rust/tests/e2e/run.ts                  every case
 *     bun rust/tests/e2e/run.ts scroll strip     some of them
 *     E2E_NOW=1 bun rust/tests/e2e/run.ts        without waiting for an idle user
 *
 * They move the real cursor, so they wait until the user has been idle for
 * 45 s and has no Agent Display open, then put the cursor, the front app,
 * the workspace and the installed Sidescreen back. Captures land in
 * rust/target/e2e. Cases skip when an app they use is already the user's.
 */
import { writeFileSync } from "node:fs";
import * as path from "node:path";
import {
  type Rect,
  Control,
  buildTools,
  capture,
  check,
  digest,
  failed,
  fixture,
  hid,
  onScreen,
  out,
  pidOf,
  pixel,
  probe,
  reopenInstalled,
  voided,
  restore,
  running,
  sh,
  snapshot,
  startDebugApp,
  startUngrantedApp,
  stopApp,
  stopDebugApp,
  waitForQuiet,
  windowsOf,
} from "./lib.ts";

type Display = { displayId: number; width: number; height: number };

const settle = (ms: number) => Bun.sleep(ms);
const quitApp = (name: string) => sh("/usr/bin/osascript", ["-e", `quit app "${name}"`], true);

/** A display with the Viewer open on it, and where its image is on screen. */
const viewerOn = async (control: Control, session: string) => {
  const display = await control.call<Display>("createDisplay", {
    sessionKey: session,
    client: "E2E",
    title: session,
  });
  return { display, image: [] as number[] };
};

/** Whether the user holds the display: waitForControl answers at once when not. */
const takenOver = async (control: Control, id: number) =>
  (await control.call<{ outcome: string }>("waitForControl", { displayId: id, timeoutSeconds: 1 }))
    .outcome !== "returned";

/** Give the display back to the agent, as 让 … 继续 does. */
const release = (control: Control, id: number) =>
  control.call("debugTakeover", { displayId: id, active: false });

const showViewer = async (control: Control, display: Display) => {
  await control.call("debugShow", { surface: "viewer", displayId: display.displayId });
  await settle(1200);
  const layout = await control.call<{ image: Rect }>("debugViewer");
  return layout.image;
};

// MARK: Scrolling

/** A scroll over a window that isn't focused moves it, as on a Mac. */
async function scroll() {
  if (running("TextEdit") || running("Calculator")) {
    console.log("skip scroll: TextEdit or Calculator is the user's");
    return;
  }
  const text = path.join(out, "long.txt");
  writeFileSync(
    text,
    Array.from({ length: 400 }, (_, i) => `line ${i + 1} of a long document`).join("\n"),
  );
  await startDebugApp();
  const control = await Control.connect();
  try {
    const { display } = await viewerOn(control, "e2e-scroll");
    const id = display.displayId;
    await control.call("launchApp", { displayId: id, app: "TextEdit", open: text });
    await settle(1500);
    const calc = (
      await control.call<{ window: { x: number; y: number; width: number } }>("launchApp", {
        displayId: id,
        app: "Calculator",
      })
    ).window;
    await settle(1000);
    // Focus goes to Calculator, away from the page to scroll.
    const focusCalculator = () =>
      control.call("click", { displayId: id, x: calc.x + calc.width / 2, y: calc.y + 14 });
    await focusCalculator();
    const image = await showViewer(control, display);
    // A point on the page left of Calculator, and the page's text around it.
    const [px, py] = onScreen(image, display, Math.max(80, calc.x - 240), 420);
    const [tx, ty] = onScreen(image, display, 20, 120);
    const [bx, by] = onScreen(image, display, Math.max(80, calc.x - 40), 700);
    const page: Rect = [tx, ty, bx - tx, by - ty];

    const moved = async (label: string, gesture: () => void) => {
      // A scroll is the user's input: it takes the display over. Hand it
      // back so the agent can move focus away again.
      await release(control, id);
      await focusCalculator();
      await settle(1500); // the agent's pointer fades
      const before = digest(capture(`scroll-${label}-0`, page));
      gesture();
      await settle(700);
      const after = digest(capture(`scroll-${label}-1`, page));
      check(`${label} scrolls the window under the pointer without a click`, before !== after);
    };
    await moved("trackpad", () => hid("scroll", px, py, 15));
    await moved("wheel", () => hid("wheel", px, py, 6));
    await moved("recorded gesture", () => hid("replay", fixture("trackpad.log"), 0.4, 9.6, px, py));
  } finally {
    sh(
      "/usr/bin/osascript",
      ["-e", 'tell application "TextEdit" to close every document saving no'],
      true,
    );
    quitApp("TextEdit");
    quitApp("Calculator");
    control.close();
    await settle(800);
  }
}

// MARK: The menu bar strip

/** The strip says it can't be clicked, and a dead click doesn't take over. */
async function strip() {
  if (running("Calculator")) {
    console.log("skip strip: Calculator is the user's");
    return;
  }
  await startDebugApp();
  const control = await Control.connect();
  try {
    const { display } = await viewerOn(control, "e2e-strip");
    const id = display.displayId;
    const image = await showViewer(control, display);
    const [sx, sy] = onScreen(image, display, display.width / 2, 12);
    const region = [image[0], image[1], image[2], 60];
    await release(control, id);

    const hover = path.join(out, "strip-hover.png");
    hid("hover", sx, sy, hover, region.map(Math.round).join(","));
    const [r, g] = pixel(hover, image[2] / 4, 12 * (image[3] / display.height));
    check("an empty strip turns red under the pointer", r! > g! + 60, `rgb ${r} ${g}`);

    hid("click", sx, sy);
    await settle(300);
    check(
      "a click that opens no menu leaves the display to the agent",
      !(await takenOver(control, id)),
    );

    await control.call("launchApp", { displayId: id, app: "Calculator" });
    await settle(2500);
    const viewer = pidOf("target/Sidescreen Dev.app/Contents/MacOS/Sidescreen")!;
    // Calculator's View menu, the third title.
    const [vx, vy] = onScreen(image, display, 150, 12);
    hid("click", vx, vy);
    await settle(600);
    const menu = windowsOf(viewer).find((w) => w.layer >= 101);
    capture("strip-menu", [image[0], image[1], image[2], 400]);
    check("a title opens the agent app's menu", menu !== undefined);
    hid("key", 53); // Escape closes it
    await settle(300);
    check("opening a menu takes the display over", await takenOver(control, id));
    await release(control, id);
  } finally {
    quitApp("Calculator");
    control.close();
    await settle(800);
  }
}

// MARK: Focus

/** The user's front app keeps its key window, and so its input method, while an agent types. */
async function focus() {
  if (running("Calculator")) {
    console.log("skip focus: Calculator is the user's");
    return;
  }
  await startDebugApp();
  const control = await Control.connect();
  const holder = Bun.spawn([path.join(out, "keyholder"), "12"], { stdout: "pipe" });
  try {
    const display = await control.call<Display>("createDisplay", {
      sessionKey: "e2e-focus",
      client: "E2E",
      title: "focus",
    });
    const id = display.displayId;
    await control.call("launchApp", { displayId: id, app: "Calculator" });
    await settle(1500);
    // Launching may bring Calculator forward; the user's app is front again.
    sh(path.join(out, "probe"), ["activate", String(holder.pid)], true);
    await settle(800);
    const before = Date.now();
    for (const text of ["7+8=", "2*3="]) await control.call("type", { displayId: id, text });
    const listed = await control.call<{ windows: { windowId: number }[] }>("listWindows", {
      displayId: id,
    });
    const shot = await control.call<{ tree: string }>("windowState", {
      displayId: id,
      windowId: listed.windows[0]!.windowId,
    });
    const typed = /value="\W*6"/.test(shot.tree);
    await settle(Math.max(0, 2000 - (Date.now() - before)));
    holder.kill();
    const log = await new Response(holder.stdout).text();
    const lines = log.split("\n").filter(Boolean);
    check(
      "the agent's typing reaches its app",
      typed,
      shot.tree.match(/statictext value="[^"]*"/g)?.join(" "),
    );
    check(
      "the user's window stays key while the agent types",
      lines[0] === "key" && !lines.includes("resign"),
      lines.join(" "),
    );
  } finally {
    holder.kill();
    quitApp("Calculator");
    control.close();
    await settle(800);
  }
}

// MARK: Permissions

const near = (a: number, b: number) => Math.abs(a - b) <= 3;

/** Without grants: the setup window, the agent's error, the Settings helper. */
async function permissions() {
  if (running("System Settings")) {
    console.log("skip permissions: System Settings is the user's");
    return;
  }
  stopDebugApp();
  const app = await startUngrantedApp();
  try {
    const pid = pidOf(`${app}/Contents/MacOS/Sidescreen`)!;
    let setup: ReturnType<typeof windowsOf>[number] | undefined;
    for (let i = 0; i < 50 && !setup; i++) {
      await settle(100);
      setup = windowsOf(pid).find((w) => w.layer === 0 && Math.abs(w.width - 480) < 2);
    }
    check("the setup window opens at launch", setup !== undefined);
    if (setup) capture("permissions-setup", [setup.x, setup.y, setup.width, setup.height]);

    const control = await Control.connect();
    const refused = await control.send("createDisplay", {
      sessionKey: "e2e-perm",
      client: "E2E",
      title: "perm",
    });
    check(
      "an agent is told what's missing and who grants it",
      refused.error?.code === "permission" && /Screen Recording/.test(refused.error.message),
      refused.error?.message ?? "accepted",
    );

    await control.call("debugShow", { surface: "assist-ax" });
    let settings: ReturnType<typeof windowsOf>[number] | undefined;
    for (let i = 0; i < 40 && !settings; i++) {
      await settle(150);
      const spid = pidOf("System Settings.app/Contents/MacOS/System Settings");
      settings = spid
        ? windowsOf(spid).find((w) => w.layer === 0 && w.width > 520 && w.height > 360)
        : undefined;
    }
    // The helper follows System Settings on a 0.4 s tick once it's in front.
    let helper: ReturnType<typeof windowsOf>[number] | undefined;
    for (let i = 0; i < 30 && !helper; i++) {
      await settle(100);
      helper = windowsOf(pid).find((w) => w.layer === 3 && Math.abs(w.width - 392) < 2);
    }
    await settle(1500); // the pane's list loads
    // Right edges aligned, 18 pt in; just below the window, or inside its
    // bottom-right corner when the screen has no room below.
    const placed =
      settings !== undefined &&
      helper !== undefined &&
      near(helper.x + helper.width, settings.x + settings.width - 18) &&
      (near(helper.y, settings.y + settings.height + 10) ||
        near(helper.y + helper.height, settings.y + settings.height - 18));
    check(
      "the drag helper sits at System Settings' bottom-right",
      placed,
      JSON.stringify({ settings, helper }),
    );
    if (settings) {
      capture("permissions-helper", [
        settings.x,
        settings.y,
        settings.width,
        settings.height + 120,
      ]);
    }

    quitApp("System Settings");
    await settle(1200);
    check(
      "the helper goes when System Settings quits",
      !windowsOf(pid).some((w) => w.layer === 3 && Math.abs(w.width - 392) < 2),
    );
    control.close();
  } finally {
    quitApp("System Settings");
    stopApp(app);
    sh("/usr/bin/tccutil", ["reset", "All", "dev.sidescreen.e2e-ungranted"], true);
  }
}

// MARK: Run

const cases: Record<string, () => Promise<void>> = { scroll, strip, focus, permissions };
const chosen = process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(cases);
for (const name of chosen)
  if (!cases[name]) throw new Error(`no case ${name}; cases: ${Object.keys(cases).join(", ")}`);

buildTools();
console.log("waiting for an idle user with no Agent Display open…");
if (!(await waitForQuiet())) {
  console.log("never quiet; nothing ran");
  process.exit(0);
}
const before = snapshot();
try {
  for (const name of chosen) {
    console.log(`— ${name}`);
    if (probe("locked") === "1" && !(await waitForQuiet())) break;
    const failedBefore = failed();
    try {
      await cases[name]!();
    } catch (error) {
      check(`${name} ran`, false, String(error));
    }
    if (probe("locked") === "1" && failed() > failedBefore) {
      console.log(`void: the screen locked during ${name}; rerun it`);
      voided(failed() - failedBefore);
    }
  }
} finally {
  stopDebugApp();
  reopenInstalled();
  await settle(1000);
  restore(before);
}
console.log(failed() ? `${failed()} failed` : "all passed");
process.exit(failed() ? 1 : 0);

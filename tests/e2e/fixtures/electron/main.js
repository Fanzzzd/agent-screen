// A window for the end-to-end tests: resizable, with draggable rows. The
// page writes what it saw into the title; if the renderer dies, so does
// this process's title, so a test can tell.
const { app, BrowserWindow, Menu, screen } = require("electron");

// As development builds that stay out of the user's way do: an accessory
// app whose window shows without activating it (issues #17, #20).
const background = process.env.HARNESS_BACKGROUND === "1";
// HARNESS_TONE ("440") plays a sine at that many Hz, for recordings' sound.
const tone = Number(process.env.HARNESS_TONE || 0);
if (tone) app.commandLine.appendSwitch("autoplay-policy", "no-user-gesture-required");
if (background) app.setActivationPolicy("accessory");
// HARNESS_SINGLE=1 allows one instance, as most packaged Electron apps do:
// a second one quits at once and the first hears of it (#29).
if (process.env.HARNESS_SINGLE === "1" && !app.requestSingleInstanceLock()) app.quit();

// HARNESS_MENU_LOG (a file) adds a Go menu whose Previous view item, with
// no shortcut, writes how its click came: "previous accelerator=false"
// for a click on the item, true for a shortcut (#39). File > Open Recent
// > notes.txt, one submenu down, writes "recent accelerator=…" the same
// way.
const menuLog = process.env.HARNESS_MENU_LOG;
if (menuLog) {
  const fs = require("fs");
  const note = (line) => fs.appendFileSync(menuLog, line + "\n");
  app.whenReady().then(() => {
    Menu.setApplicationMenu(
      Menu.buildFromTemplate([
        { role: "appMenu" },
        {
          label: "File",
          submenu: [
            { label: "New Window" },
            {
              label: "Open Recent",
              submenu: [
                {
                  label: "notes.txt",
                  click: (_item, _window, event) => note(`recent accelerator=${event.triggeredByAccelerator}`),
                },
                { label: "todo.txt" },
              ],
            },
          ],
        },
        { role: "editMenu" },
        {
          label: "Go",
          submenu: [
            {
              label: "Previous view",
              click: (_item, _window, event) => note(`previous accelerator=${event.triggeredByAccelerator}`),
            },
          ],
        },
      ]),
    );
  });
}

// HARNESS_IDLE=1 opens no window, as another tool's copy of the bundle
// that runs in the background does (#35).
const idle = process.env.HARNESS_IDLE === "1";
if (idle) app.whenReady().then(() => app.dock && app.dock.hide());

// HARNESS_PAIR=1 (#110): two windows of one app, as a browser the user
// and an agent both use. The agent's opens on HARNESS_DISPLAY without
// activating the app; the user's opens on the main screen. When the app
// first comes to the front, the user's window is key, as the user working
// in it; macOS lets an app started in the background ask for the front
// but not take it, so the test brings it forward by the pid the log
// names. HARNESS_KEY_LOG (a file) gets a line per event: each window's
// focus and blur, as AppKit's key window changes, and what each page saw
// (pair.html).
const pair = process.env.HARNESS_PAIR === "1";
if (pair) {
  const fs = require("fs");
  const keyLog = process.env.HARNESS_KEY_LOG;
  const note = (line) => keyLog && fs.appendFileSync(keyLog, `${Date.now()} ${line}\n`);
  app.whenReady().then(() => {
    const on = screen.getAllDisplays().find((d) => d.id === Number(process.env.HARNESS_DISPLAY));
    const area = screen.getPrimaryDisplay().workArea;
    const open = (side, bounds, fixed) => {
      // The user's window has no zoom or full-screen button, which makes
      // AeroSpace float it rather than tile your windows around it.
      const frame = fixed
        ? { resizable: false, maximizable: false, minimizable: false, fullscreenable: false }
        : {};
      const win = new BrowserWindow({ ...bounds, ...frame, title: `pair ${side}`, show: false });
      win.on("focus", () => note(`win ${side} focus`));
      win.on("blur", () => note(`win ${side} blur`));
      // Electron 35 and later pass one event with the message; earlier
      // ones pass it third.
      win.webContents.on("console-message", (...args) => {
        const message = typeof args[2] === "string" ? args[2] : args[0] && args[0].message;
        if (typeof message === "string" && message.startsWith("pair ")) note(message);
      });
      // A page's context menu, as a browser's: a native menu that takes
      // the keyboard while it is open.
      win.webContents.on("context-menu", () => {
        note(`pair ${side} context-menu`);
        Menu.buildFromTemplate([{ label: "Back" }, { label: "Reload" }]).popup({ window: win });
      });
      win.loadFile("pair.html", { query: { side } });
      return win;
    };
    const user = open("user", {
      x: area.x + area.width - 460,
      y: area.y + area.height - 300,
      width: 440,
      height: 280,
    }, true);
    const agent = open("agent", {
      ...(on ? { x: on.bounds.x + 40, y: on.bounds.y + 40 } : {}),
      width: 640,
      height: 480,
    });
    // Once: posted input and focus records make AppKit call a background
    // app active too, and the agent's window must keep key status then.
    app.on("did-become-active", () => note("app active"));
    app.once("did-become-active", () => user.focus());
    app.on("did-resign-active", () => note("app inactive"));
    let shown = 0;
    const ready = () => {
      if (++shown < 2) return;
      setTimeout(() => note(`ready ${process.pid}`), 500);
    };
    agent.once("ready-to-show", () => {
      agent.showInactive();
      ready();
    });
    user.once("ready-to-show", () => {
      user.show();
      ready();
    });
  });
}

app.whenReady().then(() => {
  if (idle || pair) return;
  // HARNESS_SIZE ("1700x1100") makes a window larger than the display (#19).
  const [width, height] = (process.env.HARNESS_SIZE || "640x480").split("x").map(Number);
  // HARNESS_DISPLAY (a CG display id) opens the window on that display, as
  // an app whose saved bounds are there does (#17).
  const on = screen.getAllDisplays().find((d) => d.id === Number(process.env.HARNESS_DISPLAY));
  const at = on ? { x: on.bounds.x + 40, y: on.bounds.y + 40 } : {};
  const win = new BrowserWindow({ width, height, ...at, title: "harness", show: false });
  win.once("ready-to-show", () => (background ? win.showInactive() : win.show()));
  win.webContents.on("render-process-gone", (_event, details) => {
    win.setTitle(`renderer gone: ${details.reason} ${details.exitCode}`);
  });
  // HARNESS_LATER, 1.5 s after the window shows: "1700x1100" makes it that
  // size, past the display; "main" puts it on the main screen at the
  // height of its work area, as an app restoring saved bounds does (#19).
  const later = process.env.HARNESS_LATER;
  if (later) {
    win.once("show", () =>
      setTimeout(() => {
        if (later === "main") {
          const area = screen.getPrimaryDisplay().workArea;
          win.setBounds({ x: area.x + 400, y: area.y, width: 480, height: area.height });
        } else {
          const [w, h] = later.split("x").map(Number);
          win.setSize(w, h);
        }
      }, 1500),
    );
  }
  // HARNESS_SUBTREE exercises a long earlier AX branch and a later pane (#134).
  win.loadFile(process.env.HARNESS_SUBTREE === "1" ? "subtree.html" : "index.html");
  if (tone) {
    win.webContents.once("did-finish-load", () => {
      win.webContents.executeJavaScript(
        `const c = new AudioContext(); const o = c.createOscillator(); o.frequency.value = ${tone}; ` +
          `const g = c.createGain(); g.gain.value = 0.3; o.connect(g).connect(c.destination); o.start();`,
      );
    });
  }
});
app.on("window-all-closed", () => app.quit());

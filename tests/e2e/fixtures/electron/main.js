// A window for the end-to-end tests: resizable, with draggable rows. The
// page writes what it saw into the title; if the renderer dies, so does
// this process's title, so a test can tell.
const { app, BrowserWindow, screen } = require("electron");

// As development builds that stay out of the user's way do: an accessory
// app whose window shows without activating it (issues #17, #20).
const background = process.env.HARNESS_BACKGROUND === "1";
// HARNESS_TONE ("440") plays a sine at that many Hz, for recordings' sound.
const tone = Number(process.env.HARNESS_TONE || 0);
if (tone) app.commandLine.appendSwitch("autoplay-policy", "no-user-gesture-required");
if (background) app.setActivationPolicy("accessory");

app.whenReady().then(() => {
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
  win.loadFile("index.html");
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

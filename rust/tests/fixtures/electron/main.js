// A window for the end-to-end tests: resizable, with draggable rows. The
// page writes what it saw into the title; if the renderer dies, so does
// this process's title, so a test can tell.
const { app, BrowserWindow } = require("electron");

// As development builds that stay out of the user's way do: an accessory
// app whose window shows without activating it (issues #17, #20).
const background = process.env.HARNESS_BACKGROUND === "1";
if (background) app.setActivationPolicy("accessory");

app.whenReady().then(() => {
  // HARNESS_SIZE ("1700x1100") makes a window larger than the display (#19).
  const [width, height] = (process.env.HARNESS_SIZE || "640x480").split("x").map(Number);
  const win = new BrowserWindow({ width, height, title: "harness", show: false });
  win.once("ready-to-show", () => (background ? win.showInactive() : win.show()));
  win.webContents.on("render-process-gone", (_event, details) => {
    win.setTitle(`renderer gone: ${details.reason} ${details.exitCode}`);
  });
  win.loadFile("index.html");
});
app.on("window-all-closed", () => app.quit());

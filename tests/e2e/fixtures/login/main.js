// A login window as aTrust shows one, Electron like it: a user field,
// which has the keyboard, and a password field. The page writes which
// field has focus and what each holds into the title.
const { app, BrowserWindow } = require("electron");

// LOGIN_ACTIVATE=1: it comes to the front as it shows, as aTrust does.
// Otherwise it never becomes the active app: the user may be typing.
const activate = process.env.LOGIN_ACTIVATE === "1";
if (!activate) app.setActivationPolicy("accessory");

app.whenReady().then(() => {
  const win = new BrowserWindow({ width: 360, height: 200, useContentSize: true, title: "login", show: false });
  win.once("ready-to-show", () => (activate ? win.show() : win.showInactive()));
  win.loadFile("index.html");
});
app.on("window-all-closed", () => app.quit());

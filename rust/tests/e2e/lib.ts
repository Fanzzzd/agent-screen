/**
 * Helpers for the end-to-end tests: the debug app, the control socket, real
 * pointer input and screen captures.
 *
 * The tests drive the real cursor, so they wait for the user to leave the
 * keyboard and pointer alone, and put everything back: the cursor, the front
 * app, the AeroSpace workspace and the installed Sidescreen.
 */
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, openSync, readFileSync, statSync } from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";

export const root = new URL("../../", import.meta.url).pathname; // rust/
export const out = path.join(root, "target/e2e");
/** SIDESCREEN_INSTANCE=e2e talks to a test app beside the user's, on its own socket. */
export const instance = process.env.SIDESCREEN_INSTANCE || undefined;
export const socketPath = path.join(
  os.homedir(),
  "Library/Application Support/Sidescreen",
  instance ? `control-${instance}.sock` : "control.sock",
);
const debugApp = path.join(root, "target/Sidescreen Dev.app");
export const installedApp = "/Applications/Sidescreen.app";

/** Run a command and return its stdout; throw when it fails. */
export const sh = (command: string, args: string[] = [], allowFail = false) => {
  const run = spawnSync(command, args, { encoding: "utf8" });
  if (run.status !== 0 && !allowFail) {
    throw new Error(`${command} ${args.join(" ")}: ${run.stderr || run.stdout}`);
  }
  // stdout is null when the command can't start.
  return ((run.stdout as string | null) ?? "").trim();
};

const osa = (script: string) => sh("/usr/bin/osascript", ["-e", script], true);

// MARK: Tools

/** Build the Swift helpers into target/e2e when their source is newer. */
export const buildTools = () => {
  mkdirSync(out, { recursive: true });
  for (const tool of ["hid", "probe", "keyholder"]) {
    const source = new URL(`tools/${tool}.swift`, import.meta.url).pathname;
    const binary = path.join(out, tool);
    if (!existsSync(binary) || statSync(binary).mtimeMs < statSync(source).mtimeMs) {
      sh("/usr/bin/swiftc", ["-O", source, "-o", binary]);
    }
  }
};

export const probe = (...args: string[]) => sh(path.join(out, "probe"), args);
export const hid = (...args: (string | number)[]) => sh(path.join(out, "hid"), args.map(String));
export const fixture = (name: string) => new URL(`fixtures/${name}`, import.meta.url).pathname;

/** A region of the screen: x, y, width and height in global points. */
export type Rect = [number, number, number, number];

export type Frame = {
  id: number;
  layer: number;
  x: number;
  y: number;
  width: number;
  height: number;
};

export const windowsOf = (pid: number): Frame[] =>
  probe("windows", String(pid))
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));

export const pidOf = (pattern: string) => {
  const pid = sh("/usr/bin/pgrep", ["-f", pattern], true).split("\n")[0];
  return pid ? Number(pid) : undefined;
};

export const running = (name: string) => sh("/usr/bin/pgrep", ["-x", name], true) !== "";

// MARK: Results

let failures = 0;
export const check = (label: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}${detail ? `: ${detail}` : ""}`);
  if (!ok) failures++;
};
export const failed = () => failures;
/** Take back failures from a case whose run doesn't count. */
export const voided = (count: number) => {
  failures -= count;
};

/** A capture of a screen region in global points, written under target/e2e. */
export const capture = (name: string, [x, y, w, h]: Rect) => {
  const file = path.join(out, `${name}.png`);
  sh("/usr/sbin/screencapture", ["-x", "-R", [x, y, w, h].map(Math.round).join(","), file]);
  return file;
};

export const digest = (file: string) => createHash("md5").update(readFileSync(file)).digest("hex");

/** The color at a point of a capture taken at 2x, given in capture points. */
export const pixel = (file: string, x: number, y: number) =>
  probe("pixel", file, String(x * 2), String(y * 2))
    .split(" ")
    .map(Number);

// MARK: Waiting for the user

/**
 * Wait until the user has been idle for 45 s, has no Agent Display open
 * (restarting the app ends every display) and the screen is unlocked: a
 * locked screen shows no windows and can't be captured. E2E_NOW=1 skips the wait.
 */
export const waitForQuiet = async (limit = 3600) => {
  if (process.env.E2E_NOW === "1") return true;
  const start = Date.now();
  while (Date.now() - start < limit * 1000) {
    if (Number(probe("idle")) >= 45 && probe("displays") === "0" && probe("locked") === "0")
      return true;
    await Bun.sleep(5000);
  }
  return false;
};

/** What the tests must leave as they found it. */
export const snapshot = () => ({
  front: osa(
    'tell application "System Events" to get name of first process whose frontmost is true',
  ),
  workspace: sh("aerospace", ["list-workspaces", "--focused"], true),
});

export const restore = (before: ReturnType<typeof snapshot>) => {
  const now = snapshot();
  if (before.front && now.front !== before.front)
    osa(`tell application "${before.front}" to activate`);
  if (before.workspace && now.workspace !== before.workspace)
    sh("aerospace", ["workspace", before.workspace], true);
  const after = snapshot();
  check(
    "front app and workspace unchanged",
    after.front === before.front && after.workspace === before.workspace,
    `${before.front}/${before.workspace} → ${after.front}/${after.workspace}`,
  );
};

// MARK: The app

const quit = (app: string) => {
  if (!pidOf(`${app}/Contents/MacOS/Sidescreen`)) return;
  // A normal quit hands each display's windows back; a kill leaves them to
  // macOS, which moves them onto the user's screen.
  osa(`tell application "${app}" to quit`);
  for (let i = 0; i < 40 && pidOf(`${app}/Contents/MacOS/Sidescreen`); i++) Bun.sleepSync(100);
};

const listening = () =>
  new Promise<boolean>((resolve) => {
    const socket = net.createConnection(socketPath);
    socket.once("connect", () => (socket.end(), resolve(true)));
    socket.once("error", () => resolve(false));
  });

/** Build the debug bundle and run it in place of the installed app. */
export const startDebugApp = async (env: Record<string, string> = {}) => {
  sh(path.join(root, "sidescreen/scripts/bundle.sh"), ["debug"]);
  quit(installedApp);
  quit(debugApp);
  // Started from here it borrows this terminal's privacy grants.
  // E2E_LOG=FILE keeps the app's log.
  const log = process.env.E2E_LOG ? openSync(process.env.E2E_LOG, "w") : "ignore";
  spawn(path.join(debugApp, "Contents/MacOS/Sidescreen"), [], {
    env: { ...process.env, ...env },
    stdio: ["ignore", log, log],
    detached: true,
  }).unref();
  for (let i = 0; i < 60 && !(await listening()); i++) await Bun.sleep(250);
};

export const stopDebugApp = () => quit(debugApp);

/**
 * The debug app as a second instance beside the user's Sidescreen, which
 * keeps running with its displays. Needs SIDESCREEN_INSTANCE. Agent-side
 * tests run this way at any time: their input goes to app processes, not
 * to the user's pointer and keyboard.
 */
export const startTestApp = async (env: Record<string, string> = {}) => {
  if (!instance) throw new Error("set SIDESCREEN_INSTANCE to run beside the user's app");
  sh(path.join(root, "sidescreen/scripts/bundle.sh"), ["debug"]);
  if (await listening()) return;
  const log = process.env.E2E_LOG ? openSync(process.env.E2E_LOG, "w") : "ignore";
  spawn(path.join(debugApp, "Contents/MacOS/Sidescreen"), [], {
    env: { ...process.env, ...env },
    stdio: ["ignore", log, log],
    detached: true,
  }).unref();
  for (let i = 0; i < 60 && !(await listening()); i++) await Bun.sleep(250);
  if (!(await listening())) throw new Error("the test app didn't start");
};

/** Quit the test instance through its socket; AppleScript would find the user's app by bundle id. */
export const stopTestApp = async () => {
  if (!(await listening())) return;
  const control = await Control.connect();
  await control.call("debugQuit").catch(() => {
    // The app can quit before it answers; the loop below waits either way.
  });
  control.close();
  for (let i = 0; i < 50 && (await listening()); i++) await Bun.sleep(100);
};

/**
 * Wait until the user has been idle for 45 s with the screen unlocked.
 * Tests beside the user's app that open windows need it: macOS shows a new
 * window on the user's screen for a moment before Sidescreen moves it.
 */
export const waitForIdle = async (limit = 3600) => {
  if (process.env.E2E_NOW === "1") return probe("locked") === "0";
  const start = Date.now();
  while (Date.now() - start < limit * 1000) {
    if (Number(probe("idle")) >= 45 && probe("locked") === "0") return true;
    await Bun.sleep(5000);
  }
  return false;
};

/** Wait until the screen is unlocked: a locked screen blocks input and capture. */
export const waitForUnlocked = async (limit = 3600) => {
  const start = Date.now();
  while (Date.now() - start < limit * 1000) {
    if (probe("locked") === "0") return true;
    await Bun.sleep(5000);
  }
  return false;
};

/** Start a copy under another bundle id, so it has no privacy grants. */
export const startUngrantedApp = async () => {
  sh(path.join(root, "sidescreen/scripts/bundle.sh"), ["debug"]);
  const copy = path.join(out, "Ungranted.app");
  sh("/bin/rm", ["-rf", copy]);
  sh("/usr/bin/ditto", [debugApp, copy]);
  sh("/usr/libexec/PlistBuddy", [
    "-c",
    "Set :CFBundleIdentifier dev.sidescreen.e2e-ungranted",
    `${copy}/Contents/Info.plist`,
  ]);
  const identity = sh("/bin/sh", [
    "-c",
    "security find-identity -v -p codesigning | awk -F'\"' '/Apple Development/ { print $2; exit }'",
  ]);
  sh("/usr/bin/codesign", ["--force", "--sign", identity || "-", copy]);
  // Each run starts with no grants and no entries in the lists.
  sh("/usr/bin/tccutil", ["reset", "All", "dev.sidescreen.e2e-ungranted"], true);
  quit(installedApp);
  quit(debugApp);
  // Opened by launchd, it answers for itself instead of borrowing ours.
  sh("/usr/bin/open", [copy]);
  for (let i = 0; i < 60 && !(await listening()); i++) await Bun.sleep(250);
  return copy;
};

export const stopApp = (app: string) => quit(app);

export const reopenInstalled = () => {
  if (existsSync(installedApp)) sh("/usr/bin/open", [installedApp]);
};

// MARK: Control socket

/** A control socket reply; the caller names the result's type. */
export type ControlReply = {
  id: number;
  result?: unknown;
  error?: { code: string; message: string };
};

export class Control {
  private buffer = "";
  private nextId = 1;
  private readonly pending = new Map<number, (message: ControlReply) => void>();

  private constructor(private readonly socket: net.Socket) {
    socket.setEncoding("utf8");
    socket.on("data", (chunk: string) => {
      this.buffer += chunk;
      let newline: number;
      while ((newline = this.buffer.indexOf("\n")) >= 0) {
        const message: ControlReply = JSON.parse(this.buffer.slice(0, newline));
        this.buffer = this.buffer.slice(newline + 1);
        this.pending.get(message.id)?.(message);
        this.pending.delete(message.id);
      }
    });
  }

  /** A connection holds its displays: they end when it closes. */
  static connect() {
    return new Promise<Control>((resolve, reject) => {
      const socket = net.createConnection(socketPath);
      socket.once("connect", () => resolve(new Control(socket)));
      socket.once("error", reject);
    });
  }

  /** The reply, error included, without throwing. */
  send(method: string, params: Record<string, unknown> = {}) {
    const id = this.nextId++;
    return new Promise<ControlReply>((resolve) => {
      this.pending.set(id, resolve);
      this.socket.write(`${JSON.stringify({ id, method, params })}\n`);
    });
  }

  async call<T = unknown>(method: string, params: Record<string, unknown> = {}) {
    const reply = await this.send(method, params);
    if (reply.error) throw new Error(`${method}: ${reply.error.code}: ${reply.error.message}`);
    // The app's JSON, trusted to match the method.
    return reply.result as T;
  }

  close() {
    this.socket.end();
  }
}

/** Where a display point is on screen, given the Viewer's image rect. */
export const onScreen = (
  image: Rect,
  display: { width: number; height: number },
  x: number,
  y: number,
): [number, number] => [
  image[0] + (x * image[2]) / display.width,
  image[1] + (y * image[3]) / display.height,
];

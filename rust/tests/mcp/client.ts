/** Minimal MCP client over stdio for the test scripts. */
import { fileURLToPath } from "node:url";
import { spawn, spawnSync, type ChildProcessByStdio } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import type { Readable, Writable } from "node:stream";

/** SIDESCREEN_INSTANCE=e2e talks to a test app beside the user's, on its own socket. */
export const instance = process.env.SIDESCREEN_INSTANCE || undefined;
export const socketPath = path.join(
  os.homedir(),
  "Library/Application Support/Sidescreen",
  instance ? `control-${instance}.sock` : "control.sock",
);

/** The server under test: the app binary's `mcp` subcommand from rust/target. */
const defaultServer = [
  fileURLToPath(
    new URL("../../target/Sidescreen Dev.app/Contents/MacOS/Sidescreen", import.meta.url),
  ),
  "mcp",
];

/** A window as launch_app and list_windows report it. */
export type Window = {
  windowId: number;
  app: string;
  title: string;
  kind: string;
  x: number;
  y: number;
  width: number;
  height: number;
};

/** The structuredContent fields the tests read; each tool fills some of them. */
export type Structured = {
  display_id?: number;
  window?: Window | null;
  windows?: Window[];
  display?: { width: number; height: number };
  width?: number;
  scale?: number;
  control?: string;
  note?: string;
  withdrawn?: boolean;
  reason?: string;
};

/** A tools/call result. */
export type ToolResult = {
  content: { type: string; text?: string; data?: string; mimeType?: string }[];
  structuredContent?: Structured;
  isError?: boolean;
};

/** A JSON-RPC reply; the caller names the result's type. */
export type Reply<T = ToolResult> = { id: number; result?: T; error?: unknown };

/** The text of a result's first content item. */
export const firstText = (result: ToolResult) => result.content[0]?.text ?? "";

/** Wait up to 10 s for a freshly started app to listen on its socket. */
const waitForApp = async () => {
  for (let attempt = 0; attempt < 50; attempt++) {
    const up = await new Promise<boolean>((resolve) => {
      const socket = net.createConnection(socketPath);
      socket.once("connect", () => (socket.end(), resolve(true)));
      socket.once("error", () => resolve(false));
    });
    if (up) return;
    await Bun.sleep(200);
  }
  throw new Error("the Sidescreen app is not listening on its control socket");
};

export class McpClient {
  private buffer = "";
  private nextId = 1;
  private readonly pending = new Map<number, (message: Reply<unknown>) => void>();
  private shots = 0;

  private constructor(private readonly server: ChildProcessByStdio<Writable, Readable, null>) {
    server.stdout.setEncoding("utf8");
    server.stdout.on("data", (chunk: string) => {
      this.buffer += chunk;
      let newline: number;
      while ((newline = this.buffer.indexOf("\n")) >= 0) {
        const message: Reply<unknown> = JSON.parse(this.buffer.slice(0, newline));
        this.buffer = this.buffer.slice(newline + 1);
        this.pending.get(message.id)?.(message);
        this.pending.delete(message.id);
      }
    });
  }

  /** Spawn the server and run the 2025-06-18 handshake. */
  static async start(env: Record<string, string>) {
    // A locked screen blocks input and lists no windows: every check would
    // fail for that reason alone.
    const probe = new URL("../../target/e2e/probe", import.meta.url).pathname;
    if (!existsSync(probe)) {
      spawnSync("/usr/bin/swiftc", [
        "-O",
        new URL("../e2e/tools/probe.swift", import.meta.url).pathname,
        "-o",
        probe,
      ]);
    }
    const locked = spawnSync(probe, ["locked"], { encoding: "utf8" });
    if (locked.stdout.trim() === "1") {
      console.log("skip: the screen is locked");
      process.exit(0);
    }
    await waitForApp();
    // SIDESCREEN_MCP="<binary> [args]" runs another server build, e.g.
    // `rust/target/debug/sidescreen-mcp`.
    const [command, ...args] = process.env.SIDESCREEN_MCP
      ? process.env.SIDESCREEN_MCP.split(" ")
      : defaultServer;
    const server = spawn(command!, args, {
      env: { ...process.env, CLAUDE_PROJECT_DIR: "/tmp/smoke", ...env },
      stdio: ["pipe", "pipe", "inherit"],
    });
    const client = new McpClient(server);
    const init = await client.request<{ protocolVersion: string }>("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "smoke", version: "0" },
    });
    console.log("protocol:", init.result?.protocolVersion);
    server.stdin.write(
      `${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`,
    );
    return client;
  }

  request<T = ToolResult>(method: string, params: unknown = {}) {
    return new Promise<Reply<T>>((resolve) => {
      const id = this.nextId++;
      // The server's JSON, trusted to match the method.
      this.pending.set(id, (message) => resolve(message as Reply<T>));
      this.server.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    });
  }

  /** Call a tool, print its text and save its images to /tmp. */
  async call(name: string, args: Record<string, unknown>) {
    const reply = await this.request("tools/call", { name, arguments: args });
    if (reply.error) throw new Error(`${name}: ${JSON.stringify(reply.error)}`);
    const result = reply.result!;
    for (const item of result.content) {
      if (item.type === "image") {
        const file = `/tmp/sidescreen-smoke-${++this.shots}.${item.mimeType === "image/png" ? "png" : "jpg"}`;
        writeFileSync(file, Buffer.from(item.data!, "base64"));
        console.log(`  ${name}: image → ${file}`);
      } else {
        console.log(`  ${name}${result.isError ? " (error)" : ""}: ${item.text}`);
      }
    }
    return result;
  }

  close() {
    this.server.stdin.end();
    this.server.kill();
  }
}

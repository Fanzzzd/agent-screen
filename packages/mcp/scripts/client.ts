/** Minimal MCP client over stdio for the test scripts. */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process"
import { writeFileSync } from "node:fs"
import * as net from "node:net"
import { socketPath } from "../src/Sidescreen.ts"

/** Wait up to 10 s for a freshly started app to listen on its socket. */
const waitForApp = async () => {
  for (let attempt = 0; attempt < 50; attempt++) {
    const up = await new Promise<boolean>((resolve) => {
      const socket = net.createConnection(socketPath)
      socket.once("connect", () => (socket.end(), resolve(true)))
      socket.once("error", () => resolve(false))
    })
    if (up) return
    await Bun.sleep(200)
  }
  throw new Error("the Sidescreen app is not listening on its control socket")
}

export class McpClient {
  private buffer = ""
  private nextId = 1
  private readonly pending = new Map<number, (message: any) => void>()
  private shots = 0

  private constructor(private readonly server: ChildProcessWithoutNullStreams) {
    server.stdout.setEncoding("utf8")
    server.stdout.on("data", (chunk: string) => {
      this.buffer += chunk
      let newline: number
      while ((newline = this.buffer.indexOf("\n")) >= 0) {
        const message = JSON.parse(this.buffer.slice(0, newline))
        this.buffer = this.buffer.slice(newline + 1)
        this.pending.get(message.id)?.(message)
        this.pending.delete(message.id)
      }
    })
  }

  /** Spawn the server and run the 2025-06-18 handshake. */
  static async start(env: Record<string, string>) {
    await waitForApp()
    // SIDESCREEN_MCP="<binary> [args]" runs another server build, e.g. the Rust
    // app as `Sidescreen.app/Contents/MacOS/Sidescreen mcp`.
    const [command, ...args] = process.env.SIDESCREEN_MCP ? process.env.SIDESCREEN_MCP.split(" ") : ["bun", "src/main.ts"]
    const server = spawn(command, args, {
      cwd: new URL("..", import.meta.url).pathname,
      env: { ...process.env, CLAUDE_PROJECT_DIR: "/tmp/smoke", ...env },
      stdio: ["pipe", "pipe", "inherit"]
    })
    const client = new McpClient(server)
    const init = await client.request("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "smoke", version: "0" }
    })
    console.log("protocol:", init.result?.protocolVersion)
    server.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n")
    return client
  }

  request(method: string, params: unknown = {}) {
    return new Promise<any>((resolve) => {
      const id = this.nextId++
      this.pending.set(id, resolve)
      this.server.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n")
    })
  }

  /** Call a tool, print its text and save its images to /tmp. */
  async call(name: string, args: Record<string, unknown>) {
    const reply = await this.request("tools/call", { name, arguments: args })
    if (reply.error) throw new Error(`${name}: ${JSON.stringify(reply.error)}`)
    const result = reply.result
    for (const item of result.content) {
      if (item.type === "image") {
        const file = `/tmp/sidescreen-smoke-${++this.shots}.${item.mimeType === "image/png" ? "png" : "jpg"}`
        writeFileSync(file, Buffer.from(item.data, "base64"))
        console.log(`  ${name}: image → ${file}`)
      } else {
        console.log(`  ${name}${result.isError ? " (error)" : ""}: ${item.text}`)
      }
    }
    return result
  }

  close() {
    this.server.stdin.end()
    this.server.kill()
  }
}

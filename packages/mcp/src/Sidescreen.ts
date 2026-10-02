import * as net from "node:net"
import * as os from "node:os"
import * as path from "node:path"
import { Context, Data, Effect, Layer } from "effect"

/** A request the Sidescreen app refused or could not serve. */
export class ControlError extends Data.TaggedError("ControlError")<{
  /** `invalid`, `not_found`, `ended`, `takeover`, `failed`, or `unavailable` when the app is not running. */
  readonly code: string
  readonly message: string
}> {}

export const socketPath = path.join(os.homedir(), "Library/Application Support/Sidescreen/control.sock")

/**
 * Client for the menu bar app's control socket.
 *
 * The connection opens on the first call and stays open for the life of this
 * process. The app treats its closing as the end of the Agent Session, so the
 * session's display stops taking input when the MCP server exits.
 */
export class Sidescreen extends Context.Service<Sidescreen, {
  readonly call: (
    method: string,
    params: Record<string, unknown>
  ) => Effect.Effect<Record<string, unknown>, ControlError>
}>()("sidescreen/Sidescreen") {}

type Reply =
  | { readonly id: number; readonly result: Record<string, unknown> }
  | { readonly id: number; readonly error: { readonly code: string; readonly message: string } }

class Connection {
  private socket: net.Socket | undefined
  private connecting: Promise<net.Socket> | undefined
  private buffer = ""
  private nextId = 1
  private readonly pending = new Map<number, (reply: Reply) => void>()

  request(method: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
    return this.open().then((socket) =>
      new Promise((resolve, reject) => {
        const id = this.nextId++
        this.pending.set(id, (reply) =>
          "error" in reply ? reject(new ControlError(reply.error)) : resolve(reply.result))
        socket.write(JSON.stringify({ id, method, params }) + "\n")
      })
    )
  }

  close() {
    this.socket?.end()
  }

  private open(): Promise<net.Socket> {
    if (this.socket && !this.socket.destroyed) return Promise.resolve(this.socket)
    this.connecting ??= new Promise<net.Socket>((resolve, reject) => {
      const socket = net.createConnection(socketPath)
      socket.setEncoding("utf8")
      socket.once("connect", () => {
        this.socket = socket
        this.connecting = undefined
        resolve(socket)
      })
      socket.once("error", (error) => {
        this.connecting = undefined
        reject(new ControlError({
          code: "unavailable",
          message: `The Sidescreen app is not running (${error.message}). Ask the user to start it.`
        }))
      })
      socket.on("data", (chunk: string) => this.receive(chunk))
      socket.on("close", () => this.failPending())
    })
    return this.connecting
  }

  private receive(chunk: string) {
    this.buffer += chunk
    let newline: number
    while ((newline = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, newline)
      this.buffer = this.buffer.slice(newline + 1)
      if (line.length === 0) continue
      const reply = JSON.parse(line) as Reply
      this.pending.get(reply.id)?.(reply)
      this.pending.delete(reply.id)
    }
  }

  private failPending() {
    this.socket = undefined
    for (const settle of this.pending.values()) {
      settle({ id: 0, error: { code: "unavailable", message: "The Sidescreen app closed the connection." } })
    }
    this.pending.clear()
  }
}

export const layer = Layer.effect(
  Sidescreen,
  Effect.acquireRelease(
    Effect.sync(() => new Connection()),
    (connection) => Effect.sync(() => connection.close())
  ).pipe(
    Effect.map((connection) =>
      Sidescreen.of({
        call: (method, params) =>
          Effect.tryPromise({
            try: () => connection.request(method, params),
            catch: (error) =>
              error instanceof ControlError
                ? error
                : new ControlError({ code: "failed", message: String(error) })
          })
      })
    )
  )
)

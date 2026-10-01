import { Context, Effect, Layer, Schema } from "effect"
import { McpSchema, McpServer } from "effect/ai"
import { identify } from "./Session.ts"
import { ControlError, Sidescreen } from "./Sidescreen.ts"

/**
 * Tools are registered through `McpServer.addTool` rather than a Toolkit:
 * Toolkit handlers can only return JSON, and `computer` has to return images.
 */

type Content = McpSchema.CallToolResult["content"][number]

/** Parameter schemas decode plain JSON and need no services. */
type Parameters = Schema.Constraint & { readonly DecodingServices: never }

interface Spec<S extends Parameters> {
  readonly name: string
  readonly title: string
  readonly description: string
  readonly parameters: S
  readonly readOnly?: boolean
  readonly destructive?: boolean
  readonly run: (
    params: S["Type"],
    context: McpSchema.McpRequestContext["Service"]
  ) => Effect.Effect<ReadonlyArray<Content> | Record<string, unknown>, ControlError, Sidescreen>
}

const define = <S extends Parameters>(spec: Spec<S>) => spec

const DisplayId = Schema.Int.annotate({
  description: "The display handle returned by create_display."
})

const Coordinate = Schema.Tuple([Schema.Finite, Schema.Finite]).annotate({
  description: "[x, y] in screenshot pixels, from the display's top-left corner."
})

const text = (value: string): Content => ({ type: "text", text: value })

// MARK: Displays

const createDisplay = define({
  name: "create_display",
  title: "Create Agent Display",
  description: [
    "Get a private virtual display for this session to operate GUI apps on, without touching the user's screen,",
    "cursor or keyboard. Call it only when you need a screen. Calling it again returns the same display.",
    "The returned display_id stays valid until end_display or until this session ends.",
    "Put windows on it with launch_app or place_window, then drive it with the computer tool."
  ].join(" "),
  parameters: Schema.Struct({
    title: Schema.optional(Schema.String.annotate({
      description: "Short task title shown to the user, e.g. \"Book flights\"."
    })),
    width: Schema.optional(Schema.Int.annotate({ description: "Width in points. Default 1440." })),
    height: Schema.optional(Schema.Int.annotate({ description: "Height in points. Default 900." }))
  }),
  run: Effect.fnUntraced(function*(params, context) {
    const session = identify(context)
    const sidescreen = yield* Sidescreen
    const reply = yield* sidescreen.call("createDisplay", {
      sessionKey: session.key,
      client: session.client,
      title: params.title ?? session.folder,
      width: params.width,
      height: params.height
    })
    return {
      display_id: reply.displayId,
      width: reply.width,
      height: reply.height,
      reused: reply.reused
    }
  })
})

const endDisplay = define({
  name: "end_display",
  title: "End Agent Display",
  description:
    "Finish with the display. It stays visible to the user for review but accepts no more input. Its display_id stops working.",
  parameters: Schema.Struct({ display_id: DisplayId }),
  destructive: true,
  run: Effect.fnUntraced(function*({ display_id }) {
    yield* (yield* Sidescreen).call("endDisplay", { displayId: display_id })
    return { ended: true }
  })
})

// MARK: Control

const Timeout = Schema.Int.annotate({
  description: "Seconds to wait before giving up. Default 100, at most 600."
})

const describeWait = (outcome: unknown) => {
  switch (outcome) {
    case "returned":
      return { control: "agent", note: "You have control. Take a screenshot before you continue." }
    case "timedOut":
      return { control: "user", note: "The user is still operating the display. Wait again or do something else." }
    default:
      return { control: "none", note: "The display has ended." }
  }
}

const waitForControl = define({
  name: "wait_for_control",
  title: "Wait for the user to hand the display back",
  description: [
    "Block until the user stops operating your display (a Takeover) and hands it back, or until the timeout.",
    "Call it after any tool fails with a takeover error. Returns at once if you already have control."
  ].join(" "),
  parameters: Schema.Struct({ display_id: DisplayId, timeout_seconds: Schema.optional(Timeout) }),
  readOnly: true,
  run: Effect.fnUntraced(function*({ display_id, timeout_seconds }) {
    const reply = yield* (yield* Sidescreen).call("waitForControl", {
      displayId: display_id,
      timeoutSeconds: timeout_seconds
    })
    return describeWait(reply.outcome)
  })
})

const requestHelp = define({
  name: "request_help",
  title: "Ask the user to step in",
  description: [
    "Ask the user to take over your display for something you cannot do, such as a login, a captcha or a",
    "two-factor code. The user gets a system notification with your reason. The call then waits like",
    "wait_for_control until the user has stepped in and handed the display back, or until the timeout.",
    "On a timeout the request stays open; call wait_for_control to keep waiting."
  ].join(" "),
  parameters: Schema.Struct({
    display_id: DisplayId,
    reason: Schema.String.annotate({ description: "What you need the user to do, in one short sentence." }),
    timeout_seconds: Schema.optional(Timeout)
  }),
  run: Effect.fnUntraced(function*({ display_id, reason, timeout_seconds }) {
    const sidescreen = yield* Sidescreen
    yield* sidescreen.call("requestHelp", { displayId: display_id, reason })
    const reply = yield* sidescreen.call("waitForControl", { displayId: display_id, timeoutSeconds: timeout_seconds })
    return describeWait(reply.outcome)
  })
})

// MARK: Windows

const listWindows = define({
  name: "list_windows",
  title: "List windows on the display",
  description: "List app windows on the display, front to back, with ids and bounds in screenshot pixels.",
  parameters: Schema.Struct({ display_id: DisplayId }),
  readOnly: true,
  run: Effect.fnUntraced(function*({ display_id }) {
    return yield* (yield* Sidescreen).call("listWindows", { displayId: display_id })
  })
})

const placeWindow = define({
  name: "place_window",
  title: "Place a window on the display",
  description:
    "Move an existing app window onto the display. Get window ids from list_windows (pass another display's id) or from launch_app.",
  parameters: Schema.Struct({
    display_id: DisplayId,
    window_id: Schema.Int.annotate({ description: "The window to move." })
  }),
  run: Effect.fnUntraced(function*({ display_id, window_id }) {
    yield* (yield* Sidescreen).call("placeWindow", { displayId: display_id, windowId: window_id })
    return { placed: true }
  })
})

const launchApp = define({
  name: "launch_app",
  title: "Launch an app on the display",
  description:
    "Open an app in the background and put its new window on the display. If the app is already running, a new window is opened for you instead of taking the user's.",
  parameters: Schema.Struct({
    display_id: DisplayId,
    app: Schema.String.annotate({ description: "App name (\"Safari\") or bundle id (\"com.apple.Safari\")." })
  }),
  run: Effect.fnUntraced(function*({ display_id, app }) {
    const reply = yield* (yield* Sidescreen).call("launchApp", { displayId: display_id, app })
    return reply.window ? { window: reply.window } : { window: null, note: "The app opened but no new window appeared." }
  })
})

// MARK: Computer

const Action = Schema.Literals([
  "screenshot",
  "left_click",
  "right_click",
  "middle_click",
  "double_click",
  "triple_click",
  "mouse_move",
  "left_click_drag",
  "scroll",
  "type",
  "key",
  "wait"
])

const computer = define({
  name: "computer",
  title: "Operate the display",
  description: [
    "See and operate your Agent Display. Works like Anthropic's computer-use tool, scoped to one display:",
    "screenshot pixels and coordinates are the same space. Every action except wait returns a fresh screenshot.",
    "key takes xdotool-style chords such as \"Return\", \"cmd+l\" or \"cmd+shift+t\". Click actions accept held",
    "modifiers in text, e.g. \"shift\". Fails with a takeover error while the user is operating the display;",
    "then call wait_for_control."
  ].join(" "),
  parameters: Schema.Struct({
    display_id: DisplayId,
    action: Action,
    coordinate: Schema.optional(Coordinate),
    start_coordinate: Schema.optional(Coordinate),
    text: Schema.optional(Schema.String),
    scroll_direction: Schema.optional(Schema.Literals(["up", "down", "left", "right"])),
    scroll_amount: Schema.optional(Schema.Int.annotate({ description: "Scroll wheel clicks. Default 3." })),
    duration: Schema.optional(Schema.Finite.annotate({ description: "Seconds, for wait." }))
  }),
  run: Effect.fnUntraced(function*(params) {
    const sidescreen = yield* Sidescreen
    const displayId = params.display_id
    const at = () => requireCoordinate(params.coordinate, params.action)
    const modifiers = params.text?.split("+").filter((part) => part.length > 0) ?? []

    switch (params.action) {
      case "screenshot":
        break
      case "left_click":
      case "right_click":
      case "middle_click":
      case "double_click":
      case "triple_click": {
        const [x, y] = yield* at()
        const button = params.action === "right_click" ? "right" : params.action === "middle_click" ? "middle" : "left"
        const count = params.action === "double_click" ? 2 : params.action === "triple_click" ? 3 : 1
        yield* sidescreen.call("click", { displayId, x, y, button, count, modifiers })
        break
      }
      case "mouse_move": {
        const [x, y] = yield* at()
        yield* sidescreen.call("move", { displayId, x, y })
        break
      }
      case "left_click_drag": {
        const [fromX, fromY] = yield* requireCoordinate(params.start_coordinate, "left_click_drag start")
        const [toX, toY] = yield* at()
        yield* sidescreen.call("drag", { displayId, fromX, fromY, toX, toY })
        break
      }
      case "scroll": {
        const [x, y] = yield* at()
        const amount = params.scroll_amount ?? 3
        const direction = params.scroll_direction ?? "down"
        // Line-based wheel deltas: positive scrolls content up/left into view.
        const deltaY = direction === "down" ? -amount : direction === "up" ? amount : 0
        const deltaX = direction === "right" ? -amount : direction === "left" ? amount : 0
        yield* sidescreen.call("scroll", { displayId, x, y, deltaX, deltaY })
        break
      }
      case "type":
        yield* sidescreen.call("type", { displayId, text: yield* requireText(params.text, "type") })
        break
      case "key":
        yield* sidescreen.call("key", { displayId, keys: yield* requireText(params.text, "key") })
        break
      case "wait":
        yield* Effect.sleep(`${Math.min(params.duration ?? 1, 30)} seconds`)
        return [text(`Waited ${params.duration ?? 1} s.`)]
    }

    // Let the UI settle so the screenshot shows the result of the action.
    if (params.action !== "screenshot") yield* Effect.sleep("350 millis")
    const shot = yield* sidescreen.call("screenshot", { displayId })
    return [
      text(`${shot.width}×${shot.height} screenshot of display ${displayId}.`),
      {
        type: "image",
        data: Uint8Array.from(Buffer.from(String(shot.data), "base64")),
        mimeType: String(shot.mimeType)
      }
    ]
  })
})

const requireCoordinate = (value: readonly [number, number] | undefined, action: string) =>
  value === undefined
    ? Effect.fail(new ControlError({ code: "invalid", message: `coordinate is required for ${action}` }))
    : Effect.succeed(value)

const requireText = (value: string | undefined, action: string) =>
  value === undefined || value.length === 0
    ? Effect.fail(new ControlError({ code: "invalid", message: `text is required for ${action}` }))
    : Effect.succeed(value)

// MARK: Registration

const toResult = (output: ReadonlyArray<Content> | Record<string, unknown>) =>
  Array.isArray(output)
    ? new McpSchema.CallToolResult({ content: output as Array<Content> })
    : new McpSchema.CallToolResult({
      content: [text(JSON.stringify(output))],
      structuredContent: output as Schema.Json
    })

const failure = (message: string) => new McpSchema.CallToolResult({ isError: true, content: [text(message)] })

const register = <S extends Parameters>(spec: Spec<S>) =>
  Effect.gen(function*() {
    const server = yield* McpServer.McpServer
    const services = yield* Effect.context<Sidescreen>()
    // Pin the generic types so the handler's error channel stays concrete.
    const decode = Schema.decodeUnknownEffect(spec.parameters) as (
      input: unknown
    ) => Effect.Effect<S["Type"], Schema.SchemaError>
    const run: Spec<S>["run"] = spec.run
    yield* server.addTool({
      tool: new McpSchema.Tool({
        name: spec.name,
        title: spec.title,
        description: spec.description,
        inputSchema: Schema.toJsonSchemaDocument(spec.parameters).schema as McpSchema.Tool["inputSchema"],
        annotations: {
          title: spec.title,
          readOnlyHint: spec.readOnly ?? false,
          destructiveHint: spec.destructive ?? false,
          openWorldHint: false
        }
      }),
      annotations: Context.empty(),
      handle: (payload) =>
        Effect.gen(function*() {
          const context = yield* McpSchema.McpRequestContext
          const params = yield* decode(payload ?? {})
          return toResult(yield* run(params, context))
        }).pipe(
          // Domain failures go back to the model as tool errors it can act on.
          Effect.catchTag("ControlError", (error) => Effect.succeed(failure(`${error.code}: ${error.message}`))),
          Effect.catchTag("SchemaError", (error) => Effect.succeed(failure(`invalid arguments: ${error.message}`))),
          Effect.provideContext(services)
        )
    })
  })

export const layer = Layer.effectDiscard(Effect.all([
  register(createDisplay),
  register(endDisplay),
  register(computer),
  register(waitForControl),
  register(requestHelp),
  register(listWindows),
  register(placeWindow),
  register(launchApp)
]))

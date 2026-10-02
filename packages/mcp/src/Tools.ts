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
  readonly idempotent?: boolean
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
  description: [
    "Finish with the display. It accepts no more input and its display_id stops working. If windows remain,",
    "it waits for the user to review them; give each a set_disposition first. An empty display closes."
  ].join(" "),
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
  description: [
    "List app windows on the display, front to back, with ids and bounds in screenshot pixels. origin says how",
    "a window got here (launched, placed, handedOver by the user, found, unknown); disposition is your current",
    "set_disposition verdict or null."
  ].join(" "),
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

const setDisposition = define({
  name: "set_disposition",
  title: "Keep or close a window when the display goes",
  description: [
    "Say what should happen to one of your windows when the user dismisses this display: keep it for the user",
    "to review (a result page, a filled form) or close it (scratch windows). Set it as soon as you know and",
    "change it any time; your session may end without warning. The user has the final say. Windows the user",
    "handed over always go back to them, whatever you choose."
  ].join(" "),
  parameters: Schema.Struct({
    display_id: DisplayId,
    window_id: Schema.Int.annotate({ description: "A window on the display, from list_windows." }),
    disposition: Schema.Literals(["keep", "close"])
  }),
  idempotent: true,
  run: Effect.fnUntraced(function*({ display_id, window_id, disposition }) {
    const reply = yield* (yield* Sidescreen).call("setDisposition", {
      displayId: display_id,
      windowId: window_id,
      disposition
    })
    return reply.handedOver
      ? { disposition, note: "The user handed this window over; it returns to them regardless." }
      : { disposition }
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

// MARK: Per window

const WindowId = Schema.Int.annotate({ description: "A window on your display, from list_windows or launch_app." })

/** Window state as tool content: the tree as text, then the window image. */
const windowState = (displayId: number, windowId: number) =>
  Effect.gen(function*() {
    const state = yield* (yield* Sidescreen).call("windowState", { displayId, windowId })
    const header = [
      `Window ${windowId}: ${JSON.stringify(state.window)}`,
      `${state.elements} elements${state.truncated ? " (truncated)" : ""}. Coordinates are window points from its`,
      `top-left corner, the same space as the ${state.width}×${state.height} image.`
    ].join(" ")
    return [
      text(`${header}\n\n${state.tree}`),
      { type: "image", data: Uint8Array.from(Buffer.from(String(state.data), "base64")), mimeType: String(state.mimeType) }
    ] satisfies Array<Content>
  })

const getWindowState = define({
  name: "get_window_state",
  title: "See one window",
  description: [
    "Screenshot of one window on your display plus its accessibility tree. Every element has a number in",
    "brackets that act_on_element takes. Each new state of a window renumbers its elements, so always use the",
    "numbers from the latest get_window_state or act_on_element result."
  ].join(" "),
  parameters: Schema.Struct({ display_id: DisplayId, window_id: WindowId }),
  readOnly: true,
  run: ({ display_id, window_id }) => windowState(display_id, window_id)
})

const actOnElement = define({
  name: "act_on_element",
  title: "Act on a window element",
  description: [
    "Act on an element from get_window_state, then return the window's new state. press, show_menu, focus,",
    "set_value and scroll_into_view use accessibility actions, which reach the app without focusing it; click,",
    "double_click and right_click send a real click at the element's center. press falls back to a click when",
    "the element has no press action. Fails with a takeover error while the user operates the display."
  ].join(" "),
  parameters: Schema.Struct({
    display_id: DisplayId,
    window_id: WindowId,
    element: Schema.Int.annotate({ description: "The bracketed number from get_window_state." }),
    action: Schema.Literals(["press", "click", "double_click", "right_click", "show_menu", "focus", "set_value",
      "scroll_into_view"]),
    value: Schema.optional(Schema.String.annotate({ description: "Text for set_value." }))
  }),
  run: Effect.fnUntraced(function*({ display_id, window_id, element, action, value }) {
    yield* (yield* Sidescreen).call("elementAction", {
      displayId: display_id,
      windowId: window_id,
      element,
      action,
      value
    })
    yield* Effect.sleep("350 millis")
    return yield* windowState(display_id, window_id)
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
          idempotentHint: spec.idempotent ?? false,
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
  register(launchApp),
  register(setDisposition),
  register(getWindowState),
  register(actOnElement)
]))

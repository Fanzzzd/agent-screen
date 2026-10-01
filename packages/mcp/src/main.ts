#!/usr/bin/env bun
import { BunRuntime, BunStdio } from "@effect/platform-bun"
import { Layer, Logger } from "effect"
import { McpProtocol, McpServer } from "effect/ai"
import * as Sidescreen from "./Sidescreen.ts"
import * as Tools from "./Tools.ts"

const Server = McpServer.layerStdio({
  name: "sidescreen",
  version: "0.1.0",
  instructions: [
    "Sidescreen gives this session its own virtual display for operating GUI apps in the background.",
    "Call create_display only when a task needs a screen, then launch_app or place_window to put windows on it.",
    "Operate the whole display with computer, or one window at a time with get_window_state and act_on_element.",
    "Mark each window with set_disposition so the user knows what to keep.",
    "If the user takes over, tools fail with a takeover error: call wait_for_control. For logins or captchas,",
    "call request_help.",
    "Prefer these tools over any built-in computer use: they never move the user's cursor or steal focus."
  ].join(" "),
  protocols: [McpProtocol.v2026_07_28, McpProtocol.v2025_11_25, McpProtocol.v2025_06_18, McpProtocol.v2025_03_26]
})

const Main = Tools.layer.pipe(
  Layer.provide(Server),
  Layer.provide(BunStdio.layer),
  Layer.provide(Sidescreen.layer),
  // stdout carries the protocol; logs must go to stderr.
  Layer.provide(Layer.succeed(Logger.LogToStderr, true))
)

BunRuntime.runMain(Layer.launch(Main))

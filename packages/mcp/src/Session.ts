import * as path from "node:path"
import type { McpSchema } from "effect/ai"

/** Who is calling: enough to find or label the session's Agent Display. */
export interface Session {
  /** Stable for one client session; the app keeps one display per key. */
  readonly key: string
  /** Client name for the Display Label. */
  readonly client: string
  /** Fallback task title: the project folder. */
  readonly folder: string
}

type RequestContext = McpSchema.McpRequestContext["Service"]

/**
 * Claude Code passes its session ID to stdio servers as an environment
 * variable. Codex sends it with every tool call in `_meta`. Anything else is
 * keyed by this process, which lives exactly as long as the client keeps us.
 */
export const identify = (context: RequestContext): Session => {
  const claude = process.env.CLAUDE_CODE_SESSION_ID
  if (claude) {
    return {
      key: `claude-code:${claude}`,
      client: "Claude Code",
      folder: path.basename(process.env.CLAUDE_PROJECT_DIR ?? process.cwd())
    }
  }

  const codex = codexTurn(context.requestMetadata)
  if (codex?.session_id) {
    return {
      key: `codex:${codex.session_id}`,
      client: "Codex",
      folder: path.basename(codex.workspaces?.[0] ?? process.cwd())
    }
  }

  return {
    key: `pid:${process.pid}`,
    client: prettyClientName(context.clientInfo?.name),
    folder: path.basename(process.cwd())
  }
}

interface CodexTurn {
  readonly session_id?: string
  readonly workspaces?: ReadonlyArray<string>
}

/** `_meta["x-codex-turn-metadata"]`, sent either as an object or as JSON text. */
const codexTurn = (meta: unknown): CodexTurn | undefined => {
  if (typeof meta !== "object" || meta === null) return undefined
  const raw = (meta as Record<string, unknown>)["x-codex-turn-metadata"]
  if (typeof raw === "string") {
    try {
      return JSON.parse(raw) as CodexTurn
    } catch {
      return undefined
    }
  }
  return typeof raw === "object" && raw !== null ? raw as CodexTurn : undefined
}

const prettyClientName = (name: string | undefined): string => {
  switch (name) {
    case undefined:
      return "Agent"
    case "claude-code":
      return "Claude Code"
    case "codex-mcp-client":
      return "Codex"
    default:
      return name
  }
}

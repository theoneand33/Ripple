// Ripple for OpenCode (V2 plugins): shows each session on the Island's Coding Agents tab and asks
// there for permissions. Copy this file to ~/.config/opencode/plugins/. It runs `ripple hook
// opencode`, so `ripple` must be on PATH, or set RIPPLE to its full path.

import { Plugin } from "@opencode/plugin"
import { spawn } from "node:child_process"

const ripple = process.env.RIPPLE ?? "ripple"

// Sends one Claude-style hook event to Ripple and resolves with its answer ("" for none). After
// 10 minutes unanswered it gives up, as Claude Code's and Codex's hook timeouts do.
function hook(input: Record<string, unknown>): Promise<string> {
  return new Promise((resolve) => {
    const child = spawn(ripple, ["hook", "opencode"], { stdio: ["pipe", "pipe", "ignore"] })
    const timer = setTimeout(() => child.kill(), 600_000)
    let out = ""
    child.stdout.on("data", (chunk) => (out += chunk))
    child.on("error", () => { clearTimeout(timer); resolve("") })
    child.on("close", (code) => { clearTimeout(timer); resolve(code === 0 ? out : "") })
    // A failed spawn (ripple not on PATH) also breaks the pipe: ignore that, `error` above resolves.
    child.stdin.on("error", () => {})
    child.stdin.end(JSON.stringify(input))
  })
}

export default Plugin.define({
  id: "ripple",
  async setup(ctx) {
    const cwd = ctx.location.directory
    const controller = new AbortController()
    void (async () => {
      try {
        for await (const event of ctx.event.subscribe({ types: ["session.status", "session.deleted"], signal: controller.signal })) {
          const data = ((event as { data?: Record<string, any> }).data ?? {}) as Record<string, any>
          if (typeof data.sessionID !== "string") continue
          const name = event.type === "session.deleted" ? "SessionEnd" : data.status?.type === "idle" ? "Stop" : "UserPromptSubmit"
          await hook({ session_id: data.sessionID, cwd, hook_event_name: name })
        }
      } catch {
        // Aborted on unload.
      }
    })()

    await ctx.permission.hook("evaluate", async (event) => {
      // Questions arrive here too, but Allow can't answer one: OpenCode asks those itself.
      if (event.effect !== "ask" || event.action === "question") return
      const out = await hook({
        session_id: event.sessionID,
        cwd,
        hook_event_name: "PermissionRequest",
        tool_name: event.action,
        tool_input: { command: event.resources.join(" ") },
      })
      try {
        const behavior = JSON.parse(out).hookSpecificOutput.decision.behavior
        if (behavior === "allow" || behavior === "deny") event.effect = behavior
      } catch {
        // No answer: OpenCode asks as usual.
      }
    })

    return () => controller.abort()
  },
})

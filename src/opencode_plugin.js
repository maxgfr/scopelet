// ScopeletPlugin — written by `scopelet install --agent opencode`; remove it with
// `scopelet uninstall --agent opencode`, not by hand. Every hook shells out to the
// pinned Scopelet binary and leaves the native result untouched on any failure.
// One default export serves both OpenCode generations: OpenCode 2 runs `setup`
// (plugin API v2), OpenCode 1 (>= 1.18.29) runs `server` (legacy hook map).
import { execFile } from "node:child_process";

const BINARY = __SCOPELET_BINARY__;
const CONFIG = __SCOPELET_CONFIG__;
// Thresholds of the Rust binary, written at installation.
const SMALL = __SCOPELET_SMALL__;
const MAX_INPUT = __SCOPELET_MAX_INPUT__;

// Asynchronous, so a compression never blocks OpenCode's event loop.
function hook(event) {
  return new Promise((resolve) => {
    try {
      const child = execFile(
        BINARY,
        ["hook", "opencode"],
        {
          env: { ...process.env, SCOPELET_CONFIG_DIR: CONFIG },
          maxBuffer: MAX_INPUT * 4,
          timeout: 10000,
          encoding: "utf8",
        },
        (error, stdout) => {
          if (error) return resolve({});
          try {
            const result = JSON.parse(stdout);
            resolve(result && typeof result === "object" ? result : {});
          } catch {
            resolve({});
          }
        },
      );
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(event));
    } catch {
      resolve({});
    }
  });
}

const ScopeletPlugin = {
  id: "scopelet",
  // OpenCode 2: hooks are registered on the domain that owns the operation.
  async setup(ctx) {
    // The response-style preference (`scopelet mode`) rides the system prompt,
    // where Claude Code and Codex receive it as hook context.
    await ctx.session.hook("context", async (event) => {
      if (!event || !Array.isArray(event.system)) return;
      const result = await hook({ hook_event_name: "SystemPrompt", session_id: event.sessionID });
      if (typeof result.system === "string" && result.system) {
        event.system.push({ type: "text", text: result.system });
      }
    });
    await ctx.tool.hook("execute.after", async (event) => {
      if (!event || event.status !== "completed") return;
      const output = event.result;
      if (!output || typeof output.output !== "string") return;
      if (String(event.tool ?? "").toLowerCase() !== "bash") return;
      const bytes = Buffer.byteLength(output.output);
      if (bytes <= SMALL || bytes > MAX_INPUT) return;
      const command = event.input?.command;
      const result = await hook({
        hook_event_name: "ToolOutput",
        tool_name: "Bash",
        tool_input: { command: typeof command === "string" ? command : "" },
        tool_response: { output: output.output },
      });
      if (typeof result.output === "string") output.output = result.output;
    });
  },
  // OpenCode 1: the legacy hook map. Hook output objects are mutated in place.
  async server() {
    return {
      // The response-style preference (`scopelet mode`) rides the system prompt,
      // where Claude Code and Codex receive it as hook context.
      "experimental.chat.system.transform": async (input, output) => {
        if (!output || !Array.isArray(output.system)) return;
        const result = await hook({ hook_event_name: "SystemPrompt", session_id: input?.sessionID });
        if (typeof result.system === "string" && result.system) output.system.push(result.system);
      },
      "tool.execute.after": async (input, output) => {
        if (!output || typeof output.output !== "string") return;
        if (String(input?.tool ?? "").toLowerCase() !== "bash") return;
        const bytes = Buffer.byteLength(output.output);
        if (bytes <= SMALL || bytes > MAX_INPUT) return;
        const command = input?.args?.command;
        const result = await hook({
          hook_event_name: "ToolOutput",
          tool_name: "Bash",
          tool_input: { command: typeof command === "string" ? command : "" },
          tool_response: { output: output.output },
        });
        if (typeof result.output === "string") output.output = result.output;
      },
    };
  },
};

export default ScopeletPlugin;

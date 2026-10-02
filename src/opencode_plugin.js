// ScopeletPlugin — written by `scopelet install --agent opencode`; remove it with
// `scopelet uninstall --agent opencode`, not by hand. Every hook shells out to the
// pinned Scopelet binary and leaves the native result untouched on any failure.
// OpenCode treats every named export as a plugin, so this module exports only
// the plugin function. Hook output objects must be mutated in place.
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

export const ScopeletPlugin = async () => ({
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
});

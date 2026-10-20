// ScopeletPlugin — written by `scopelet install --agent opencode`; remove it with
// `scopelet uninstall --agent opencode`, not by hand. Every hook shells out to the
// pinned Scopelet binary and leaves the native result untouched on any failure.
// OpenCode 2 plugin API: one default export whose `setup` registers the hooks.
import { execFile } from "node:child_process";
import { lstat, readFile, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join } from "node:path";

const BINARY = __SCOPELET_BINARY__;
const CONFIG = __SCOPELET_CONFIG__;
// Thresholds of the Rust binary, written at installation.
const SMALL = __SCOPELET_SMALL__;
const MAX_INPUT = __SCOPELET_MAX_INPUT__;
// OpenCode keeps the tail of a long shell result and ends it with this notice.
const TRUNCATED = /\n+\[showing lines \d+-\d+ of \d+; full output saved to ([^\n\]]+)\]$/;

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

// The whole output OpenCode saved for a truncated shell result, or null. The
// path comes from model-visible text, so it is read only when it names a regular
// file directly inside OpenCode's shell output directory for this project
// (`<XDG data>/opencode/shell/<project id>`, derived the way OpenCode does),
// fits MAX_INPUT, is valid UTF-8 and ends with the tail OpenCode showed.
async function savedOutput(text, project) {
  const notice = TRUNCATED.exec(text);
  if (!notice || typeof project !== "string" || !/^[\w-]+$/.test(project)) return null;
  const file = notice[1];
  if (!isAbsolute(file) || !/^sh_\w+\.out$/.test(basename(file))) return null;
  const data = process.env.XDG_DATA_HOME || join(homedir(), ".local", "share");
  try {
    const [directory, parent] = await Promise.all([
      realpath(join(data, "opencode", "shell", project)),
      realpath(dirname(file)),
    ]);
    if (parent !== directory) return null;
    const stat = await lstat(file);
    if (!stat.isFile() || stat.size > MAX_INPUT) return null;
    const full = new TextDecoder("utf-8", { fatal: true }).decode(await readFile(file));
    const shown = text.slice(0, notice.index).replace(/\n+$/, "").slice(-1000);
    return full.replace(/\n+$/, "").endsWith(shown) ? full : null;
  } catch {
    return null;
  }
}

const ScopeletPlugin = {
  id: "scopelet",
  // Hooks are registered on the domain that owns the operation.
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
    // The shell tool is `shell`, its `result.output` is a structured object, and
    // the model reads `result.content`: the text parts are what gets compressed.
    await ctx.tool.hook("execute.after", async (event) => {
      if (!event || event.status !== "completed") return;
      if (String(event.tool ?? "").toLowerCase() !== "shell") return;
      const result = event.result;
      if (!result || !Array.isArray(result.content)) return;
      const texts = result.content.filter((part) => part?.type === "text" && typeof part.text === "string");
      if (texts.length === 0) return;
      const text = texts.map((part) => part.text).join("");
      // OpenCode truncates before this hook; compress the saved whole output
      // instead, so `scopelet expand` recovers every line.
      const output = (await savedOutput(text, ctx.location?.project?.id)) ?? text;
      const bytes = Buffer.byteLength(output);
      if (bytes <= SMALL || bytes > MAX_INPUT) return;
      const command = event.input?.command;
      const compressed = await hook({
        hook_event_name: "ToolOutput",
        session_id: typeof event.sessionID === "string" ? event.sessionID : undefined,
        tool_name: "Bash",
        tool_input: { command: typeof command === "string" ? command : "" },
        tool_response: { output },
      });
      if (typeof compressed.output !== "string") return;
      // In place: the first text part carries the compression, the others go.
      texts[0].text = compressed.output;
      for (let i = result.content.length - 1; i >= 0; i--) {
        if (texts.includes(result.content[i]) && result.content[i] !== texts[0]) result.content.splice(i, 1);
      }
      if (result.output && typeof result.output === "object" && typeof result.output.output === "string") {
        result.output.output = compressed.output;
      }
    });
  },
};

export default ScopeletPlugin;

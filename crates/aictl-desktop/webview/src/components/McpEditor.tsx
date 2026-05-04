import { Show, createSignal, onCleanup, onMount } from "solid-js";
import type { Component } from "solid-js";

import { ipc } from "../lib/ipc";

interface Props {
  /// Names of currently-configured servers — surfaced for the
  /// overwrite confirmation before the backend rejects the save.
  existingNames: string[];
  onSaved: (name: string) => void;
  onClose: () => void;
}

/// Parse the env-var textarea: one `KEY=VALUE` per line, blank lines
/// ignored. Returns `null` on the first malformed line so the caller
/// can render the error inline rather than letting the backend reject
/// the whole save.
function parseEnv(text: string): { ok: Record<string, string> } | { err: string } {
  const out: Record<string, string> = {};
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i += 1) {
    const raw = lines[i].trim();
    if (raw === "") continue;
    const eq = raw.indexOf("=");
    if (eq <= 0) {
      return { err: `line ${i + 1}: expected KEY=VALUE` };
    }
    const key = raw.slice(0, eq).trim();
    const value = raw.slice(eq + 1);
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
      return { err: `line ${i + 1}: invalid env key "${key}"` };
    }
    out[key] = value;
  }
  return { ok: out };
}

type Transport = "stdio" | "http" | "sse";

const McpEditor: Component<Props> = (props) => {
  const [name, setName] = createSignal("");
  const [transport, setTransport] = createSignal<Transport>("stdio");
  const [command, setCommand] = createSignal("");
  const [argsText, setArgsText] = createSignal("");
  const [envText, setEnvText] = createSignal("");
  const [url, setUrl] = createSignal("");
  const [headersText, setHeadersText] = createSignal("");
  const [timeoutText, setTimeoutText] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const isRemote = () => transport() !== "stdio";

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      props.onClose();
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  const onBackdropClick = (e: MouseEvent) => {
    if (e.target === e.currentTarget) props.onClose();
  };

  const validName = () => /^[A-Za-z0-9_-]+$/.test(name().trim());
  const clash = () =>
    name().trim() !== "" && props.existingNames.includes(name().trim());

  const save = async () => {
    setError(null);
    if (!validName()) {
      setError("Invalid name — letters, numbers, underscore, or dash only.");
      return;
    }
    if (isRemote()) {
      const u = url().trim();
      if (u === "") {
        setError("URL is empty.");
        return;
      }
      if (!u.startsWith("http://") && !u.startsWith("https://")) {
        setError("URL must start with http:// or https://.");
        return;
      }
    } else if (command().trim() === "") {
      setError("Command is empty.");
      return;
    }
    const argList = argsText()
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s !== "");
    const env = parseEnv(envText());
    if ("err" in env) {
      setError(env.err);
      return;
    }
    const headers = parseEnv(headersText());
    if ("err" in headers) {
      setError(`headers: ${headers.err}`);
      return;
    }
    let timeoutSecs: number | undefined;
    const t = timeoutText().trim();
    if (t !== "") {
      const parsed = Number.parseInt(t, 10);
      if (!Number.isFinite(parsed) || parsed <= 0) {
        setError("Timeout must be a positive integer.");
        return;
      }
      timeoutSecs = parsed;
    }
    if (clash()) {
      const ok = window.confirm(
        `A server named "${name().trim()}" already exists. Overwrite it?`,
      );
      if (!ok) return;
    }
    setSaving(true);
    try {
      await ipc.mcpCreate({
        name: name().trim(),
        transport: transport(),
        command: isRemote() ? "" : command().trim(),
        args: isRemote() ? [] : argList,
        env: isRemote() ? {} : env.ok,
        url: isRemote() ? url().trim() : "",
        headers: isRemote() ? headers.ok : {},
        timeoutSecs,
        overwrite: clash(),
      });
      props.onSaved(name().trim());
    } catch (err) {
      setError(`${err}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      class="editor-modal-overlay"
      role="dialog"
      aria-modal="true"
      onClick={onBackdropClick}
    >
      <div class="editor-modal-panel">
        <header class="editor-modal-header">
          <h2>New MCP server</h2>
          <button
            type="button"
            class="editor-modal-close"
            aria-label="Close new-mcp-server dialog"
            title="Close (Esc)"
            onClick={props.onClose}
          >
            ✕
          </button>
        </header>
        <div class="editor-modal-body">
          <Show when={error()}>
            <p class="editor-modal-error">{error()}</p>
          </Show>
          <div class="editor-modal-row">
            <label for="mcp-editor-name">Name</label>
            <input
              id="mcp-editor-name"
              type="text"
              placeholder="my-server"
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
            />
            <Show when={name() !== "" && !validName()}>
              <p class="editor-modal-help danger">
                Use only letters, numbers, underscore, or dash.
              </p>
            </Show>
            <Show when={validName() && clash()}>
              <p class="editor-modal-help warn">
                A server with this name already exists — saving will
                prompt to overwrite.
              </p>
            </Show>
          </div>
          <div class="editor-modal-row">
            <label for="mcp-editor-transport">Transport</label>
            <select
              id="mcp-editor-transport"
              value={transport()}
              onInput={(e) =>
                setTransport(e.currentTarget.value as Transport)
              }
            >
              <option value="stdio">stdio (local process)</option>
              <option value="http">http (remote, streamable)</option>
              <option value="sse">sse (remote, legacy)</option>
            </select>
            <p class="editor-modal-help">
              <code>stdio</code> spawns a local subprocess.{" "}
              <code>http</code> / <code>sse</code> dispatch over the
              network — see{" "}
              <code>AICTL_MCP_ALLOW_HOSTS</code> /{" "}
              <code>AICTL_MCP_DENY_HOSTS</code> to gate hostnames.
            </p>
          </div>
          <Show when={!isRemote()}>
            <div class="editor-modal-row">
              <label for="mcp-editor-command">Command</label>
              <input
                id="mcp-editor-command"
                type="text"
                placeholder="python or /usr/bin/node"
                value={command()}
                onInput={(e) => setCommand(e.currentTarget.value)}
              />
              <p class="editor-modal-help">
                Executable launched at startup. Spawned via the OS — no
                shell interpolation.
              </p>
            </div>
            <div class="editor-modal-row">
              <label for="mcp-editor-args">Arguments</label>
              <textarea
                id="mcp-editor-args"
                rows={3}
                placeholder={"one argument per line\n-m\nmcp_server"}
                value={argsText()}
                onInput={(e) => setArgsText(e.currentTarget.value)}
              />
              <p class="editor-modal-help">
                Optional. One argument per line.
              </p>
            </div>
            <div class="editor-modal-row">
              <label for="mcp-editor-env">Environment</label>
              <textarea
                id="mcp-editor-env"
                rows={3}
                placeholder={"KEY=value (one per line)\nAPI_KEY=${keyring:OPENAI_API_KEY}"}
                value={envText()}
                onInput={(e) => setEnvText(e.currentTarget.value)}
              />
              <p class="editor-modal-help">
                Optional. <code>{"${keyring:NAME}"}</code> pulls the
                named secret from the system keyring at spawn time.
              </p>
            </div>
          </Show>
          <Show when={isRemote()}>
            <div class="editor-modal-row">
              <label for="mcp-editor-url">URL</label>
              <input
                id="mcp-editor-url"
                type="text"
                placeholder="https://mcp.example.com/v1"
                value={url()}
                onInput={(e) => setUrl(e.currentTarget.value)}
              />
              <p class="editor-modal-help">
                Must use <code>https://</code> unless{" "}
                <code>AICTL_MCP_ALLOW_HTTP=true</code>.
              </p>
            </div>
            <div class="editor-modal-row">
              <label for="mcp-editor-headers">Headers</label>
              <textarea
                id="mcp-editor-headers"
                rows={3}
                placeholder={"Header=value (one per line)\nAuthorization=Bearer ${keyring:MCP_TOKEN}"}
                value={headersText()}
                onInput={(e) => setHeadersText(e.currentTarget.value)}
              />
              <p class="editor-modal-help">
                Optional. <code>{"${keyring:NAME}"}</code> pulls the
                named secret from the system keyring at request time.
              </p>
            </div>
          </Show>
          <div class="editor-modal-row">
            <label for="mcp-editor-timeout">Timeout</label>
            <input
              id="mcp-editor-timeout"
              type="number"
              min="1"
              placeholder="30"
              value={timeoutText()}
              onInput={(e) => setTimeoutText(e.currentTarget.value)}
            />
            <p class="editor-modal-help">
              Optional per-call RPC timeout in seconds. Defaults to{" "}
              <code>AICTL_MCP_TIMEOUT</code> (30s).
            </p>
          </div>
        </div>
        <footer class="editor-modal-footer">
          <button type="button" disabled={saving()} onClick={props.onClose}>
            Cancel
          </button>
          <button type="button" disabled={saving()} onClick={() => void save()}>
            {saving() ? "Saving…" : "Save"}
          </button>
        </footer>
      </div>
    </div>
  );
};

export default McpEditor;

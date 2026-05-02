import type { Component } from "solid-js";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";

import {
  ipc,
  type ActiveModel,
  type AgentRow,
  type AgentView,
  type ConfigEntry,
  type ContextStatus,
  type HookRow,
  type HooksStatus,
  type KeyBackend,
  type KeyRow,
  type McpStatus,
  type ModelEntry,
  type OllamaProbeResult,
  type OllamaStatus,
  type PluginsStatus,
  type ServerProbeResult,
  type ServerStatus,
  type SessionRow,
  type SkillRow,
  type SkillView,
  type StatsBucket,
  type StatsSnapshot,
  type ToolRow,
  type WorkspaceState,
} from "../lib/ipc";
import { renderMarkdown } from "../lib/markdown";

interface Props {
  workspace: WorkspaceState;
  onPickWorkspace: () => void | Promise<void>;
  onClose: () => void;
  models: ModelEntry[];
  activeModel: ActiveModel;
  onChangeModel: (provider: string, model: string) => Promise<void>;
}

type Tab =
  | "general"
  | "security"
  | "provider"
  | "keys"
  | "server"
  | "mcp"
  | "hooks"
  | "skills"
  | "agents"
  | "plugins"
  | "sessions"
  | "context"
  | "stats"
  | "redaction"
  | "shell"
  | "appearance"
  | "about";

const TABS: { id: Tab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "appearance", label: "Appearance" },
  { id: "provider", label: "Model" },
  { id: "keys", label: "API Keys" },
  { id: "security", label: "Security" },
  { id: "redaction", label: "Redaction" },
  { id: "shell", label: "Shell" },
  { id: "server", label: "Server" },
  { id: "mcp", label: "MCP" },
  { id: "hooks", label: "Hooks" },
  { id: "skills", label: "Skills" },
  { id: "agents", label: "Agents" },
  { id: "plugins", label: "Plugins" },
  { id: "sessions", label: "Sessions" },
  { id: "context", label: "Context" },
  { id: "stats", label: "Stats" },
  { id: "about", label: "About" },
];

const PROVIDER_LABELS: Record<string, string> = {
  anthropic: "Anthropic",
  openai: "OpenAI",
  gemini: "Gemini",
  grok: "Grok",
  mistral: "Mistral",
  deepseek: "DeepSeek",
  kimi: "Kimi",
  zai: "Z.ai",
  ollama: "Ollama",
  gguf: "Native GGUF",
  mlx: "Native MLX",
  "aictl-server": "aictl-server",
};

const Settings: Component<Props> = (props) => {
  const [tab, setTab] = createSignal<Tab>("general");

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

  return (
    <div class="settings-overlay" role="dialog" aria-modal="true">
      <div class="settings-panel">
        <header class="settings-header">
          <h2>Settings</h2>
          <button
            type="button"
            class="settings-close"
            aria-label="Close settings"
            title="Close (Esc)"
            onClick={props.onClose}
          >
            ✕
          </button>
        </header>
        <div class="settings-body">
          <nav class="settings-tabs">
            <For each={TABS}>
              {(t) => (
                <button
                  type="button"
                  class="settings-tab"
                  data-active={String(tab() === t.id)}
                  onClick={() => setTab(t.id)}
                >
                  {t.label}
                </button>
              )}
            </For>
          </nav>
          <section class="settings-content">
            <Show when={tab() === "provider"}>
              <ProviderTab
                models={props.models}
                activeModel={props.activeModel}
                onChangeModel={props.onChangeModel}
              />
            </Show>
            <Show when={tab() === "keys"}>
              <KeysTab />
            </Show>
            <Show when={tab() === "general"}>
              <GeneralTab
                workspace={props.workspace}
                onPickWorkspace={props.onPickWorkspace}
              />
            </Show>
            <Show when={tab() === "security"}>
              <SecurityTab />
            </Show>
            <Show when={tab() === "appearance"}>
              <AppearanceTab />
            </Show>
            <Show when={tab() === "server"}>
              <ServerTab />
            </Show>
            <Show when={tab() === "mcp"}>
              <McpTab />
            </Show>
            <Show when={tab() === "hooks"}>
              <HooksTab />
            </Show>
            <Show when={tab() === "skills"}>
              <SkillsTab />
            </Show>
            <Show when={tab() === "agents"}>
              <AgentsTab />
            </Show>
            <Show when={tab() === "plugins"}>
              <PluginsTab />
            </Show>
            <Show when={tab() === "sessions"}>
              <SessionsTab />
            </Show>
            <Show when={tab() === "context"}>
              <ContextTab />
            </Show>
            <Show when={tab() === "stats"}>
              <StatsTab />
            </Show>
            <Show when={tab() === "redaction"}>
              <RedactionTab />
            </Show>
            <Show when={tab() === "shell"}>
              <ShellTab />
            </Show>
            <Show when={tab() === "about"}>
              <AboutTab />
            </Show>
          </section>
        </div>
      </div>
    </div>
  );
};

interface ProviderTabProps {
  models: ModelEntry[];
  activeModel: ActiveModel;
  onChangeModel: (provider: string, model: string) => Promise<void>;
}

const ProviderTab: Component<ProviderTabProps> = (props) => {
  const [error, setError] = createSignal<string | null>(null);

  const groups = createMemo(() => {
    const order: string[] = [];
    const buckets = new Map<string, string[]>();
    for (const e of props.models) {
      if (!buckets.has(e.provider)) {
        buckets.set(e.provider, []);
        order.push(e.provider);
      }
      buckets.get(e.provider)!.push(e.model);
    }
    return order.map((provider) => ({
      provider,
      label: PROVIDER_LABELS[provider] ?? provider,
      models: buckets.get(provider)!,
    }));
  });

  const select = async (provider: string, model: string) => {
    setError(null);
    try {
      await props.onChangeModel(provider, model);
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Model</h3>
      <p class="settings-hint">
        Pick which model the chat uses. The composer's dropdown points
        at the same setting.
      </p>
      <div class="settings-row">
        <label>Active</label>
        <div class="settings-value">
          <Show
            when={props.activeModel.provider && props.activeModel.model}
            fallback={
              <span class="settings-empty">
                No model selected — pick one below.
              </span>
            }
          >
            <code>
              {PROVIDER_LABELS[props.activeModel.provider!] ??
                props.activeModel.provider}{" "}
              · {props.activeModel.model}
            </code>
          </Show>
        </div>
      </div>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <div class="settings-model-grid">
        <For each={groups()}>
          {(group) => (
            <div class="settings-model-group">
              <h4>{group.label}</h4>
              <ul>
                <For each={group.models}>
                  {(model) => {
                    const isActive = () =>
                      props.activeModel.provider === group.provider &&
                      props.activeModel.model === model;
                    return (
                      <li>
                        <button
                          type="button"
                          class="settings-model-option"
                          data-active={String(isActive())}
                          onClick={() => void select(group.provider, model)}
                        >
                          {model}
                        </button>
                      </li>
                    );
                  }}
                </For>
              </ul>
            </div>
          )}
        </For>
      </div>
    </div>
  );
};

const KeysTab: Component = () => {
  const [rows, { refetch }] = createResource<KeyRow[]>(() => ipc.keysStatus());
  const [backend] = createResource<KeyBackend>(() => ipc.keysBackend());
  const [editing, setEditing] = createSignal<string | null>(null);
  const [draft, setDraft] = createSignal("");
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  const save = async (name: string) => {
    setError(null);
    setFeedback(null);
    const value = draft().trim();
    if (!value) {
      setError("value is empty");
      return;
    }
    try {
      const where = await ipc.keysSet(name, value);
      setFeedback(`saved to ${where}`);
      setEditing(null);
      setDraft("");
      await refetch();
    } catch (err) {
      setError(`${err}`);
    }
  };

  const remove = async (name: string) => {
    setError(null);
    setFeedback(null);
    try {
      await ipc.keysClear(name);
      await refetch();
      setFeedback("cleared");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const lock = async (name: string) => {
    setError(null);
    setFeedback(null);
    try {
      const outcome = await ipc.keysLock(name);
      await refetch();
      setFeedback(
        outcome === "already_locked"
          ? `${name} already in keyring`
          : `${name} → keyring`,
      );
    } catch (err) {
      setError(`${err}`);
    }
  };

  const unlock = async (name: string) => {
    setError(null);
    setFeedback(null);
    try {
      const outcome = await ipc.keysUnlock(name);
      await refetch();
      setFeedback(
        outcome === "already_unlocked"
          ? `${name} already in config`
          : `${name} → config`,
      );
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>API Keys</h3>
      <p class="settings-hint">
        Stored in the system keychain when available, otherwise in
        plain <code>~/.aictl/config</code>.
        <br />
        Local providers (Ollama, GGUF, MLX) don't need keys.
      </p>
      <Show when={backend()}>
        {(b) => (
          <p class="settings-meta">
            Backend: <code>{b().name}</code>
            {b().available ? "" : " — falling back to plain config"}
          </p>
        )}
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <table class="settings-keys-table">
        <thead>
          <tr>
            <th>Provider</th>
            <th>Key name</th>
            <th>Status</th>
            <th />
          </tr>
        </thead>
        <tbody>
          <For each={rows() ?? []}>
            {(row) => (
              <tr>
                <td>{row.label || row.name}</td>
                <td>
                  <code>{row.name}</code>
                </td>
                <td>
                  <span data-status={row.location}>{row.location}</span>
                </td>
                <td class="settings-keys-actions">
                  <Show
                    when={editing() === row.name}
                    fallback={
                      <>
                        <button
                          type="button"
                          class="ghost mini"
                          onClick={() => {
                            setEditing(row.name);
                            setDraft("");
                            setFeedback(null);
                            setError(null);
                          }}
                        >
                          {row.location === "unset" ? "Set" : "Replace"}
                        </button>
                        <Show
                          when={
                            backend()?.available &&
                            (row.location === "plain" || row.location === "both")
                          }
                        >
                          <button
                            type="button"
                            class="ghost mini"
                            title="Move from plain config to system keyring"
                            onClick={() => void lock(row.name)}
                          >
                            Lock
                          </button>
                        </Show>
                        <Show
                          when={
                            backend()?.available &&
                            (row.location === "keyring" ||
                              row.location === "both")
                          }
                        >
                          <button
                            type="button"
                            class="ghost mini"
                            title="Move from system keyring back to plain config"
                            onClick={() => void unlock(row.name)}
                          >
                            Unlock
                          </button>
                        </Show>
                        <Show when={row.location !== "unset"}>
                          <button
                            type="button"
                            class="ghost mini danger"
                            onClick={() => void remove(row.name)}
                          >
                            Clear
                          </button>
                        </Show>
                      </>
                    }
                  >
                    <input
                      type="password"
                      class="settings-keys-input"
                      placeholder="paste key…"
                      value={draft()}
                      autofocus
                      onInput={(e) => setDraft(e.currentTarget.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          void save(row.name);
                        } else if (e.key === "Escape") {
                          setEditing(null);
                          setDraft("");
                        }
                      }}
                    />
                    <button
                      type="button"
                      class="ghost mini"
                      onClick={() => void save(row.name)}
                    >
                      Save
                    </button>
                    <button
                      type="button"
                      class="ghost mini"
                      onClick={() => {
                        setEditing(null);
                        setDraft("");
                      }}
                    >
                      Cancel
                    </button>
                  </Show>
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
};

const SECURITY_BOOL_KEYS: { key: string; label: string; help: string }[] = [
  {
    key: "AICTL_SECURITY",
    label: "Security policy",
    help: "Master switch for the security gate. Off disables CWD jail, shell allow-list, and tool denial — leave on unless you really know what you're doing.",
  },
  {
    key: "AICTL_SECURITY_INJECTION_GUARD",
    label: "Prompt-injection guard",
    help: "Scans tool output for adversarial injection patterns before feeding it back to the agent.",
  },
  {
    key: "AICTL_SECURITY_AUDIT_LOG",
    label: "Audit log",
    help: "Logs every tool call to ~/.aictl/audit/<session-id>. Useful for review; takes disk.",
  },
  {
    key: "AICTL_SECURITY_REDACTION",
    label: "Redact secrets to providers",
    help: "Strip API keys, tokens, and other secrets from outbound LLM payloads.",
  },
  {
    key: "AICTL_SECURITY_REDACTION_LOCAL",
    label: "Apply redaction to local providers too",
    help: "Off by default — Ollama / GGUF / MLX run on your machine.",
  },
  {
    key: "AICTL_SECURITY_CWD_RESTRICT",
    label: "Restrict tools to workspace folder",
    help: "When on, file-system tools refuse paths outside the workspace. Foundation of the CWD jail.",
  },
  {
    key: "AICTL_SECURITY_BLOCK_SUBSHELL",
    label: "Block shell metacharacters",
    help: "Refuse subshell / pipe / redirect syntax in shell commands. Off lets the agent run pipelines but loses one layer of defense.",
  },
];

const MISC_BOOL_KEYS: { key: string; label: string; help: string }[] = [
  {
    key: "AICTL_PROMPT_FALLBACK",
    label: "Project prompt-file fallback",
    help: "When AICTL.md is missing, also try CLAUDE.md and AGENTS.md. Off skips the fallbacks.",
  },
  {
    key: "AICTL_STREAMING",
    label: "Stream tokens",
    help: "Render the model's output as it arrives. Off waits until the full response has been received.",
  },
];

const NUM_KEYS: {
  key: string;
  label: string;
  help: string;
  suffix: string;
  defaultValue: string;
}[] = [
  {
    key: "AICTL_AUTO_COMPACT_THRESHOLD",
    label: "Auto-compact threshold",
    help: "Compact context automatically when usage crosses this percentage. 0 disables. Leave blank for the default.",
    suffix: "%",
    defaultValue: "80",
  },
  {
    key: "AICTL_LLM_TIMEOUT",
    label: "LLM call timeout",
    help: "Per-request timeout in seconds. 0 disables. Leave blank for the default.",
    suffix: "s",
    defaultValue: "30",
  },
  {
    key: "AICTL_MAX_ITERATIONS",
    label: "Max iterations per turn",
    help: "Cap on LLM calls inside one agent turn — bounds runaway tool-call loops. Leave blank for the default.",
    suffix: "",
    defaultValue: "20",
  },
];

const GeneralTab: Component<{
  workspace: WorkspaceState;
  onPickWorkspace: () => void | Promise<void>;
}> = (props) => {
  const [config, { refetch }] = createResource<ConfigEntry[]>(() =>
    ipc.configDump(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const get = (key: string): string | null => {
    const entry = (config() ?? []).find((e) => e.key === key);
    return entry?.value ?? null;
  };

  const isOn = (key: string): boolean => {
    const v = get(key);
    if (v === null) return true;
    return v !== "false" && v !== "0";
  };

  const setBool = async (key: string, on: boolean) => {
    setError(null);
    setFeedback(null);
    try {
      if (on) {
        await ipc.configClear(key);
      } else {
        await ipc.configWrite(key, "false");
      }
      await refetch();
      setFeedback(`${key} = ${on ? "on" : "off"}`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  const setNum = async (key: string, value: string) => {
    setError(null);
    setFeedback(null);
    try {
      if (value.trim() === "") {
        await ipc.configClear(key);
      } else {
        await ipc.configWrite(key, value.trim());
      }
      await refetch();
      setFeedback(`${key} updated`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  const setText = async (key: string, value: string) => {
    setError(null);
    setFeedback(null);
    try {
      if (value.trim() === "") {
        await ipc.configClear(key);
      } else {
        await ipc.configWrite(key, value);
      }
      await refetch();
      setFeedback(`${key} updated`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  const memoryMode = (): "long-term" | "short-term" =>
    get("AICTL_MEMORY") === "short-term" ? "short-term" : "long-term";

  const setMemory = async (mode: "long-term" | "short-term") => {
    setError(null);
    setFeedback(null);
    try {
      if (mode === "long-term") {
        await ipc.configClear("AICTL_MEMORY");
      } else {
        await ipc.configWrite("AICTL_MEMORY", "short-term");
      }
      await refetch();
      setFeedback(`memory mode = ${mode}`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  const approvalMode = (): "ask" | "auto" =>
    get("AICTL_TOOL_APPROVAL") === "auto" ? "auto" : "ask";

  const setApproval = async (mode: "ask" | "auto") => {
    setError(null);
    setFeedback(null);
    try {
      if (mode === "ask") {
        await ipc.configClear("AICTL_TOOL_APPROVAL");
      } else {
        await ipc.configWrite("AICTL_TOOL_APPROVAL", "auto");
      }
      await refetch();
      setFeedback(`tool approval = ${mode}`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  const toolsOn = (): boolean => isOn("AICTL_TOOLS_ENABLED");

  return (
    <div class="settings-tab-content">
      <h3>General</h3>
      <p class="settings-hint">
        Engine knobs the desktop shares with the CLI through{" "}
        <code>~/.aictl/config</code>.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>

      <h4 class="settings-subhead">Workspace</h4>
      <div class="settings-row settings-row-stack">
        <label>Workspace folder</label>
        <p class="settings-hint">
          The CWD jail root for every tool call — the agent can only
          read and write files inside it.
        </p>
        <div class="settings-value">
          <Show
            when={props.workspace.path}
            fallback={<span class="settings-empty">No workspace selected</span>}
          >
            <code>{props.workspace.path}</code>
          </Show>
        </div>
        <Show when={props.workspace.error}>
          <p class="settings-error">{props.workspace.error}</p>
        </Show>
        <div class="settings-actions">
          <button type="button" onClick={() => void props.onPickWorkspace()}>
            {props.workspace.path ? "Change workspace…" : "Pick workspace…"}
          </button>
        </div>
      </div>

      <h4 class="settings-subhead">Memory</h4>
      <div class="settings-row settings-row-stack">
        <label>Conversation memory</label>
        <div class="settings-control-line">
          <select
            class="settings-select"
            value={memoryMode()}
            onChange={(e) =>
              void setMemory(
                e.currentTarget.value as "long-term" | "short-term",
              )
            }
          >
            <option value="long-term">Long-term (full history)</option>
            <option value="short-term">Short-term (recent window)</option>
          </select>
        </div>
        <p class="settings-hint">
          Long-term sends the full transcript on every turn. Short-term
          sends only the most recent window — cheaper, but the agent
          forgets older context. Mirrors the CLI's <code>/memory</code>.
        </p>
      </div>

      <h4 class="settings-subhead">Tool approval</h4>
      <div class="settings-row settings-row-stack">
        <label>Default approval mode</label>
        <div class="settings-control-line">
          <select
            class="settings-select"
            value={approvalMode()}
            onChange={(e) =>
              void setApproval(e.currentTarget.value as "ask" | "auto")
            }
          >
            <option value="ask">Ask each tool call (recommended)</option>
            <option value="auto">Auto-accept all tool calls</option>
          </select>
        </div>
        <p class="settings-hint">
          The composer's per-conversation toggle still wins for the
          current session — this picks the default when the desktop
          launches.
        </p>
      </div>

      <h4 class="settings-subhead">Behavior</h4>
      <BehaviorEditor onSaved={() => void refetch()} />

      <h4 class="settings-subhead">Numbers</h4>
      <For each={NUM_KEYS}>
        {(spec) => (
          <NumberRow
            label={spec.label}
            help={spec.help}
            suffix={spec.suffix}
            initial={get(spec.key) ?? ""}
            placeholder={spec.defaultValue}
            onCommit={(v) => void setNum(spec.key, v)}
          />
        )}
      </For>

      <h4 class="settings-subhead">Tools</h4>
      <BoolRow
        label="Tools enabled"
        help="Master switch — turn off to run the agent in chat-only mode (no shell, no file edits, no MCP)."
        on={toolsOn()}
        onChange={(v) => void setBool("AICTL_TOOLS_ENABLED", v)}
      />
      <ToolsList disabled={!toolsOn()} />

      <h4 class="settings-subhead">Misc</h4>
      <For each={MISC_BOOL_KEYS}>
        {(spec) => (
          <BoolRow
            label={spec.label}
            help={spec.help}
            on={isOn(spec.key)}
            onChange={(v) => void setBool(spec.key, v)}
          />
        )}
      </For>
    </div>
  );
};

const SecurityTab: Component = () => {
  const [config, { refetch }] = createResource<ConfigEntry[]>(() =>
    ipc.configDump(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const get = (key: string): string | null => {
    const entry = (config() ?? []).find((e) => e.key === key);
    return entry?.value ?? null;
  };

  const isOn = (key: string): boolean => {
    const v = get(key);
    if (v === null) return true;
    return v !== "false" && v !== "0";
  };

  const setBool = async (key: string, on: boolean) => {
    setError(null);
    setFeedback(null);
    try {
      if (on) {
        await ipc.configClear(key);
      } else {
        await ipc.configWrite(key, "false");
      }
      await refetch();
      setFeedback(`${key} = ${on ? "on" : "off"}`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Security</h3>
      <p class="settings-hint">
        Master toggles for the security gate, audit log, prompt-injection
        guard, and outbound redaction. Fine-grained shell / path /
        redaction rules live in their own tabs.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <For each={SECURITY_BOOL_KEYS}>
        {(spec) => (
          <BoolRow
            label={spec.label}
            help={spec.help}
            on={isOn(spec.key)}
            onChange={(v) => void setBool(spec.key, v)}
          />
        )}
      </For>
    </div>
  );
};

const BehaviorEditor: Component<{ onSaved: () => void | Promise<unknown> }> = (
  props,
) => {
  const [initial, { refetch }] = createResource<string>(async () => {
    const v = await ipc.configValue("AICTL_BEHAVIOR");
    return v ?? "";
  });
  const [draft, setDraft] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  createEffect(() => {
    const v = initial();
    if (v !== undefined) setDraft(v);
  });

  const save = async () => {
    setError(null);
    setFeedback(null);
    try {
      const trimmed = draft().trim();
      if (trimmed === "") {
        await ipc.configClear("AICTL_BEHAVIOR");
      } else {
        await ipc.configWrite("AICTL_BEHAVIOR", draft());
      }
      await refetch();
      await props.onSaved();
      setFeedback("saved");
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-row settings-row-stack">
      <label>Persistent behavior override</label>
      <p class="settings-hint">
        Free-form text appended to every system prompt. Use it to lock
        in coding conventions, tone, or guardrails the agent must follow
        across every session.
      </p>
      <textarea
        class="settings-textarea"
        rows={6}
        placeholder="e.g. Always use snake_case in Python; never write to /tmp."
        value={draft()}
        onInput={(e) => setDraft(e.currentTarget.value)}
      />
      <div class="settings-actions">
        <button type="button" onClick={() => void save()}>
          Save
        </button>
        <button
          type="button"
          onClick={() => {
            setDraft("");
            void save();
          }}
        >
          Clear
        </button>
      </div>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
    </div>
  );
};

const ToolsList: Component<{ disabled: boolean }> = (props) => {
  const [tools, { refetch }] = createResource<ToolRow[]>(() => ipc.toolsList());
  const [error, setError] = createSignal<string | null>(null);

  const toggle = async (name: string, disable: boolean) => {
    setError(null);
    try {
      await ipc.toolSetDisabled(name, disable);
      await refetch();
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-row settings-row-stack">
      <label>Per-tool enable / disable</label>
      <p class="settings-hint">
        Disabled tools are stripped from the system prompt and refused at
        the security gate. Stored as a comma-separated list in{" "}
        <code>AICTL_SECURITY_DISABLED_TOOLS</code>.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <ul class="settings-tools-list" data-dim={String(props.disabled)}>
        <For each={tools() ?? []}>
          {(tool) => (
            <li>
              <label class="settings-tool-item">
                <input
                  type="checkbox"
                  checked={!tool.disabled}
                  disabled={props.disabled}
                  onChange={(e) =>
                    void toggle(tool.name, !e.currentTarget.checked)
                  }
                />
                <span class="settings-tool-name">
                  <code>{tool.name}</code>
                </span>
                <span class="settings-tool-desc">{tool.description}</span>
              </label>
            </li>
          )}
        </For>
      </ul>
    </div>
  );
};

const NumberRow: Component<{
  label: string;
  help: string;
  suffix: string;
  initial: string;
  placeholder?: string;
  onCommit: (value: string) => void;
}> = (props) => {
  const [value, setValue] = createSignal(props.initial);
  createEffect(() => setValue(props.initial));
  return (
    <div class="settings-row settings-row-stack">
      <label>{props.label}</label>
      <div class="settings-control-line">
        <input
          type="number"
          min="0"
          class="settings-num-input"
          value={value()}
          placeholder={props.placeholder}
          onInput={(e) => setValue(e.currentTarget.value)}
          onBlur={() => props.onCommit(value())}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              props.onCommit(value());
            }
          }}
        />
        <span class="settings-suffix">{props.suffix}</span>
        <Show when={props.placeholder && value() === ""}>
          <span class="settings-default-hint">
            default: <code>{props.placeholder}</code>
          </span>
        </Show>
      </div>
      <p class="settings-hint">{props.help}</p>
    </div>
  );
};

const TextRow: Component<{
  label: string;
  help: string;
  initial: string;
  placeholder?: string;
  onCommit: (value: string) => void;
}> = (props) => {
  const [value, setValue] = createSignal(props.initial);
  createEffect(() => setValue(props.initial));
  return (
    <div class="settings-row settings-row-stack">
      <label>{props.label}</label>
      <div class="settings-control-line">
        <input
          type="text"
          class="settings-text-input"
          value={value()}
          placeholder={props.placeholder}
          onInput={(e) => setValue(e.currentTarget.value)}
          onBlur={() => props.onCommit(value())}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              props.onCommit(value());
            }
          }}
        />
      </div>
      <p class="settings-hint">{props.help}</p>
    </div>
  );
};

const BoolRow: Component<{
  label: string;
  help: string;
  on: boolean;
  onChange: (next: boolean) => void;
}> = (props) => (
  <div class="settings-row settings-row-stack">
    <div class="settings-bool-line">
      <label>
        <input
          type="checkbox"
          checked={props.on}
          onChange={(e) => props.onChange(e.currentTarget.checked)}
        />
        <span>{props.label}</span>
      </label>
    </div>
    <p class="settings-hint">{props.help}</p>
  </div>
);

const ServerTab: Component = () => {
  const [status, { refetch }] = createResource<ServerStatus>(() =>
    ipc.serverStatus(),
  );
  const [host, setHost] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [probe, setProbe] = createSignal<ServerProbeResult | null>(null);
  const [probing, setProbing] = createSignal(false);

  // Track whether the input has unsaved edits so the Save button can
  // show whether it's a no-op without doing a round-trip first. The
  // server `host` and the input draft start in sync (createEffect just
  // below); the dirty flag is flipped by the input's onInput.
  const [dirty, setDirty] = createSignal(false);

  createEffect(() => {
    const s = status();
    if (s) {
      setHost(s.host ?? "");
      setDirty(false);
    }
  });

  const saveHost = async () => {
    setError(null);
    setFeedback(null);
    try {
      const v = host().trim();
      if (v === "") {
        await ipc.configClear("AICTL_CLIENT_HOST");
      } else {
        await ipc.configWrite("AICTL_CLIENT_HOST", v);
      }
      await refetch();
      setFeedback("aictl-server host saved");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const runProbe = async () => {
    setError(null);
    setFeedback(null);
    setProbing(true);
    setProbe(null);
    try {
      setProbe(await ipc.serverProbe());
    } catch (err) {
      setError(`${err}`);
    } finally {
      setProbing(false);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>aictl-server</h3>
      <p class="settings-hint">
        Route LLM calls through a self-hosted{" "}
        <code>aictl-server</code> by selecting the{" "}
        <code>aictl-server</code> provider in the Model tab. The host
        URL and master key are also stored in{" "}
        <code>~/.aictl/config</code> so the CLI sees the same values.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <div class="settings-row settings-row-stack">
        <label>Host URL</label>
        <div class="settings-control-line">
          <input
            type="text"
            class="settings-text-input"
            placeholder="https://aictl-server.example.com"
            value={host()}
            onInput={(e) => {
              setHost(e.currentTarget.value);
              setDirty(true);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                if (dirty()) void saveHost();
              }
            }}
          />
        </div>
        <p class="settings-hint">
          Stored in <code>AICTL_CLIENT_HOST</code>. The master key
          (<code>AICTL_CLIENT_MASTER_KEY</code>) is configured in the
          API Keys tab.
        </p>
        <div class="settings-actions">
          <button type="button" disabled={!dirty()} onClick={() => void saveHost()}>
            Save host
          </button>
        </div>
      </div>
      <div class="settings-row">
        <label>Master key</label>
        <div class="settings-value">
          <Show
            when={status()?.master_key_set}
            fallback={
              <span class="settings-empty">
                Not set — add it in the API Keys tab.
              </span>
            }
          >
            <code>configured</code>
          </Show>
        </div>
      </div>
      <div class="settings-row">
        <label>Connection</label>
        <div class="settings-value">
          <Show
            when={status()?.fully_configured}
            fallback={
              <span class="settings-empty">
                Host or master key still missing.
              </span>
            }
          >
            <code>ready</code>
          </Show>
        </div>
      </div>
      <div class="settings-actions">
        <button type="button" disabled={probing()} onClick={() => void runProbe()}>
          {probing() ? "Probing…" : "Run /healthz + key probe"}
        </button>
      </div>
      <Show when={probe()}>
        {(p) => (
          <div class="settings-probe">
            <p class="settings-meta">
              <code>/healthz</code>:{" "}
              <span data-status={p().healthz_ok ? "ok" : "fail"}>
                {p().healthz_ok ? "ok" : "fail"}
              </span>
              <Show when={p().healthz_status}>
                {(s) => <> ({s()})</>}
              </Show>
              <Show when={p().healthz_error}>
                {(e) => <> — {e()}</>}
              </Show>
            </p>
            <p class="settings-meta">
              <code>/v1/models</code>:{" "}
              <span data-status={p().models_ok ? "ok" : "fail"}>
                {p().models_ok ? "ok" : "fail"}
              </span>
              <Show when={p().models_status}>
                {(s) => <> ({s()})</>}
              </Show>
              <Show when={p().models_error}>
                {(e) => <> — {e()}</>}
              </Show>
              <Show when={p().model_count !== null}>
                <> — {p().model_count} models advertised</>
              </Show>
            </p>
          </div>
        )}
      </Show>

      <h4 class="settings-subhead">Ollama</h4>
      <OllamaSection />
    </div>
  );
};

const OllamaSection: Component = () => {
  const [status, { refetch }] = createResource<OllamaStatus>(() =>
    ipc.ollamaStatus(),
  );
  const [host, setHost] = createSignal("");
  const [dirty, setDirty] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [probe, setProbe] = createSignal<OllamaProbeResult | null>(null);
  const [probing, setProbing] = createSignal(false);

  // The IPC returns the resolved host (override or default). The input
  // shows the override so blanking it falls back to the default rather
  // than persisting `http://localhost:11434` as if it were custom.
  createEffect(() => {
    const s = status();
    if (s) {
      setHost(s.overridden ? s.host : "");
      setDirty(false);
    }
  });

  const save = async () => {
    setError(null);
    setFeedback(null);
    try {
      const v = host().trim();
      if (v === "") {
        await ipc.configClear("LLM_OLLAMA_HOST");
      } else {
        await ipc.configWrite("LLM_OLLAMA_HOST", v);
      }
      await refetch();
      setFeedback(
        v === "" ? "reverted to default localhost" : "ollama host saved",
      );
    } catch (err) {
      setError(`${err}`);
    }
  };

  const test = async () => {
    setError(null);
    setFeedback(null);
    setProbe(null);
    setProbing(true);
    try {
      setProbe(await ipc.ollamaProbe());
    } catch (err) {
      setError(`${err}`);
    } finally {
      setProbing(false);
    }
  };

  return (
    <>
      <p class="settings-hint">
        Local Ollama daemon. Default endpoint is{" "}
        <code>http://localhost:11434</code>; override here to point at a
        remote box on your network. Stored in{" "}
        <code>LLM_OLLAMA_HOST</code>.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <div class="settings-row settings-row-stack">
        <label>Host URL</label>
        <div class="settings-control-line">
          <input
            type="text"
            class="settings-text-input"
            placeholder={status()?.default_host ?? "http://localhost:11434"}
            value={host()}
            onInput={(e) => {
              setHost(e.currentTarget.value);
              setDirty(true);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                if (dirty()) void save();
              }
            }}
          />
          <Show when={!status()?.overridden && host() === ""}>
            <span class="settings-default-hint">
              default: <code>{status()?.default_host}</code>
            </span>
          </Show>
        </div>
        <div class="settings-actions">
          <button type="button" disabled={!dirty()} onClick={() => void save()}>
            Save host
          </button>
          <button type="button" disabled={probing()} onClick={() => void test()}>
            {probing() ? "Testing…" : "Test connection"}
          </button>
        </div>
      </div>
      <Show when={probe()}>
        {(p) => (
          <div class="settings-probe">
            <p class="settings-meta">
              <code>/api/tags</code>:{" "}
              <span data-status={p().ok ? "ok" : "fail"}>
                {p().ok ? "ok" : "fail"}
              </span>
              <Show when={p().status}>
                {(s) => <> ({s()})</>}
              </Show>
              <Show when={p().error}>
                {(e) => <> — {e()}</>}
              </Show>
              <Show when={p().model_count !== null}>
                <> — {p().model_count} model{p().model_count === 1 ? "" : "s"} available</>
              </Show>
            </p>
            <Show when={p().sample_models.length > 0}>
              <p class="settings-meta">
                Models:{" "}
                <For each={p().sample_models}>
                  {(name, i) => (
                    <>
                      <Show when={i() > 0}>{", "}</Show>
                      <code>{name}</code>
                    </>
                  )}
                </For>
              </p>
            </Show>
          </div>
        )}
      </Show>
    </>
  );
};

const McpTab: Component = () => {
  const [status, { refetch }] = createResource<McpStatus>(() => ipc.mcpStatus());
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const setEnabled = async (on: boolean) => {
    setError(null);
    setFeedback(null);
    try {
      if (on) {
        await ipc.configWrite("AICTL_MCP_ENABLED", "true");
      } else {
        await ipc.configClear("AICTL_MCP_ENABLED");
      }
      await refetch();
      setFeedback(`MCP ${on ? "enabled" : "disabled"} (restart desktop to apply)`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  const toggle = async (name: string, on: boolean) => {
    setError(null);
    setFeedback(null);
    try {
      await ipc.mcpToggle(name, on);
      await refetch();
      setFeedback(`${name} ${on ? "enabled" : "disabled"} (restart desktop to apply)`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>MCP servers</h3>
      <p class="settings-hint">
        Model Context Protocol servers (Claude-Desktop-compatible).
        Configured in <code>~/.aictl/mcp.json</code>; spawned at startup
        when the master switch is on.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <BoolRow
        label="MCP subsystem enabled"
        help="Master switch — third-party MCP servers run as child processes, so they're opt-in."
        on={status()?.enabled ?? false}
        onChange={(v) => void setEnabled(v)}
      />
      <Show when={status()}>
        {(s) => (
          <p class="settings-meta">
            Config: <code>{s().config_path}</code>
            {s().config_exists ? "" : " (file does not exist yet)"}
          </p>
        )}
      </Show>
      <Show
        when={(status()?.servers ?? []).length > 0}
        fallback={
          <p class="settings-hint">
            <em>No servers configured. Add entries to mcp.json.</em>
          </p>
        }
      >
        <table class="settings-keys-table">
          <thead>
            <tr>
              <th>Server</th>
              <th>Command</th>
              <th>Tools</th>
              <th>State</th>
              <th />
            </tr>
          </thead>
          <tbody>
            <For each={status()?.servers ?? []}>
              {(row) => (
                <tr>
                  <td>
                    <code>{row.name}</code>
                  </td>
                  <td>
                    <code>{row.command}</code>
                  </td>
                  <td>{row.tool_count}</td>
                  <td>
                    <span data-status={row.state}>{row.state}</span>
                    <Show when={row.state_detail}>
                      {(d) => (
                        <div class="settings-meta" title={d()}>
                          {d()}
                        </div>
                      )}
                    </Show>
                  </td>
                  <td class="settings-keys-actions">
                    <button
                      type="button"
                      class="ghost mini"
                      onClick={() => void toggle(row.name, !row.enabled)}
                    >
                      {row.enabled ? "Disable" : "Enable"}
                    </button>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </div>
  );
};

const HOOK_EVENTS = [
  "SessionStart",
  "SessionEnd",
  "UserPromptSubmit",
  "PreToolUse",
  "PostToolUse",
  "Stop",
  "PreCompact",
  "Notification",
];

const HooksTab: Component = () => {
  const [status, { refetch }] = createResource<HooksStatus>(() =>
    ipc.hooksStatus(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [adding, setAdding] = createSignal(false);
  const [draftEvent, setDraftEvent] = createSignal("PreToolUse");
  const [draftMatcher, setDraftMatcher] = createSignal("*");
  const [draftCommand, setDraftCommand] = createSignal("");
  const [draftTimeout, setDraftTimeout] = createSignal("");

  const toggle = async (row: HookRow) => {
    setError(null);
    try {
      await ipc.hookToggle(row.event, row.idx, !row.enabled);
      await refetch();
    } catch (err) {
      setError(`${err}`);
    }
  };

  const remove = async (row: HookRow) => {
    setError(null);
    try {
      await ipc.hookDelete(row.event, row.idx);
      await refetch();
      setFeedback("hook deleted");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const create = async () => {
    setError(null);
    setFeedback(null);
    try {
      const t = draftTimeout().trim();
      const timeoutSecs = t === "" ? undefined : Number.parseInt(t, 10);
      await ipc.hookCreate(
        draftEvent(),
        draftMatcher(),
        draftCommand(),
        Number.isNaN(timeoutSecs ?? NaN) ? undefined : timeoutSecs,
      );
      setDraftCommand("");
      setDraftMatcher("*");
      setDraftTimeout("");
      setAdding(false);
      await refetch();
      setFeedback("hook added");
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Hooks</h3>
      <p class="settings-hint">
        Lifecycle hooks run shell commands on events like
        <code> PreToolUse</code> or <code>Stop</code>. Configured in{" "}
        <code>~/.aictl/hooks.json</code>; <code>--unrestricted</code>{" "}
        does not bypass them.
      </p>
      <Show when={status()?.config_path}>
        {(p) => (
          <p class="settings-meta">
            Config: <code>{p()}</code>
          </p>
        )}
      </Show>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <div class="settings-actions">
        <button type="button" onClick={() => setAdding((v) => !v)}>
          {adding() ? "Cancel" : "Add hook"}
        </button>
      </div>
      <Show when={adding()}>
        <div class="settings-row settings-row-stack">
          <label>New hook</label>
          <div class="settings-control-line">
            <select
              class="settings-select"
              value={draftEvent()}
              onChange={(e) => setDraftEvent(e.currentTarget.value)}
            >
              <For each={HOOK_EVENTS}>
                {(ev) => <option value={ev}>{ev}</option>}
              </For>
            </select>
            <input
              type="text"
              class="settings-text-input"
              placeholder="matcher (e.g. exec_shell, edit_file|write_file, *)"
              value={draftMatcher()}
              onInput={(e) => setDraftMatcher(e.currentTarget.value)}
            />
          </div>
          <textarea
            class="settings-textarea"
            rows={3}
            placeholder="shell command — receives a JSON payload on stdin"
            value={draftCommand()}
            onInput={(e) => setDraftCommand(e.currentTarget.value)}
          />
          <div class="settings-control-line">
            <input
              type="number"
              class="settings-num-input"
              placeholder="60"
              value={draftTimeout()}
              onInput={(e) => setDraftTimeout(e.currentTarget.value)}
            />
            <span class="settings-suffix">s timeout</span>
            <button type="button" onClick={() => void create()}>
              Save
            </button>
          </div>
        </div>
      </Show>
      <Show
        when={(status()?.hooks ?? []).length > 0}
        fallback={
          <p class="settings-hint">
            <em>No hooks defined.</em>
          </p>
        }
      >
        <table class="settings-keys-table">
          <thead>
            <tr>
              <th>Event</th>
              <th>Matcher</th>
              <th>Command</th>
              <th>Timeout</th>
              <th>State</th>
              <th />
            </tr>
          </thead>
          <tbody>
            <For each={status()?.hooks ?? []}>
              {(row) => (
                <tr>
                  <td>
                    <code>{row.event}</code>
                  </td>
                  <td>
                    <code>{row.matcher}</code>
                  </td>
                  <td>
                    <code class="settings-cmd">{row.command}</code>
                  </td>
                  <td>{row.timeout_secs}s</td>
                  <td>
                    <span data-status={row.enabled ? "ok" : "unset"}>
                      {row.enabled ? "enabled" : "disabled"}
                    </span>
                  </td>
                  <td class="settings-keys-actions">
                    <button
                      type="button"
                      class="ghost mini"
                      onClick={() => void toggle(row)}
                    >
                      {row.enabled ? "Disable" : "Enable"}
                    </button>
                    <button
                      type="button"
                      class="ghost mini danger"
                      onClick={() => void remove(row)}
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </div>
  );
};

interface ViewerState {
  title: string;
  origin: string;
  path: string;
  body: string;
  raw: string;
}

const PromptViewer: Component<{
  view: ViewerState;
  onClose: () => void;
}> = (props) => {
  const [mode, setMode] = createSignal<"rendered" | "source">("rendered");
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
  return (
    <div class="prompt-viewer-overlay" role="dialog" aria-modal="true">
      <div class="prompt-viewer">
        <header class="prompt-viewer-header">
          <div>
            <h3>{props.view.title}</h3>
            <p class="settings-meta">
              {props.view.origin} · <code>{props.view.path}</code>
            </p>
          </div>
          <div class="prompt-viewer-actions">
            <button
              type="button"
              class="prompt-viewer-toggle"
              data-active={String(mode() === "rendered")}
              onClick={() => setMode("rendered")}
            >
              Rendered
            </button>
            <button
              type="button"
              class="prompt-viewer-toggle"
              data-active={String(mode() === "source")}
              onClick={() => setMode("source")}
            >
              Source
            </button>
            <button
              type="button"
              class="settings-close"
              aria-label="Close viewer"
              title="Close (Esc)"
              onClick={props.onClose}
            >
              ✕
            </button>
          </div>
        </header>
        <div class="prompt-viewer-body">
          <Show
            when={mode() === "rendered"}
            fallback={<pre class="prompt-viewer-source">{props.view.raw}</pre>}
          >
            <div
              class="prompt-viewer-rendered chat-markdown"
              innerHTML={renderMarkdown(props.view.body)}
            />
          </Show>
        </div>
      </div>
    </div>
  );
};

const SkillsTab: Component = () => {
  // Plain signal instead of createResource — Delete needs a synchronous
  // optimistic update, and a still-in-flight initial fetch from
  // createResource can resolve *after* the delete and revive the row,
  // forcing the user to click twice. Owning the list outright avoids
  // that race entirely.
  const [skills, setSkills] = createSignal<SkillRow[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [viewer, setViewer] = createSignal<ViewerState | null>(null);

  const load = async () => {
    try {
      setSkills(await ipc.skillsList());
      setError(null);
    } catch (err) {
      setError(`${err}`);
    }
  };
  void load();

  const remove = async (row: SkillRow) => {
    setError(null);
    const previous = skills();
    // Optimistic removal — the row vanishes the moment the click lands.
    setSkills(
      previous.filter((s) => !(s.name === row.name && s.origin === row.origin)),
    );
    try {
      await ipc.skillDelete(row.name, row.origin);
      setFeedback(`deleted ${row.name}`);
    } catch (err) {
      setSkills(previous);
      setError(`${err}`);
    }
  };

  const view = async (row: SkillRow) => {
    setError(null);
    try {
      const v: SkillView = await ipc.skillView(row.name, row.origin);
      setViewer({
        title: v.name,
        origin: v.origin,
        path: v.path,
        body: v.body,
        raw: v.raw,
      });
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Skills</h3>
      <p class="settings-hint">
        One-turn markdown playbooks invoked via{" "}
        <code>/&lt;skill&gt;</code>. Stored under{" "}
        <code>~/.aictl/skills/&lt;name&gt;/SKILL.md</code> (or
        per-project <code>.aictl/skills/</code>).
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <Show
        when={(skills() ?? []).length > 0}
        fallback={
          <p class="settings-hint">
            <em>No skills installed.</em>
          </p>
        }
      >
        <table class="settings-keys-table settings-catalogue-table">
          <colgroup>
            <col />
            <col class="settings-catalogue-origin-col" />
            <col class="settings-catalogue-actions-col" />
          </colgroup>
          <thead>
            <tr>
              <th>Skill</th>
              <th>Origin</th>
              <th class="settings-actions-col" />
            </tr>
          </thead>
          <tbody>
            <For each={skills() ?? []}>
              {(row) => (
                <tr>
                  <td>
                    <div class="settings-name-cell">
                      <code>{row.name}</code>
                      <Show when={row.official}>
                        <span class="badge">official</span>
                      </Show>
                    </div>
                  </td>
                  <td>{row.origin}</td>
                  <td class="settings-actions-col">
                    <div class="settings-keys-actions">
                      <button
                        type="button"
                        class="ghost mini"
                        onClick={() => void view(row)}
                      >
                        View
                      </button>
                      <button
                        type="button"
                        class="ghost mini danger"
                        onClick={() => void remove(row)}
                      >
                        Delete
                      </button>
                    </div>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
      <Show when={viewer()}>
        {(v) => <PromptViewer view={v()} onClose={() => setViewer(null)} />}
      </Show>
    </div>
  );
};

const AgentsTab: Component = () => {
  // See SkillsTab — plain signal to dodge the createResource race that
  // otherwise re-introduces a deleted row when the initial fetch settles
  // after the optimistic mutate.
  const [agents, setAgents] = createSignal<AgentRow[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [viewer, setViewer] = createSignal<ViewerState | null>(null);

  const load = async () => {
    try {
      setAgents(await ipc.agentsList());
      setError(null);
    } catch (err) {
      setError(`${err}`);
    }
  };
  void load();

  const remove = async (row: AgentRow) => {
    setError(null);
    const previous = agents();
    setAgents(
      previous.filter((a) => !(a.name === row.name && a.origin === row.origin)),
    );
    try {
      await ipc.agentDelete(row.name, row.origin);
      setFeedback(`deleted ${row.name}`);
    } catch (err) {
      setAgents(previous);
      setError(`${err}`);
    }
  };

  const view = async (row: AgentRow) => {
    setError(null);
    try {
      const v: AgentView = await ipc.agentView(row.name, row.origin);
      setViewer({
        title: v.name,
        origin: v.origin,
        path: v.path,
        body: v.body,
        raw: v.raw,
      });
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Agents</h3>
      <p class="settings-hint">
        Persistent system-prompt overlays loaded via{" "}
        <code>--agent</code> or the CLI's <code>/agent</code>. Stored at{" "}
        <code>~/.aictl/agents/&lt;name&gt;.md</code>.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <Show
        when={(agents() ?? []).length > 0}
        fallback={
          <p class="settings-hint">
            <em>No agents installed.</em>
          </p>
        }
      >
        <table class="settings-keys-table settings-catalogue-table">
          <colgroup>
            <col />
            <col class="settings-catalogue-origin-col" />
            <col class="settings-catalogue-actions-col" />
          </colgroup>
          <thead>
            <tr>
              <th>Agent</th>
              <th>Origin</th>
              <th class="settings-actions-col" />
            </tr>
          </thead>
          <tbody>
            <For each={agents() ?? []}>
              {(row) => (
                <tr>
                  <td>
                    <div class="settings-name-cell">
                      <code>{row.name}</code>
                      <Show when={row.official}>
                        <span class="badge">official</span>
                      </Show>
                    </div>
                  </td>
                  <td>{row.origin}</td>
                  <td class="settings-actions-col">
                    <div class="settings-keys-actions">
                      <button
                        type="button"
                        class="ghost mini"
                        onClick={() => void view(row)}
                      >
                        View
                      </button>
                      <button
                        type="button"
                        class="ghost mini danger"
                        onClick={() => void remove(row)}
                      >
                        Delete
                      </button>
                    </div>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
      <Show when={viewer()}>
        {(v) => <PromptViewer view={v()} onClose={() => setViewer(null)} />}
      </Show>
    </div>
  );
};

const PluginsTab: Component = () => {
  const [status, { refetch }] = createResource<PluginsStatus>(() =>
    ipc.pluginsStatus(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const setEnabled = async (on: boolean) => {
    setError(null);
    setFeedback(null);
    try {
      if (on) {
        await ipc.configWrite("AICTL_PLUGINS_ENABLED", "true");
      } else {
        await ipc.configClear("AICTL_PLUGINS_ENABLED");
      }
      await refetch();
      setFeedback(
        `plugins ${on ? "enabled" : "disabled"} (restart desktop to apply)`,
      );
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Plugins</h3>
      <p class="settings-hint">
        User-installed tool plugins. Each plugin lives at{" "}
        <code>~/.aictl/plugins/&lt;name&gt;/</code> with a{" "}
        <code>plugin.toml</code> manifest and an entrypoint executable.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <BoolRow
        label="Plugin subsystem enabled"
        help="Master switch — plugins execute third-party code, so they must be opted in."
        on={status()?.enabled ?? false}
        onChange={(v) => void setEnabled(v)}
      />
      <Show when={status()}>
        {(s) => (
          <p class="settings-meta">
            Plugins dir: <code>{s().plugins_dir}</code>
          </p>
        )}
      </Show>
      <Show
        when={(status()?.plugins ?? []).length > 0}
        fallback={
          <p class="settings-hint">
            <em>No plugins installed.</em>
          </p>
        }
      >
        <table class="settings-keys-table">
          <thead>
            <tr>
              <th>Plugin</th>
              <th>Description</th>
              <th>Entrypoint</th>
              <th>Confirm?</th>
              <th>Timeout</th>
            </tr>
          </thead>
          <tbody>
            <For each={status()?.plugins ?? []}>
              {(row) => (
                <tr>
                  <td>
                    <code>{row.name}</code>
                  </td>
                  <td class="settings-desc">{row.description}</td>
                  <td>
                    <code>{row.entrypoint}</code>
                  </td>
                  <td>{row.requires_confirmation ? "yes" : "no"}</td>
                  <td>
                    {row.timeout_secs !== null ? `${row.timeout_secs}s` : "—"}
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </div>
  );
};

const SessionsTab: Component = () => {
  const [rows, { refetch }] = createResource<SessionRow[]>(() =>
    ipc.listSessions(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const remove = async (id: string) => {
    setError(null);
    try {
      await ipc.deleteSession(id);
      await refetch();
      setFeedback("session deleted");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const clearAll = async () => {
    setError(null);
    try {
      await ipc.clearSessions();
      await refetch();
      setFeedback("all sessions cleared");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const newIncognito = async () => {
    setError(null);
    try {
      await ipc.newIncognitoSession();
      setFeedback("started incognito session");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const fmtSize = (n: number) => {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / (1024 * 1024)).toFixed(2)} MB`;
  };

  const fmtAge = (secs: number) => {
    const now = Math.floor(Date.now() / 1000);
    const age = Math.max(0, now - secs);
    if (age < 60) return `${age}s ago`;
    if (age < 3600) return `${Math.floor(age / 60)}m ago`;
    if (age < 86400) return `${Math.floor(age / 3600)}h ago`;
    return `${Math.floor(age / 86400)}d ago`;
  };

  return (
    <div class="settings-tab-content">
      <h3>Sessions</h3>
      <p class="settings-hint">
        Saved conversations under <code>~/.aictl/sessions/</code>. The
        sidebar drives the same list — this view adds bulk-clear and
        an incognito toggle.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <div class="settings-actions">
        <button type="button" onClick={() => void newIncognito()}>
          Start incognito session
        </button>
        <button type="button" onClick={() => void clearAll()}>
          Delete all sessions
        </button>
      </div>
      <Show
        when={(rows() ?? []).length > 0}
        fallback={
          <p class="settings-hint">
            <em>No saved sessions.</em>
          </p>
        }
      >
        <table class="settings-keys-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Id</th>
              <th>Size</th>
              <th>Modified</th>
              <th />
            </tr>
          </thead>
          <tbody>
            <For each={rows() ?? []}>
              {(row) => (
                <tr>
                  <td>{row.name ?? <em>unnamed</em>}</td>
                  <td>
                    <code>{row.id}</code>
                    <Show when={row.active}> <span class="badge">active</span></Show>
                  </td>
                  <td>{fmtSize(row.size)}</td>
                  <td>{fmtAge(row.modified_secs)}</td>
                  <td class="settings-keys-actions">
                    <button
                      type="button"
                      class="ghost mini danger"
                      onClick={() => void remove(row.id)}
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </div>
  );
};

const ContextTab: Component = () => {
  const [ctx, { refetch }] = createResource<ContextStatus>(() =>
    ipc.contextStatus(),
  );
  const [error, setError] = createSignal<string | null>(null);

  const refresh = async () => {
    setError(null);
    try {
      await refetch();
    } catch (err) {
      setError(`${err}`);
    }
  };

  // Bar tone tracks the same thresholds the CLI's `/context` paints
  // with: green under 50%, yellow 50–79%, red above. Keeps the desktop
  // and terminal at-a-glance summaries identical.
  const tone = (pct: number): "ok" | "warn" | "danger" => {
    if (pct >= 80) return "danger";
    if (pct >= 50) return "warn";
    return "ok";
  };

  const fmt = (n: number) => n.toLocaleString();

  return (
    <div class="settings-tab-content">
      <h3>Context</h3>
      <p class="settings-hint">
        Live state of the active conversation: how full the model's
        context window is, how many messages have piled up, and where
        the auto-compact threshold sits. Mirrors the CLI's{" "}
        <code>/context</code>.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={ctx()}>
        {(c) => (
          <>
            <div class="settings-row settings-row-stack">
              <label>Active model</label>
              <div class="settings-value">
                <Show
                  when={c().model}
                  fallback={
                    <span class="settings-empty">
                      No model selected — pick one in the Model tab.
                    </span>
                  }
                >
                  <code>
                    {c().provider ?? "?"} · {c().model}
                  </code>
                </Show>
              </div>
            </div>
            <div class="settings-row settings-row-stack">
              <label>Context window</label>
              <div class="settings-context-bar">
                <div
                  class="settings-context-fill"
                  data-tone={tone(c().context_pct)}
                  style={{ width: `${Math.min(c().context_pct, 100)}%` }}
                />
              </div>
              <p class="settings-meta">
                {c().context_pct}% used — token usage{" "}
                {c().token_pct}% · message buffer {c().message_pct}%
              </p>
            </div>
            <div class="settings-row">
              <label>Last input tokens</label>
              <div class="settings-value">
                <code>
                  {fmt(c().last_input_tokens)} / {fmt(c().context_limit)}
                </code>
              </div>
            </div>
            <div class="settings-row">
              <label>Last output tokens</label>
              <div class="settings-value">
                <code>{fmt(c().last_output_tokens)}</code>
              </div>
            </div>
            <div class="settings-row">
              <label>Messages</label>
              <div class="settings-value">
                <code>
                  {c().messages} / {c().max_messages}
                </code>
              </div>
            </div>
            <div class="settings-row">
              <label>Auto-compact at</label>
              <div class="settings-value">
                <code>{c().auto_compact_threshold}%</code>{" "}
                <span class="settings-meta">
                  ({c().auto_compact_overridden ? "overridden" : "default"})
                </span>
              </div>
            </div>
            <Show when={c().last_input_tokens === 0}>
              <p class="settings-hint">
                <em>
                  No turns recorded yet — token counts populate after the
                  first model response.
                </em>
              </p>
            </Show>
          </>
        )}
      </Show>
      <div class="settings-actions">
        <button type="button" onClick={() => void refresh()}>
          Refresh
        </button>
      </div>
    </div>
  );
};

const StatsTab: Component = () => {
  const [snap, { refetch }] = createResource<StatsSnapshot>(() =>
    ipc.statsSnapshot(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const clear = async () => {
    setError(null);
    try {
      await ipc.statsClear();
      await refetch();
      setFeedback("stats cleared");
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Stats</h3>
      <p class="settings-hint">
        Daily counts of sessions, tool calls, tokens, and estimated
        cost. Stored under <code>~/.aictl/stats/</code> by both the
        desktop and the CLI.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <Show when={snap()}>
        {(s) => (
          <>
            <p class="settings-meta">
              {s().day_count} day file{s().day_count === 1 ? "" : "s"} on
              disk.
            </p>
            <div class="settings-stats-grid">
              <BucketCard label="Today" bucket={s().today} />
              <BucketCard label="This month" bucket={s().month} />
              <BucketCard label="Overall" bucket={s().overall} />
            </div>
          </>
        )}
      </Show>
      <div class="settings-actions">
        <button type="button" onClick={() => void clear()}>
          Clear all stats
        </button>
      </div>
    </div>
  );
};

const BucketCard: Component<{ label: string; bucket: StatsBucket }> = (props) => (
  <div class="settings-stats-card">
    <h4>{props.label}</h4>
    <dl>
      <dt>Sessions</dt>
      <dd>{props.bucket.sessions}</dd>
      <dt>Requests</dt>
      <dd>{props.bucket.requests}</dd>
      <dt>LLM calls</dt>
      <dd>{props.bucket.llm_calls}</dd>
      <dt>Tool calls</dt>
      <dd>{props.bucket.tool_calls}</dd>
      <dt>Input tokens</dt>
      <dd>{props.bucket.input_tokens.toLocaleString()}</dd>
      <dt>Output tokens</dt>
      <dd>{props.bucket.output_tokens.toLocaleString()}</dd>
      <dt>Cost (USD)</dt>
      <dd>${props.bucket.cost_usd.toFixed(4)}</dd>
    </dl>
    <Show when={props.bucket.models.length > 0}>
      <h5>Top models</h5>
      <ul>
        <For each={props.bucket.models.slice(0, 5)}>
          {(m) => (
            <li>
              <code>{m.model}</code> · {m.count}
            </li>
          )}
        </For>
      </ul>
    </Show>
  </div>
);

const REDACTION_DETECTORS = [
  { slug: "api_key", label: "API keys" },
  { slug: "aws", label: "AWS keys" },
  { slug: "jwt", label: "JWTs" },
  { slug: "private_key", label: "Private keys (PEM)" },
  { slug: "connection_string", label: "Connection strings" },
  { slug: "credit_card", label: "Credit cards" },
  { slug: "iban", label: "IBAN" },
  { slug: "email", label: "Emails" },
  { slug: "phone", label: "Phone numbers" },
  { slug: "high_entropy", label: "High-entropy strings" },
  { slug: "person_name", label: "Person names (NER)" },
  { slug: "location", label: "Locations (NER)" },
  { slug: "organization", label: "Organizations (NER)" },
];

const RedactionTab: Component = () => {
  const [config, { refetch }] = createResource<ConfigEntry[]>(() =>
    ipc.configDump(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const get = (key: string): string => {
    const entry = (config() ?? []).find((e) => e.key === key);
    return entry?.value ?? "";
  };

  const setConfig = async (key: string, value: string) => {
    setError(null);
    setFeedback(null);
    try {
      if (value.trim() === "") {
        await ipc.configClear(key);
      } else {
        await ipc.configWrite(key, value);
      }
      await refetch();
      setFeedback(`${key} updated`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  const detectorsRaw = () => get("AICTL_REDACTION_DETECTORS");
  const enabledSet = createMemo(() => {
    const raw = detectorsRaw();
    if (raw.trim() === "") {
      return new Set(REDACTION_DETECTORS.map((d) => d.slug));
    }
    return new Set(
      raw
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean),
    );
  });

  const toggleDetector = async (slug: string, on: boolean) => {
    const next = new Set(enabledSet());
    if (on) next.add(slug);
    else next.delete(slug);
    if (next.size === REDACTION_DETECTORS.length) {
      await setConfig("AICTL_REDACTION_DETECTORS", "");
    } else {
      await setConfig("AICTL_REDACTION_DETECTORS", Array.from(next).join(","));
    }
  };

  const isOn = (key: string): boolean => {
    const v = get(key);
    if (v === "") return key !== "AICTL_REDACTION_NER";
    return v !== "false" && v !== "0";
  };

  return (
    <div class="settings-tab-content">
      <h3>Redaction</h3>
      <p class="settings-hint">
        Strip secrets from outbound LLM payloads. The master switch is
        in General → Security; this tab tunes which detectors fire and
        adds project-specific allow/deny patterns.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>

      <h4 class="settings-subhead">Built-in detectors</h4>
      <p class="settings-hint">
        All detectors are on by default. Unchecking a row removes it
        from <code>AICTL_REDACTION_DETECTORS</code>.
      </p>
      <ul class="settings-tools-list">
        <For each={REDACTION_DETECTORS}>
          {(d) => (
            <li>
              <label class="settings-tool-item">
                <input
                  type="checkbox"
                  checked={enabledSet().has(d.slug)}
                  onChange={(e) =>
                    void toggleDetector(d.slug, e.currentTarget.checked)
                  }
                />
                <span class="settings-tool-name">
                  <code>{d.slug}</code>
                </span>
                <span class="settings-tool-desc">{d.label}</span>
              </label>
            </li>
          )}
        </For>
      </ul>

      <h4 class="settings-subhead">Custom patterns</h4>
      <TextRow
        label="Extra patterns"
        help="Comma-separated NAME=REGEX pairs. e.g. INTERNAL_TOKEN=tok_[a-zA-Z0-9]{16}"
        initial={get("AICTL_REDACTION_EXTRA_PATTERNS")}
        placeholder=""
        onCommit={(v) => void setConfig("AICTL_REDACTION_EXTRA_PATTERNS", v)}
      />
      <TextRow
        label="Allow-list patterns"
        help="Comma-separated regexes whose matches override any detector hit. Useful for test fixtures or known-safe placeholders."
        initial={get("AICTL_REDACTION_ALLOW")}
        placeholder=""
        onCommit={(v) => void setConfig("AICTL_REDACTION_ALLOW", v)}
      />

      <h4 class="settings-subhead">NER pass</h4>
      <BoolRow
        label="Enable NER (people, locations, organizations)"
        help="Requires a build with the redaction-ner cargo feature and the gline-rs model on disk. Off by default."
        on={isOn("AICTL_REDACTION_NER")}
        onChange={(v) =>
          void setConfig("AICTL_REDACTION_NER", v ? "true" : "")
        }
      />
    </div>
  );
};

const ShellTab: Component = () => {
  const [config, { refetch }] = createResource<ConfigEntry[]>(() =>
    ipc.configDump(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const get = (key: string): string => {
    const entry = (config() ?? []).find((e) => e.key === key);
    return entry?.value ?? "";
  };

  const setConfig = async (key: string, value: string) => {
    setError(null);
    setFeedback(null);
    try {
      if (value.trim() === "") {
        await ipc.configClear(key);
      } else {
        await ipc.configWrite(key, value);
      }
      await refetch();
      setFeedback(`${key} updated`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Shell &amp; limits</h3>
      <p class="settings-hint">
        Fine-grained controls over what shell commands the agent can
        invoke and how big a single tool result can grow. CLI's{" "}
        <code>/security</code> reads the same keys.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>

      <h4 class="settings-subhead">Shell allow/block</h4>
      <TextRow
        label="Allowed binaries"
        help="Comma-separated. When non-empty, only these binaries can be invoked. Leave blank to allow everything not on the block list."
        initial={get("AICTL_SECURITY_SHELL_ALLOWED")}
        placeholder=""
        onCommit={(v) => void setConfig("AICTL_SECURITY_SHELL_ALLOWED", v)}
      />
      <TextRow
        label="Additionally blocked binaries"
        help="Comma-separated. Adds to the built-in block list (rm -rf, sudo, etc.)."
        initial={get("AICTL_SECURITY_SHELL_BLOCKED")}
        placeholder=""
        onCommit={(v) => void setConfig("AICTL_SECURITY_SHELL_BLOCKED", v)}
      />

      <h4 class="settings-subhead">Path policy</h4>
      <TextRow
        label="Additionally blocked paths"
        help="Comma-separated absolute paths (or ~/relative). Adds to the built-in block list."
        initial={get("AICTL_SECURITY_BLOCKED_PATHS")}
        placeholder=""
        onCommit={(v) => void setConfig("AICTL_SECURITY_BLOCKED_PATHS", v)}
      />
      <TextRow
        label="Allowed paths"
        help="When non-empty, file-system tools may only touch paths under one of these prefixes."
        initial={get("AICTL_SECURITY_ALLOWED_PATHS")}
        placeholder=""
        onCommit={(v) => void setConfig("AICTL_SECURITY_ALLOWED_PATHS", v)}
      />
      <TextRow
        label="Blocked env vars"
        help="Comma-separated env-var names that are scrubbed from every tool subprocess."
        initial={get("AICTL_SECURITY_BLOCKED_ENV")}
        placeholder=""
        onCommit={(v) => void setConfig("AICTL_SECURITY_BLOCKED_ENV", v)}
      />

      <h4 class="settings-subhead">Limits</h4>
      <NumberRow
        label="Shell timeout"
        help="Per-command shell-tool timeout in seconds. Leave blank for the default."
        suffix="s"
        initial={get("AICTL_SECURITY_SHELL_TIMEOUT")}
        placeholder="30"
        onCommit={(v) => void setConfig("AICTL_SECURITY_SHELL_TIMEOUT", v)}
      />
      <NumberRow
        label="Max file write"
        help="Cap on the byte size of a single write_file / edit_file call."
        suffix="B"
        initial={get("AICTL_SECURITY_MAX_WRITE")}
        placeholder=""
        onCommit={(v) => void setConfig("AICTL_SECURITY_MAX_WRITE", v)}
      />
    </div>
  );
};

const AppearanceTab: Component = () => {
  const [config, { refetch }] = createResource<ConfigEntry[]>(() =>
    ipc.configDump(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const get = (key: string): string => {
    const entry = (config() ?? []).find((e) => e.key === key);
    return entry?.value ?? "";
  };

  const setConfig = async (key: string, value: string) => {
    setError(null);
    setFeedback(null);
    try {
      if (value.trim() === "") {
        await ipc.configClear(key);
      } else {
        await ipc.configWrite(key, value);
      }
      await refetch();
      applyAppearance({
        theme: key === "AICTL_DESKTOP_THEME" ? value : get("AICTL_DESKTOP_THEME"),
        density:
          key === "AICTL_DESKTOP_DENSITY" ? value : get("AICTL_DESKTOP_DENSITY"),
      });
      setFeedback(`${key} updated`);
    } catch (err) {
      setError(`${err}`);
    }
  };

  const theme = (): string => get("AICTL_DESKTOP_THEME") || "dark";
  const density = (): string => get("AICTL_DESKTOP_DENSITY") || "comfortable";
  const notifications = (): boolean => {
    const v = get("AICTL_DESKTOP_NOTIFICATIONS");
    return v !== "false" && v !== "0";
  };

  return (
    <div class="settings-tab-content">
      <h3>Appearance</h3>
      <p class="settings-hint">
        Desktop-only knobs. Stored under{" "}
        <code>AICTL_DESKTOP_*</code> so the CLI ignores them.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>

      <h4 class="settings-subhead">Theme</h4>
      <div class="settings-row settings-row-stack">
        <label>Color scheme</label>
        <div class="settings-control-line">
          <select
            class="settings-select"
            value={theme()}
            onChange={(e) =>
              void setConfig("AICTL_DESKTOP_THEME", e.currentTarget.value)
            }
          >
            <option value="dark">Dark</option>
            <option value="light">Light</option>
            <option value="system">Follow system</option>
          </select>
        </div>
        <p class="settings-hint">
          Light theme is a higher-contrast variant of the brutalist palette.
        </p>
      </div>

      <h4 class="settings-subhead">Density</h4>
      <div class="settings-row settings-row-stack">
        <label>Chat density</label>
        <div class="settings-control-line">
          <select
            class="settings-select"
            value={density()}
            onChange={(e) =>
              void setConfig("AICTL_DESKTOP_DENSITY", e.currentTarget.value)
            }
          >
            <option value="comfortable">Comfortable</option>
            <option value="compact">Compact</option>
            <option value="cozy">Cozy</option>
          </select>
        </div>
        <p class="settings-hint">
          Adjusts message padding and font scale across the chat.
        </p>
      </div>

      <h4 class="settings-subhead">Notifications</h4>
      <BoolRow
        label="Notify when a long response finishes"
        help="Fires a browser notification when the window is unfocused and an agent turn completes."
        on={notifications()}
        onChange={(v) =>
          void setConfig("AICTL_DESKTOP_NOTIFICATIONS", v ? "true" : "")
        }
      />
    </div>
  );
};

interface AppearanceState {
  theme: string;
  density: string;
}

/// Apply theme + density tokens to the root element so the change
/// takes effect immediately. Mirrors the side-effect performed at
/// boot in main.tsx.
export function applyAppearance(s: AppearanceState) {
  const theme = (s.theme || "dark").toLowerCase();
  const density = (s.density || "comfortable").toLowerCase();
  const root = document.documentElement;
  if (theme === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", theme);
  }
  root.setAttribute("data-density", density);
}

const AboutTab: Component = () => {
  const [version] = createResource<string>(() => ipc.version());
  const [error, setError] = createSignal<string | null>(null);
  const reveal = async (kind: "audit" | "config") => {
    setError(null);
    try {
      if (kind === "audit") await ipc.revealAuditLog();
      else await ipc.revealConfigDir();
    } catch (err) {
      setError(`${err}`);
    }
  };
  const open = async (url: string) => {
    setError(null);
    try {
      await ipc.openUrl(url);
    } catch (err) {
      setError(`${err}`);
    }
  };
  return (
    <div class="settings-tab-content">
      <h3>About</h3>
      <div class="settings-row">
        <label>Version</label>
        <div class="settings-value">
          <code>{version() ?? "…"}</code>
        </div>
      </div>
      <div class="settings-row">
        <label>Website</label>
        <div class="settings-value">
          <a href="#" onClick={(e) => { e.preventDefault(); void open("https://aictl.app"); }}>
            aictl.app
          </a>
        </div>
      </div>
      <div class="settings-row">
        <label>Source</label>
        <div class="settings-value">
          <a href="#" onClick={(e) => { e.preventDefault(); void open("https://github.com/pwittchen/aictl"); }}>
            github.com/pwittchen/aictl
          </a>
        </div>
      </div>
      <div class="settings-row">
        <label>Developer</label>
        <div class="settings-value">
          Piotr Wittchen —{" "}
          <a href="#" onClick={(e) => { e.preventDefault(); void open("https://wittchen.io"); }}>
            wittchen.io
          </a>
        </div>
      </div>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <div class="settings-actions">
        <button type="button" onClick={() => void reveal("config")}>
          Reveal config in Finder
        </button>
        <button type="button" onClick={() => void reveal("audit")}>
          Reveal audit log
        </button>
      </div>
    </div>
  );
};

export default Settings;

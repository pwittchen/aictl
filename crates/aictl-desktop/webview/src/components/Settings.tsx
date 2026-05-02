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
  type ConfigEntry,
  type KeyBackend,
  type KeyRow,
  type ModelEntry,
  type WorkspaceState,
} from "../lib/ipc";

interface Props {
  workspace: WorkspaceState;
  onPickWorkspace: () => void | Promise<void>;
  onClose: () => void;
  models: ModelEntry[];
  activeModel: ActiveModel;
  onChangeModel: (provider: string, model: string) => Promise<void>;
}

type Tab = "workspace" | "provider" | "keys" | "general" | "about";

const TABS: { id: Tab; label: string }[] = [
  { id: "workspace", label: "Workspace" },
  { id: "provider", label: "Provider & Model" },
  { id: "keys", label: "API Keys" },
  { id: "general", label: "General" },
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
  const [tab, setTab] = createSignal<Tab>("workspace");

  // Esc closes the overlay. The composer's own listeners are inactive
  // while the chat is masked behind the modal, so we don't need to
  // worry about double-handling.
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
            <Show when={tab() === "workspace"}>
              <WorkspaceTab
                workspace={props.workspace}
                onPick={props.onPickWorkspace}
              />
            </Show>
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
              <GeneralTab />
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

const WorkspaceTab: Component<{
  workspace: WorkspaceState;
  onPick: () => void | Promise<void>;
}> = (props) => (
  <div class="settings-tab-content">
    <h3>Workspace</h3>
    <p class="settings-hint">
      The workspace folder is the CWD jail root for every tool call —
      the agent can only read and write files inside it.
    </p>
    <div class="settings-row">
      <label>Current</label>
      <div class="settings-value">
        <Show
          when={props.workspace.path}
          fallback={<span class="settings-empty">No workspace selected</span>}
        >
          <code>{props.workspace.path}</code>
        </Show>
      </div>
    </div>
    <Show when={props.workspace.error}>
      <p class="settings-error">{props.workspace.error}</p>
    </Show>
    <div class="settings-actions">
      <button type="button" onClick={() => void props.onPick()}>
        {props.workspace.path ? "Change workspace…" : "Pick workspace…"}
      </button>
    </div>
  </div>
);

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
      <h3>Provider & Model</h3>
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

  return (
    <div class="settings-tab-content">
      <h3>API Keys</h3>
      <p class="settings-hint">
        Stored in the system keychain when available, otherwise in
        plain <code>~/.aictl/config</code>. Local providers (Ollama,
        GGUF, MLX) don't need keys.
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

const BOOL_KEYS: { key: string; label: string; help: string }[] = [
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
];

const NUM_KEYS: { key: string; label: string; help: string; suffix: string }[] = [
  {
    key: "AICTL_AUTO_COMPACT_THRESHOLD",
    label: "Auto-compact threshold",
    help: "Compact context automatically when usage crosses this percentage. 0 disables.",
    suffix: "%",
  },
  {
    key: "AICTL_LLM_TIMEOUT",
    label: "LLM call timeout",
    help: "Per-request timeout in seconds. 0 disables.",
    suffix: "s",
  },
];

const GeneralTab: Component = () => {
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
    if (v === null) return true; // most security flags default-on
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

      <h4 class="settings-subhead">Numbers</h4>
      <For each={NUM_KEYS}>
        {(spec) => (
          <NumberRow
            label={spec.label}
            help={spec.help}
            suffix={spec.suffix}
            initial={get(spec.key) ?? ""}
            onCommit={(v) => void setNum(spec.key, v)}
          />
        )}
      </For>

      <h4 class="settings-subhead">Security</h4>
      <For each={BOOL_KEYS}>
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

const NumberRow: Component<{
  label: string;
  help: string;
  suffix: string;
  initial: string;
  onCommit: (value: string) => void;
}> = (props) => {
  const [value, setValue] = createSignal(props.initial);
  // Reset when the resource feeds a fresh value (e.g. after refetch).
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

const AboutTab: Component = () => {
  const [version] = createResource<string>(() => ipc.version());
  const reveal = async (kind: "audit" | "config") => {
    try {
      if (kind === "audit") await ipc.revealAuditLog();
      else await ipc.revealConfigDir();
    } catch (err) {
      console.warn(err);
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
        <label>Project</label>
        <div class="settings-value">
          <a href="https://github.com/aictl/aictl">github.com/aictl/aictl</a>
        </div>
      </div>
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

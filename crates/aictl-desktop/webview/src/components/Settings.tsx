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
  type LocalModelsStatus,
  type McpStatus,
  type NerStatus,
  type MemoryRow,
  type MemoryStatus,
  type ModelEntry,
  type OllamaProbeResult,
  type OllamaStatus,
  type PluginsStatus,
  type RemoteCatalogueRow,
  type ServerProbeResult,
  type ServerStatus,
  type SessionRow,
  type SkillRow,
  type SkillView,
  type DailyPoint,
  type StatsBucket,
  type StatsSnapshot,
  type ToolRow,
  type WorkspaceState,
} from "../lib/ipc";
import { renderMarkdown } from "../lib/markdown";
import { checkUpdate, type UpdateInfo } from "../lib/updater";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import AgentEditor from "./AgentEditor";
import ConfirmDelete from "./ConfirmDelete";
import { Dropdown } from "./Dropdown";
import McpEditor from "./McpEditor";
import SkillEditor from "./SkillEditor";

// Tauri's clipboard plugin is the happy path on desktop; the navigator
// fallback covers Vite dev mode where the plugin isn't initialized.
async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await writeText(text);
    return true;
  } catch {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      return false;
    }
  }
}

interface Props {
  workspace: WorkspaceState;
  onPickWorkspace: () => void | Promise<void>;
  onClose: () => void;
  models: ModelEntry[];
  activeModel: ActiveModel;
  onChangeModel: (provider: string, model: string) => Promise<void>;
  /// Called by the Local Models tab after a download finishes so the
  /// app-level catalogue picks up the new entry — keeps the composer
  /// dropdown and the Provider tab in sync.
  onRefreshModels: () => Promise<void>;
  /// Initial tab the panel opens on. Used by the provider-setup dialog
  /// to deep-link straight to API Keys / Local Models / LLM Servers.
  initialTab?: Tab;
  /// Open the in-app update modal. Called from the About tab's
  /// "Install update" button so the modal is mounted at the App level
  /// (and can outlive a Settings close) rather than nested inside this
  /// overlay.
  onShowUpdate?: (info: UpdateInfo | null) => void;
  /// Delete a single session — App-level so the sidebar refresh and
  /// active-session reset semantics match the sidebar's own Delete
  /// button. SessionsTab wires its row-level button through this so
  /// the left panel and the chat pane stay coherent.
  onDeleteSession: (id: string) => Promise<void>;
  /// Bulk-delete every session. Same App-level handler as the sidebar's
  /// Clear-All — guarantees the sidebar refreshes and the active chat
  /// resets when the current session lands in the wipe.
  onClearAllSessions: () => Promise<void>;
  /// Called every time a per-tool enable/disable flips inside the
  /// Tools list, so App can re-derive the composer's globe / picture
  /// icon state in real time instead of waiting for the panel to
  /// close. App passes a function that re-runs `refreshWebEnabled` and
  /// `refreshImageEnabled`; ToolsList fires it after each successful
  /// `toolSetDisabled` round-trip.
  onToolToggled?: () => void;
}

export type Tab =
  | "general"
  | "security"
  | "provider"
  | "keys"
  | "server"
  | "mcp"
  | "memory"
  | "hooks"
  | "skills"
  | "agents"
  | "plugins"
  | "models"
  | "sessions"
  | "context"
  | "stats"
  | "redaction"
  | "shell"
  | "tools"
  | "appearance"
  | "about";

const TABS: { id: Tab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "appearance", label: "Appearance" },
  { id: "provider", label: "Models" },
  { id: "models", label: "Local Models" },
  { id: "keys", label: "API Keys" },
  { id: "security", label: "Security" },
  { id: "redaction", label: "Redaction" },
  { id: "shell", label: "Shell" },
  { id: "server", label: "LLM Servers" },
  { id: "mcp", label: "MCP Servers" },
  { id: "memory", label: "Memory" },
  { id: "hooks", label: "Hooks" },
  { id: "skills", label: "Skills" },
  { id: "agents", label: "Agents" },
  { id: "tools", label: "Tools" },
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
  "aictl-server": "AICTL (self-hosted)",
};

const Settings: Component<Props> = (props) => {
  const [tab, setTab] = createSignal<Tab>(props.initialTab ?? "general");

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
              <KeysTab onRefreshModels={props.onRefreshModels} />
            </Show>
            <Show when={tab() === "general"}>
              <GeneralTab
                workspace={props.workspace}
                onPickWorkspace={props.onPickWorkspace}
              />
            </Show>
            <Show when={tab() === "tools"}>
              <ToolsTab onToolToggled={props.onToolToggled} />
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
            <Show when={tab() === "memory"}>
              <MemoryTab />
            </Show>
            <Show when={tab() === "models"}>
              <ModelsTab onRefreshModels={props.onRefreshModels} />
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
              <SessionsTab
                onDeleteSession={props.onDeleteSession}
                onClearAll={props.onClearAllSessions}
              />
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
              <AboutTab onShowUpdate={props.onShowUpdate} />
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
      <h3>Models</h3>
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

interface KeysTabProps {
  /// Re-pulls the model catalogue from `list_models`. The Composer's
  /// dropdown and the Models tab both filter cloud models by which
  /// API keys are set, so any save / clear / lock / unlock here needs
  /// to refresh that list — otherwise newly-configured providers
  /// would only appear after a window reload.
  onRefreshModels: () => Promise<void>;
}

const KeysTab: Component<KeysTabProps> = (props) => {
  const [rows, { refetch }] = createResource<KeyRow[]>(() => ipc.keysStatus());
  const [backend] = createResource<KeyBackend>(() => ipc.keysBackend());
  const [editing, setEditing] = createSignal<KeyRow | null>(null);
  const [draft, setDraft] = createSignal("");
  const [pendingClear, setPendingClear] = createSignal<KeyRow | null>(null);
  const [pendingLock, setPendingLock] = createSignal<KeyRow | null>(null);
  const [pendingUnlock, setPendingUnlock] = createSignal<KeyRow | null>(null);
  const [pendingLockAll, setPendingLockAll] = createSignal(false);
  const [pendingUnlockAll, setPendingUnlockAll] = createSignal(false);
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
      await props.onRefreshModels();
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
      await props.onRefreshModels();
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
      await props.onRefreshModels();
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
      await props.onRefreshModels();
      setFeedback(
        outcome === "already_unlocked"
          ? `${name} already in config`
          : `${name} → config`,
      );
    } catch (err) {
      setError(`${err}`);
    }
  };

  const lockAll = async () => {
    setError(null);
    setFeedback(null);
    try {
      const r = await ipc.keysLockAll();
      await refetch();
      await props.onRefreshModels();
      const parts: string[] = [];
      parts.push(`${r.migrated} → keyring`);
      if (r.already > 0) parts.push(`${r.already} already locked`);
      if (r.errors.length > 0) parts.push(`${r.errors.length} error(s)`);
      setFeedback(`Lock all: ${parts.join(", ")}`);
      if (r.errors.length > 0) {
        setError(r.errors.map(([n, e]) => `${n}: ${e}`).join("; "));
      }
    } catch (err) {
      setError(`${err}`);
    }
  };

  const unlockAll = async () => {
    setError(null);
    setFeedback(null);
    try {
      const r = await ipc.keysUnlockAll();
      await refetch();
      await props.onRefreshModels();
      const parts: string[] = [];
      parts.push(`${r.migrated} → config`);
      if (r.already > 0) parts.push(`${r.already} already unlocked`);
      if (r.errors.length > 0) parts.push(`${r.errors.length} error(s)`);
      setFeedback(`Unlock all: ${parts.join(", ")}`);
      if (r.errors.length > 0) {
        setError(r.errors.map(([n, e]) => `${n}: ${e}`).join("; "));
      }
    } catch (err) {
      setError(`${err}`);
    }
  };

  const anyLockable = () =>
    (rows() ?? []).some(
      (r) => r.location === "plain" || r.location === "both",
    );
  const anyUnlockable = () =>
    (rows() ?? []).some(
      (r) => r.location === "keyring" || r.location === "both",
    );

  // Aggregate lock state across every key that has a value set. A key
  // counts as locked only when it lives purely in the keyring; `both`
  // (still has a plain copy) and `plain` count as not-locked, since the
  // user still has cleanup to do via "Lock". `unset` rows are ignored
  // so an empty profile reads as "none locked" rather than "all
  // locked".
  const lockState = createMemo<"all" | "partial" | "none">(() => {
    const setRows = (rows() ?? []).filter((r) => r.location !== "unset");
    if (setRows.length === 0) return "none";
    if (setRows.every((r) => r.location === "keyring")) return "all";
    if (setRows.every((r) => r.location === "plain")) return "none";
    return "partial";
  });

  const lockStateTitle = () => {
    switch (lockState()) {
      case "all":
        return "All keys locked in the system keyring";
      case "partial":
        return "Some keys are still in plain config";
      case "none":
        return "No keys are locked in the system keyring";
    }
  };

  // Bucket keys into three groups so the table is split by purpose.
  // LLM provider keys all share the LLM_ prefix on the Tauri side;
  // both AICTL_*_MASTER_KEY entries belong to the self-hosted route
  // (one is the bearer the CLI/desktop sends, the other is the value
  // the server accepts); anything left over (Firecrawl today) lands
  // in "Other".
  const groupedRows = createMemo(() => {
    const llm: KeyRow[] = [];
    const aictl: KeyRow[] = [];
    const other: KeyRow[] = [];
    for (const r of rows() ?? []) {
      if (r.name.startsWith("LLM_")) {
        llm.push(r);
      } else if (
        r.name === "AICTL_CLIENT_MASTER_KEY" ||
        r.name === "AICTL_SERVER_MASTER_KEY"
      ) {
        aictl.push(r);
      } else {
        other.push(r);
      }
    }
    return [
      { title: "LLM API Keys", firstColumn: "Provider", rows: llm },
      { title: "AICTL (self-hosted)", firstColumn: "Type", rows: aictl },
      { title: "Other", firstColumn: "Provider", rows: other },
    ].filter((g) => g.rows.length > 0);
  });

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
      <Show when={backend()?.available}>
        <div class="settings-keys-bulk">
          <button
            type="button"
            disabled={!anyLockable()}
            title="Move every plain-config key into the system keyring"
            onClick={() => {
              setFeedback(null);
              setError(null);
              setPendingLockAll(true);
            }}
          >
            Lock All
          </button>
          <button
            type="button"
            disabled={!anyUnlockable()}
            title="Move every keyring-stored key back into plain config"
            onClick={() => {
              setFeedback(null);
              setError(null);
              setPendingUnlockAll(true);
            }}
          >
            Unlock All
          </button>
          <span
            class="settings-keys-lock-status"
            data-state={lockState()}
            title={lockStateTitle()}
            aria-label={lockStateTitle()}
          >
            <Show
              when={lockState() === "none"}
              fallback={
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  viewBox="0 0 16 16"
                  fill="currentColor"
                  aria-hidden="true"
                >
                  <path
                    fill-rule="evenodd"
                    d="M8 1a3.5 3.5 0 0 0-3.5 3.5V7A1.5 1.5 0 0 0 3 8.5v5A1.5 1.5 0 0 0 4.5 15h7a1.5 1.5 0 0 0 1.5-1.5v-5A1.5 1.5 0 0 0 11.5 7V4.5A3.5 3.5 0 0 0 8 1Zm2 6V4.5a2 2 0 1 0-4 0V7h4Z"
                    clip-rule="evenodd"
                  />
                </svg>
              }
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 16 16"
                fill="currentColor"
                aria-hidden="true"
              >
                <path d="M11.5 1A3.5 3.5 0 0 0 8 4.5V7H2.5A1.5 1.5 0 0 0 1 8.5v5A1.5 1.5 0 0 0 2.5 15h7a1.5 1.5 0 0 0 1.5-1.5v-5A1.5 1.5 0 0 0 9.5 7V4.5a2 2 0 1 1 4 0v1.75a.75.75 0 0 0 1.5 0V4.5A3.5 3.5 0 0 0 11.5 1Z" />
              </svg>
            </Show>
          </span>
        </div>
      </Show>
      <For each={groupedRows()}>
        {(group) => (
          <>
            <h4 class="settings-subhead">{group.title}</h4>
            <table class="settings-keys-table">
              <thead>
                <tr>
                  <th>{group.firstColumn}</th>
                  <th>Key name</th>
                  <th>Status</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                <For each={group.rows}>
                  {(row) => (
                    <tr>
                      <td>{row.label || row.name}</td>
                      <td>
                        <code>{row.name}</code>
                      </td>
                      <td>
                        <span data-status={row.location}>{row.location}</span>
                      </td>
                      <td class="settings-keys-actions-cell">
                        <div class="settings-keys-actions">
                          <button
                            type="button"
                            class="ghost mini"
                            onClick={() => {
                              setEditing(row);
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
                              (row.location === "plain" ||
                                row.location === "both")
                            }
                          >
                            <button
                              type="button"
                              class="ghost mini"
                              title="Move from plain config to system keyring"
                              onClick={() => {
                                setFeedback(null);
                                setError(null);
                                setPendingLock(row);
                              }}
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
                              onClick={() => {
                                setFeedback(null);
                                setError(null);
                                setPendingUnlock(row);
                              }}
                            >
                              Unlock
                            </button>
                          </Show>
                          <Show when={row.location !== "unset"}>
                            <button
                              type="button"
                              class="ghost mini danger"
                              onClick={() => {
                                setFeedback(null);
                                setError(null);
                                setPendingClear(row);
                              }}
                            >
                              Clear
                            </button>
                          </Show>
                        </div>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </>
        )}
      </For>
      <Show when={editing()}>
        {(row) => (
          <KeyEditModal
            row={row()}
            draft={draft()}
            onDraft={setDraft}
            onCancel={() => {
              setEditing(null);
              setDraft("");
            }}
            onSubmit={() => void save(row().name)}
          />
        )}
      </Show>
      <Show when={pendingClear()}>
        {(row) => (
          <ConfirmDelete
            title="Clear API key"
            detail={`${row().label || row().name} (${row().name})`}
            note="The stored value will be removed from both the keyring and plain config. You can set a new key afterwards."
            onCancel={() => setPendingClear(null)}
            onConfirm={() => {
              const target = row();
              setPendingClear(null);
              void remove(target.name);
            }}
          />
        )}
      </Show>
      <Show when={pendingLock()}>
        {(row) => (
          <ConfirmDelete
            title="Lock API key"
            detail={`${row().label || row().name} (${row().name})`}
            note="The value will be moved from plain ~/.aictl/config into the system keyring."
            confirmLabel="Lock"
            confirmVariant="allow"
            onCancel={() => setPendingLock(null)}
            onConfirm={() => {
              const target = row();
              setPendingLock(null);
              void lock(target.name);
            }}
          />
        )}
      </Show>
      <Show when={pendingUnlock()}>
        {(row) => (
          <ConfirmDelete
            title="Unlock API key"
            detail={`${row().label || row().name} (${row().name})`}
            note="The value will be moved from the system keyring back into plain ~/.aictl/config."
            confirmLabel="Unlock"
            confirmVariant="deny"
            onCancel={() => setPendingUnlock(null)}
            onConfirm={() => {
              const target = row();
              setPendingUnlock(null);
              void unlock(target.name);
            }}
          />
        )}
      </Show>
      <Show when={pendingLockAll()}>
        <ConfirmDelete
          title="Lock all API keys"
          detail={`${
            (rows() ?? []).filter(
              (r) => r.location === "plain" || r.location === "both",
            ).length
          } key(s) in plain config`}
          note="Every plain-config key will be moved into the system keyring."
          confirmLabel="Lock All"
          confirmVariant="allow"
          onCancel={() => setPendingLockAll(false)}
          onConfirm={() => {
            setPendingLockAll(false);
            void lockAll();
          }}
        />
      </Show>
      <Show when={pendingUnlockAll()}>
        <ConfirmDelete
          title="Unlock all API keys"
          detail={`${
            (rows() ?? []).filter(
              (r) => r.location === "keyring" || r.location === "both",
            ).length
          } key(s) in keyring`}
          note="Every keyring-stored key will be moved back into plain ~/.aictl/config."
          confirmLabel="Unlock All"
          confirmVariant="deny"
          onCancel={() => setPendingUnlockAll(false)}
          onConfirm={() => {
            setPendingUnlockAll(false);
            void unlockAll();
          }}
        />
      </Show>
    </div>
  );
};

interface KeyEditModalProps {
  row: KeyRow;
  draft: string;
  onDraft: (value: string) => void;
  onCancel: () => void;
  onSubmit: () => void;
}

const KeyEditModal: Component<KeyEditModalProps> = (props) => {
  let inputRef: HTMLInputElement | undefined;

  // Capture-phase + stopImmediatePropagation so the parent <Settings>'s
  // window-level Esc handler doesn't fire alongside this one and close
  // the whole panel underneath the modal.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (props.draft.trim() !== "") props.onSubmit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onCancel();
    }
  };

  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
    queueMicrotask(() => inputRef?.focus());
  });

  const title = () =>
    props.row.location === "unset" ? "Set API key" : "Replace API key";

  return (
    <div class="tool-modal" role="dialog" aria-modal="true">
      <div class="panel">
        <h2>{title()}</h2>
        <div class="create-prompt-target">
          for <code>{props.row.label || props.row.name}</code>
        </div>
        <input
          ref={inputRef}
          type="password"
          class="create-prompt-input"
          placeholder="paste key…"
          value={props.draft}
          onInput={(e) => props.onDraft(e.currentTarget.value)}
        />
        <div class="actions">
          <button type="button" data-variant="deny" onClick={props.onCancel}>
            Cancel Esc
          </button>
          <button
            type="button"
            data-variant="allow"
            disabled={props.draft.trim() === ""}
            onClick={props.onSubmit}
          >
            Save ↩
          </button>
        </div>
      </div>
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
    help: "Cap on LLM calls inside one agent turn — bounds runaway tool-call loops. 0 disables the cap (unlimited). Leave blank for the default.",
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
        Master toggles for the security gate, audit log, and prompt-injection
        guard. Fine-grained shell / path rules live in their own tab;
        outbound redaction has its own tab.
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
    return await ipc.behaviorRead();
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
      await ipc.behaviorWrite(draft());
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
        across every session. Stored at{" "}
        <code>~/.aictl/AICTL.md</code> and shared with the CLI.
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

const ToolsList: Component<{
  disabled: boolean;
  /// Fired after every successful per-tool flip so App can re-derive
  /// composer-icon state (globe = web tools, picture = image tools)
  /// in real time. Optional — callers that don't have a composer to
  /// keep in sync can ignore it.
  onToggle?: () => void;
}> = (props) => {
  const [tools, { refetch }] = createResource<ToolRow[]>(() => ipc.toolsList());
  const [error, setError] = createSignal<string | null>(null);

  const toggle = async (name: string, disable: boolean) => {
    setError(null);
    try {
      await ipc.toolSetDisabled(name, disable);
      await refetch();
      props.onToggle?.();
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

const ToolsTab: Component<{
  /// Forwarded to `ToolsList` so per-tool flips push the new policy
  /// back up to App, which re-derives the composer's globe / picture
  /// icon state in real time. Mirrors the wiring the General tab used
  /// to carry before the Tools section moved out.
  onToolToggled?: () => void;
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
      <h3>Tools</h3>
      <p class="settings-hint">
        Per-tool enable / disable and the default approval mode the
        agent uses when it calls a tool. Stored in{" "}
        <code>~/.aictl/config</code>.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>

      <h4 class="settings-subhead">Tool approval</h4>
      <div class="settings-row settings-row-stack">
        <label>Default approval mode</label>
        <div class="settings-control-line">
          <Dropdown
            value={approvalMode()}
            onChange={(v) => void setApproval(v as "ask" | "auto")}
            options={[
              { value: "ask", label: "Ask each tool call (recommended)" },
              { value: "auto", label: "Auto-accept all tool calls" },
            ]}
          />
        </div>
        <p class="settings-hint">
          The composer's per-conversation toggle still wins for the
          current session — this picks the default when the desktop
          launches.
        </p>
      </div>

      <h4 class="settings-subhead">Tools</h4>
      <BoolRow
        label="Tools enabled"
        help="Master switch — turn off to run the agent in chat-only mode (no shell, no file edits, no MCP)."
        on={toolsOn()}
        onChange={(v) => void setBool("AICTL_TOOLS_ENABLED", v)}
      />
      <ToolsList
        disabled={!toolsOn()}
        onToggle={() => props.onToolToggled?.()}
      />
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

  const setEnabled = async (next: boolean) => {
    setError(null);
    setFeedback(null);
    try {
      if (next) {
        await ipc.configClear("AICTL_CLIENT_ENABLED");
      } else {
        await ipc.configWrite("AICTL_CLIENT_ENABLED", "false");
      }
      await refetch();
      setFeedback(
        next
          ? "AICTL (self-hosted) enabled"
          : "AICTL (self-hosted) disabled",
      );
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>LLM Servers</h3>
      <h4 class="settings-subhead">AICTL (self-hosted)</h4>
      <p class="settings-hint">
        Route LLM calls through a self-hosted{" "}
        <code>aictl-server</code> by selecting AICTL (self-hosted) as
        the provider in the Model tab. The host URL and master key are
        also stored in <code>~/.aictl/config</code> so the CLI sees the
        same values.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <BoolRow
        label="Enabled"
        help="When off, the AICTL (self-hosted) route is hidden from model pickers and dispatch is short-circuited even if the host and master key are set. Stored in AICTL_CLIENT_ENABLED."
        on={status()?.enabled ?? true}
        onChange={(next) => void setEnabled(next)}
      />
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

  const setEnabled = async (next: boolean) => {
    setError(null);
    setFeedback(null);
    try {
      if (next) {
        await ipc.configClear("LLM_OLLAMA_ENABLED");
      } else {
        await ipc.configWrite("LLM_OLLAMA_ENABLED", "false");
      }
      await refetch();
      setFeedback(next ? "Ollama enabled" : "Ollama disabled");
    } catch (err) {
      setError(`${err}`);
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
      <BoolRow
        label="Enabled"
        help="When off, Ollama models disappear from the picker and chat calls short-circuit with an error. Stored in LLM_OLLAMA_ENABLED."
        on={status()?.enabled ?? true}
        onChange={(next) => void setEnabled(next)}
      />
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
  const [showEditor, setShowEditor] = createSignal(false);

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
      <div class="settings-keys-bulk" style={{ "margin-bottom": "var(--space-3)" }}>
        <button type="button" onClick={() => setShowEditor(true)}>
          New MCP server
        </button>
      </div>
      <Show
        when={(status()?.servers ?? []).length > 0}
        fallback={
          <p class="settings-hint">
            <em>No servers configured yet — click "New MCP server" to add one.</em>
          </p>
        }
      >
        <table class="settings-keys-table">
          <thead>
            <tr>
              <th>Server</th>
              <th>Transport</th>
              <th>Target</th>
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
                    <code>{row.transport || "stdio"}</code>
                  </td>
                  <td>
                    <code>{row.url || row.command}</code>
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
      <Show when={showEditor()}>
        <McpEditor
          existingNames={(status()?.servers ?? []).map((s) => s.name)}
          onSaved={(name) => {
            setShowEditor(false);
            setError(null);
            setFeedback(`saved ${name} (restart desktop to apply)`);
            void refetch();
          }}
          onClose={() => setShowEditor(false)}
        />
      </Show>
    </div>
  );
};

function fmtBytes(n: number): string {
  if (n <= 0) return "—";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

interface ActiveDownload {
  id: number;
  label: string;
  current: number;
  total: number | null;
  message: string | null;
}

interface ModelsTabProps {
  onRefreshModels: () => Promise<void>;
}

const ModelsTab: Component<ModelsTabProps> = (props) => {
  const [status, { refetch }] = createResource<LocalModelsStatus>(() =>
    ipc.localModelsStatus(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [showGgufDialog, setShowGgufDialog] = createSignal(false);
  const [showMlxDialog, setShowMlxDialog] = createSignal(false);
  const [downloads, setDownloads] = createSignal<ActiveDownload[]>([]);

  // Subscribe to the engine's progress events so model downloads render
  // an inline bar. The id is minted server-side; we treat any in-flight
  // ProgressBegin as a model download because the only emitter on the
  // desktop is the local-models pull path.
  onMount(() => {
    let unlisten: (() => void) | null = null;
    void ipc
      .onAgentEvent((evt) => {
        if (evt.kind === "progress_begin") {
          setDownloads((prev) => [
            ...prev,
            {
              id: evt.id,
              label: evt.label,
              current: 0,
              total: evt.total,
              message: null,
            },
          ]);
        } else if (evt.kind === "progress_update") {
          setDownloads((prev) =>
            prev.map((d) =>
              d.id === evt.id
                ? { ...d, current: evt.current, message: evt.message }
                : d,
            ),
          );
        } else if (evt.kind === "progress_end") {
          setDownloads((prev) => {
            const next = prev.filter((d) => d.id !== evt.id);
            // Clear the "downloading…" hint once the last in-flight
            // download wraps up. MLX downloads emit one Begin/End cycle
            // per repo file, so the feedback should stick around until
            // every file is done — not vanish after the first one. The
            // app-level catalogue is also refreshed at the same moment
            // so the composer dropdown and the Provider tab pick up the
            // new entry without an app restart.
            if (next.length === 0) {
              setFeedback(null);
              void props.onRefreshModels();
            }
            return next;
          });
          // Refetch the local-models status so the new model appears in
          // the per-backend table on this tab.
          void refetch();
        }
      })
      .then((u) => {
        unlisten = u;
      });
    onCleanup(() => {
      if (unlisten) unlisten();
    });
  });

  const removeGguf = async (name: string) => {
    if (!window.confirm(`Remove GGUF model "${name}"?`)) return;
    setError(null);
    setFeedback(null);
    try {
      await ipc.localModelsRemoveGguf(name);
      setFeedback(`removed ${name}`);
      await refetch();
      await props.onRefreshModels();
    } catch (err) {
      setError(`${err}`);
    }
  };

  const removeMlx = async (name: string) => {
    if (!window.confirm(`Remove MLX model "${name}"?`)) return;
    setError(null);
    setFeedback(null);
    try {
      await ipc.localModelsRemoveMlx(name);
      setFeedback(`removed ${name}`);
      await refetch();
      await props.onRefreshModels();
    } catch (err) {
      setError(`${err}`);
    }
  };

  return (
    <div class="settings-tab-content">
      <h3>Local models</h3>
      <p class="settings-warn">
        ⚠ Local model support (GGUF and MLX) is{" "}
        <strong>experimental</strong>. Downloads work today; inference may
        be rough or unavailable depending on the build flags. Expect rough
        edges.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>

      <Show when={downloads().length > 0}>
        <div class="settings-downloads">
          <h4 class="settings-subhead">In progress</h4>
          <For each={downloads()}>
            {(d) => (
              <div class="settings-download-row">
                <div class="settings-download-label">
                  {d.label}
                  <Show when={d.message}>
                    {(m) => <span class="settings-meta"> · {m()}</span>}
                  </Show>
                </div>
                <progress
                  class="settings-download-bar"
                  value={d.current}
                  max={d.total ?? undefined}
                />
                <div class="settings-download-meta">
                  {fmtBytes(d.current)}
                  <Show when={d.total}>
                    {(t) => <> / {fmtBytes(t())}</>}
                  </Show>
                </div>
              </div>
            )}
          </For>
        </div>
      </Show>

      <Show when={status()}>
        {(s) => (
          <>
            <h4 class="settings-subhead">Native GGUF (CPU, llama.cpp)</h4>
            <p class="settings-meta">
              <Show
                when={s().gguf.inference_available}
                fallback={
                  <>
                    Models can be downloaded, but this build was not
                    compiled with <code>--features gguf</code> — inference
                    is not available on this binary.
                  </>
                }
              >
                Inference enabled. Models live in{" "}
                <code>{s().gguf.dir}</code>.
              </Show>
            </p>
            <div class="settings-keys-bulk" style={{ "margin-bottom": "var(--space-3)" }}>
              <button type="button" onClick={() => setShowGgufDialog(true)}>
                Download GGUF model
              </button>
            </div>
            <Show
              when={s().gguf.models.length > 0}
              fallback={
                <p class="settings-hint">
                  <em>No GGUF models downloaded yet.</em>
                </p>
              }
            >
              <table class="settings-keys-table">
                <thead>
                  <tr>
                    <th>Model</th>
                    <th>Size</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  <For each={s().gguf.models}>
                    {(m) => (
                      <tr>
                        <td>
                          <code>{m.name}</code>
                        </td>
                        <td>{fmtBytes(m.size_bytes)}</td>
                        <td class="settings-keys-actions">
                          <button
                            type="button"
                            class="ghost mini"
                            onClick={() => void removeGguf(m.name)}
                          >
                            Remove
                          </button>
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </Show>

            <h4 class="settings-subhead">Native MLX (Apple Silicon)</h4>
            <p class="settings-meta">
              <Show
                when={s().mlx.host_supports_mlx}
                fallback={
                  <>
                    This host is not Apple Silicon — MLX models can be
                    downloaded for archival but cannot run here.
                  </>
                }
              >
                <Show
                  when={s().mlx.inference_available}
                  fallback={
                    <>
                      Models can be downloaded, but this build was not
                      compiled with <code>--features mlx</code> —
                      inference is not available on this binary.
                    </>
                  }
                >
                  Inference enabled. Models live in{" "}
                  <code>{s().mlx.dir}</code>.
                </Show>
              </Show>
            </p>
            <div class="settings-keys-bulk" style={{ "margin-bottom": "var(--space-3)" }}>
              <button type="button" onClick={() => setShowMlxDialog(true)}>
                Download MLX model
              </button>
            </div>
            <Show
              when={s().mlx.models.length > 0}
              fallback={
                <p class="settings-hint">
                  <em>No MLX models downloaded yet.</em>
                </p>
              }
            >
              <table class="settings-keys-table">
                <thead>
                  <tr>
                    <th>Model</th>
                    <th>Size</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  <For each={s().mlx.models}>
                    {(m) => (
                      <tr>
                        <td>
                          <code>{m.name}</code>
                        </td>
                        <td>{fmtBytes(m.size_bytes)}</td>
                        <td class="settings-keys-actions">
                          <button
                            type="button"
                            class="ghost mini"
                            onClick={() => void removeMlx(m.name)}
                          >
                            Remove
                          </button>
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </Show>
          </>
        )}
      </Show>

      <Show when={showGgufDialog() && status()}>
        <LocalModelDownloader
          backend="gguf"
          catalog={status()!.gguf.catalog}
          onDownload={async (spec, name) => {
            setError(null);
            try {
              await ipc.localModelsPullGguf(spec, name);
              setShowGgufDialog(false);
              setFeedback(`downloading ${name ?? spec}…`);
            } catch (err) {
              setError(`${err}`);
            }
          }}
          onClose={() => setShowGgufDialog(false)}
        />
      </Show>
      <Show when={showMlxDialog() && status()}>
        <LocalModelDownloader
          backend="mlx"
          catalog={status()!.mlx.catalog}
          onDownload={async (spec, name) => {
            setError(null);
            try {
              await ipc.localModelsPullMlx(spec, name);
              setShowMlxDialog(false);
              setFeedback(`downloading ${name ?? spec}…`);
            } catch (err) {
              setError(`${err}`);
            }
          }}
          onClose={() => setShowMlxDialog(false)}
        />
      </Show>
    </div>
  );
};

interface LocalModelDownloaderProps {
  backend: "gguf" | "mlx";
  catalog: { label: string; spec: string; size_label: string }[];
  onDownload: (spec: string, name?: string) => Promise<void>;
  onClose: () => void;
}

const LocalModelDownloader: Component<LocalModelDownloaderProps> = (props) => {
  const [pickIndex, setPickIndex] = createSignal<number | null>(0);
  const [customSpec, setCustomSpec] = createSignal("");
  const [name, setName] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  // Capture-phase + stopImmediatePropagation so the parent <Settings>'s
  // window-level Esc handler doesn't fire alongside this one and close
  // the whole panel underneath the modal.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onClose();
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });

  const resolvedSpec = () => {
    const idx = pickIndex();
    if (idx === null) return customSpec().trim();
    if (idx < props.catalog.length) return props.catalog[idx].spec;
    return customSpec().trim();
  };

  const isCustom = () => {
    const idx = pickIndex();
    return idx === null || idx >= props.catalog.length;
  };

  const submit = async () => {
    const spec = resolvedSpec();
    if (spec === "") return;
    setBusy(true);
    try {
      await props.onDownload(spec, name().trim() || undefined);
    } finally {
      setBusy(false);
    }
  };

  const title =
    props.backend === "gguf"
      ? "Download GGUF model"
      : "Download MLX model";
  const help =
    props.backend === "gguf"
      ? "Curated from lmstudio-community on Hugging Face."
      : "Curated from mlx-community on Hugging Face.";

  return (
    <div
      class="editor-modal-overlay"
      role="dialog"
      aria-modal="true"
      onClick={(e) => {
        if (e.target === e.currentTarget) props.onClose();
      }}
    >
      <div class="editor-modal-panel">
        <header class="editor-modal-header">
          <h2>{title}</h2>
          <button
            type="button"
            class="editor-modal-close"
            aria-label="Close"
            title="Close (Esc)"
            onClick={props.onClose}
          >
            ✕
          </button>
        </header>
        <div class="editor-modal-body">
          <p class="editor-modal-help">
            ⚠ Experimental. {help}
          </p>
          <div class="editor-modal-row">
            <label for="local-model-pick">Model</label>
            <Dropdown
              id="local-model-pick"
              value={String(pickIndex() ?? props.catalog.length)}
              onChange={(v) => {
                const n = Number.parseInt(v, 10);
                setPickIndex(Number.isFinite(n) ? n : props.catalog.length);
              }}
              options={[
                ...props.catalog.map((entry, i) => ({
                  value: String(i),
                  label: `${entry.label} — ${entry.size_label}`,
                })),
                {
                  value: String(props.catalog.length),
                  label: "custom spec…",
                },
              ]}
            />
          </div>
          <Show when={isCustom()}>
            <div class="editor-modal-row">
              <label for="local-model-spec">Spec</label>
              <input
                id="local-model-spec"
                type="text"
                placeholder={
                  props.backend === "gguf"
                    ? "owner/repo:filename.gguf or hf:owner/repo/path.gguf"
                    : "owner/repo (e.g. mlx-community/Llama-3.2-3B-Instruct-4bit)"
                }
                value={customSpec()}
                onInput={(e) => setCustomSpec(e.currentTarget.value)}
              />
              <p class="editor-modal-help">
                <Show when={props.backend === "gguf"}>
                  Accepts <code>hf:owner/repo/file.gguf</code>,{" "}
                  <code>owner/repo:file.gguf</code>, or an{" "}
                  <code>https://</code> URL.
                </Show>
                <Show when={props.backend === "mlx"}>
                  Accepts <code>mlx:owner/repo</code> or{" "}
                  <code>owner/repo</code>. Repo must be MLX-format
                  (safetensors).
                </Show>
              </p>
            </div>
          </Show>
          <div class="editor-modal-row">
            <label for="local-model-name">Local name (optional)</label>
            <input
              id="local-model-name"
              type="text"
              placeholder="leave blank to derive from spec"
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
            />
            <p class="editor-modal-help">
              Letters, numbers, dot, dash, and underscore only.
            </p>
          </div>
        </div>
        <footer class="editor-modal-footer">
          <button type="button" disabled={busy()} onClick={props.onClose}>
            Cancel
          </button>
          <button
            type="button"
            disabled={busy() || resolvedSpec() === ""}
            onClick={() => void submit()}
          >
            {busy() ? "Starting…" : "Download"}
          </button>
        </footer>
      </div>
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
            <Dropdown
              value={draftEvent()}
              onChange={(v) => setDraftEvent(v)}
              options={HOOK_EVENTS.map((ev) => ({ value: ev, label: ev }))}
            />
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
  // Capture-phase + stopImmediatePropagation so the parent <Settings>'s
  // window-level Esc handler doesn't fire alongside this one and close
  // the whole panel underneath the viewer.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onClose();
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
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
  const [remote, setRemote] = createSignal<RemoteCatalogueRow[]>([]);
  // Distinct "loading" / "error" channels for the remote fetch — the
  // catalogue depends on GitHub being reachable, which fails far more
  // often than a local FS read, so we keep its state separate from the
  // local list's hard error.
  const [remoteLoading, setRemoteLoading] = createSignal(false);
  const [remoteError, setRemoteError] = createSignal<string | null>(null);
  const [pulling, setPulling] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [viewer, setViewer] = createSignal<ViewerState | null>(null);
  const [showEditor, setShowEditor] = createSignal(false);

  const load = async () => {
    try {
      setSkills(await ipc.skillsList());
      setError(null);
    } catch (err) {
      setError(`${err}`);
    }
  };
  const loadRemote = async () => {
    setRemoteLoading(true);
    setRemoteError(null);
    try {
      setRemote(await ipc.skillsListRemote());
    } catch (err) {
      setRemoteError(`${err}`);
    } finally {
      setRemoteLoading(false);
    }
  };
  void load();
  void loadRemote();

  // Remote rows the user can act on — skip anything already installed
  // locally (matched by name) so the catalogue list shrinks as the
  // user pulls entries.
  const installable = createMemo(() => {
    const local = new Set(skills().map((s) => s.name));
    return remote().filter((r) => r.state === "not_pulled" && !local.has(r.name));
  });

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

  const pull = async (row: RemoteCatalogueRow) => {
    setError(null);
    setPulling(row.name);
    try {
      // overwrite=false — the local-installed filter above means we only
      // ever pull entries that aren't on disk, so a server-side conflict
      // would be a surprise worth reporting rather than auto-clobbering.
      await ipc.skillPull(row.name, false);
      setFeedback(`pulled ${row.name}`);
      await load();
      // Drop the row from the remote view immediately so the list
      // matches what the user sees in the local table.
      setRemote((prev) => prev.filter((r) => r.name !== row.name));
    } catch (err) {
      setError(`${err}`);
    } finally {
      setPulling(null);
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
      <div class="settings-keys-bulk">
        <button type="button" onClick={() => setShowEditor(true)}>
          New Skill
        </button>
      </div>
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
      <div class="settings-catalogue-section">
        <div class="settings-catalogue-header">
          <h4>Catalogue</h4>
          <button
            type="button"
            class="ghost mini"
            disabled={remoteLoading()}
            onClick={() => void loadRemote()}
          >
            {remoteLoading() ? "Refreshing…" : "Refresh"}
          </button>
        </div>
        <p class="settings-hint">
          First-party skills hosted in the aictl repo. Pull installs to{" "}
          <code>~/.aictl/skills/&lt;name&gt;/SKILL.md</code>.
        </p>
        <Show when={remoteError()}>
          <p class="settings-error">{remoteError()}</p>
        </Show>
        <Show
          when={installable().length > 0}
          fallback={
            <Show when={!remoteLoading() && !remoteError()}>
              <p class="settings-hint">
                <em>Catalogue empty — everything is already installed.</em>
              </p>
            </Show>
          }
        >
          <table class="settings-keys-table settings-catalogue-table">
            <colgroup>
              <col />
              <col class="settings-catalogue-actions-col" />
            </colgroup>
            <thead>
              <tr>
                <th>Skill</th>
                <th class="settings-actions-col" />
              </tr>
            </thead>
            <tbody>
              <For each={installable()}>
                {(row) => (
                  <tr>
                    <td>
                      <div class="settings-name-cell">
                        <code>{row.name}</code>
                      </div>
                      <Show when={row.description}>
                        <div class="settings-catalogue-desc">
                          {row.description}
                        </div>
                      </Show>
                    </td>
                    <td class="settings-actions-col">
                      <button
                        type="button"
                        class="ghost mini"
                        disabled={pulling() === row.name}
                        onClick={() => void pull(row)}
                      >
                        {pulling() === row.name ? "Pulling…" : "Pull"}
                      </button>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </div>
      <Show when={viewer()}>
        {(v) => <PromptViewer view={v()} onClose={() => setViewer(null)} />}
      </Show>
      <Show when={showEditor()}>
        <SkillEditor
          existingNames={(skills() ?? []).map((s) => s.name)}
          onSaved={(name) => {
            setShowEditor(false);
            setFeedback(`saved ${name}`);
            void load();
          }}
          onClose={() => setShowEditor(false)}
        />
      </Show>
    </div>
  );
};

const AgentsTab: Component = () => {
  // See SkillsTab — plain signal to dodge the createResource race that
  // otherwise re-introduces a deleted row when the initial fetch settles
  // after the optimistic mutate.
  const [agents, setAgents] = createSignal<AgentRow[]>([]);
  const [remote, setRemote] = createSignal<RemoteCatalogueRow[]>([]);
  const [remoteLoading, setRemoteLoading] = createSignal(false);
  const [remoteError, setRemoteError] = createSignal<string | null>(null);
  const [pulling, setPulling] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [viewer, setViewer] = createSignal<ViewerState | null>(null);
  const [showEditor, setShowEditor] = createSignal(false);

  const load = async () => {
    try {
      setAgents(await ipc.agentsList());
      setError(null);
    } catch (err) {
      setError(`${err}`);
    }
  };
  const loadRemote = async () => {
    setRemoteLoading(true);
    setRemoteError(null);
    try {
      setRemote(await ipc.agentsListRemote());
    } catch (err) {
      setRemoteError(`${err}`);
    } finally {
      setRemoteLoading(false);
    }
  };
  void load();
  void loadRemote();

  const installable = createMemo(() => {
    const local = new Set(agents().map((a) => a.name));
    return remote().filter((r) => r.state === "not_pulled" && !local.has(r.name));
  });

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

  const pull = async (row: RemoteCatalogueRow) => {
    setError(null);
    setPulling(row.name);
    try {
      await ipc.agentPull(row.name, false);
      setFeedback(`pulled ${row.name}`);
      await load();
      setRemote((prev) => prev.filter((r) => r.name !== row.name));
    } catch (err) {
      setError(`${err}`);
    } finally {
      setPulling(null);
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
      <div class="settings-keys-bulk">
        <button type="button" onClick={() => setShowEditor(true)}>
          New Agent
        </button>
      </div>
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
      <div class="settings-catalogue-section">
        <div class="settings-catalogue-header">
          <h4>Catalogue</h4>
          <button
            type="button"
            class="ghost mini"
            disabled={remoteLoading()}
            onClick={() => void loadRemote()}
          >
            {remoteLoading() ? "Refreshing…" : "Refresh"}
          </button>
        </div>
        <p class="settings-hint">
          First-party agents hosted in the aictl repo. Pull installs to{" "}
          <code>~/.aictl/agents/&lt;name&gt;.md</code>.
        </p>
        <Show when={remoteError()}>
          <p class="settings-error">{remoteError()}</p>
        </Show>
        <Show
          when={installable().length > 0}
          fallback={
            <Show when={!remoteLoading() && !remoteError()}>
              <p class="settings-hint">
                <em>Catalogue empty — everything is already installed.</em>
              </p>
            </Show>
          }
        >
          <table class="settings-keys-table settings-catalogue-table">
            <colgroup>
              <col />
              <col class="settings-catalogue-actions-col" />
            </colgroup>
            <thead>
              <tr>
                <th>Agent</th>
                <th class="settings-actions-col" />
              </tr>
            </thead>
            <tbody>
              <For each={installable()}>
                {(row) => (
                  <tr>
                    <td>
                      <div class="settings-name-cell">
                        <code>{row.name}</code>
                      </div>
                      <Show when={row.description}>
                        <div class="settings-catalogue-desc">
                          {row.description}
                        </div>
                      </Show>
                    </td>
                    <td class="settings-actions-col">
                      <button
                        type="button"
                        class="ghost mini"
                        disabled={pulling() === row.name}
                        onClick={() => void pull(row)}
                      >
                        {pulling() === row.name ? "Pulling…" : "Pull"}
                      </button>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </div>
      <Show when={viewer()}>
        {(v) => <PromptViewer view={v()} onClose={() => setViewer(null)} />}
      </Show>
      <Show when={showEditor()}>
        <AgentEditor
          existingNames={(agents() ?? []).map((a) => a.name)}
          onSaved={(name) => {
            setShowEditor(false);
            setFeedback(`saved ${name}`);
            void load();
          }}
          onClose={() => setShowEditor(false)}
        />
      </Show>
    </div>
  );
};

const DEFAULT_PLUGIN_BODY = `#!/bin/sh
# Receives the tool body on stdin, writes the tool output on stdout.
# Non-zero exit codes surface as "[exit N] <stderr>" to the agent.
cat
`;

const MemoryTab: Component = () => {
  const [status, { refetch }] = createResource<MemoryStatus>(() =>
    ipc.memoryStatus(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [draft, setDraft] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  // Row whose full text is shown in the modal. `null` = closed.
  const [viewing, setViewing] = createSignal<MemoryRow | null>(null);
  // Row pending delete confirmation. `null` = no prompt open.
  const [pendingDelete, setPendingDelete] = createSignal<MemoryRow | null>(null);
  // True when the "clear all" confirmation modal is open.
  const [pendingClearAll, setPendingClearAll] = createSignal(false);

  const setEnabled = async (on: boolean) => {
    setError(null);
    setFeedback(null);
    try {
      await ipc.memorySetEnabled(on);
      await refetch();
      setFeedback(
        on
          ? "memory enabled — facts will load into the system prompt"
          : "memory disabled — saved entries kept on disk but not loaded",
      );
    } catch (err) {
      setError(`${err}`);
    }
  };

  const add = async () => {
    setError(null);
    setFeedback(null);
    const text = draft().trim();
    if (text === "") {
      setError("Fact is empty.");
      return;
    }
    setSaving(true);
    try {
      const row = await ipc.memoryAdd(text);
      setDraft("");
      setFeedback(`saved: ${row.text}`);
      await refetch();
    } catch (err) {
      setError(`${err}`);
    } finally {
      setSaving(false);
    }
  };

  const performRemove = async (row: MemoryRow) => {
    setError(null);
    setFeedback(null);
    try {
      await ipc.memoryRemove(row.id);
      await refetch();
      setFeedback("memory deleted");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const performClearAll = async () => {
    setError(null);
    setFeedback(null);
    try {
      await ipc.memoryClear();
      await refetch();
      setFeedback("all memories cleared");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const formatDate = (secs: number) => {
    if (!secs) return "—";
    const d = new Date(secs * 1000);
    return d.toLocaleString();
  };

  return (
    <div class="settings-tab-content">
      <h3>Memory</h3>
      <p class="settings-hint">
        Long-term facts the agent has learned about you. Stored in{" "}
        <code>~/.aictl/memory.json</code> and injected into the system
        prompt of every conversation when memory is enabled. Disabled in
        incognito sessions.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>
      <BoolRow
        label="Memory enabled"
        help="When on, saved facts load into the system prompt at the start of every turn."
        on={status()?.enabled ?? false}
        onChange={(v) => void setEnabled(v)}
      />
      <Show when={status()}>
        {(s) => (
          <p class="settings-meta">
            {s().count} / {s().max_entries} memories stored.
          </p>
        )}
      </Show>
      <div class="settings-row settings-row-stack">
        <label>Add a memory</label>
        <div class="settings-control-line">
          <input
            type="text"
            class="settings-text-input"
            placeholder="e.g. user is a senior Rust engineer"
            value={draft()}
            onInput={(e) => setDraft(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void add();
              }
            }}
          />
          <button
            type="button"
            disabled={saving() || draft().trim() === ""}
            onClick={() => void add()}
          >
            {saving() ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
      <Show
        when={(status()?.entries ?? []).length > 0}
        fallback={
          <p class="settings-hint">
            <em>No memories saved yet.</em>
          </p>
        }
      >
        <div class="settings-actions">
          <button
            type="button"
            class="ghost mini danger"
            onClick={() => {
              if ((status()?.entries.length ?? 0) > 0) {
                setPendingClearAll(true);
              }
            }}
          >
            Clear all
          </button>
        </div>
        <table class="settings-keys-table settings-memory-table">
          <thead>
            <tr>
              <th>#</th>
              <th>Memory</th>
              <th>Saved</th>
              <th />
            </tr>
          </thead>
          <tbody>
            <For each={status()?.entries ?? []}>
              {(row, i) => (
                <tr>
                  <td>{i() + 1}</td>
                  <td
                    class="settings-desc settings-memory-cell"
                    title={row.text}
                  >
                    {row.text}
                  </td>
                  <td>{formatDate(row.created_at)}</td>
                  <td class="settings-memory-actions-cell">
                    <div class="settings-keys-actions">
                      <button
                        type="button"
                        class="ghost mini"
                        onClick={() => setViewing(row)}
                      >
                        View
                      </button>
                      <button
                        type="button"
                        class="ghost mini danger"
                        onClick={() => setPendingDelete(row)}
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
      <Show when={viewing()}>
        {(row) => (
          <MemoryViewer
            row={row()}
            formattedDate={formatDate(row().created_at)}
            onClose={() => setViewing(null)}
          />
        )}
      </Show>
      <Show when={pendingDelete()}>
        {(row) => (
          <ConfirmDelete
            title="Delete memory"
            detail={row().text}
            note="The fact will no longer be loaded into the system prompt."
            onCancel={() => setPendingDelete(null)}
            onConfirm={() => {
              const target = row();
              setPendingDelete(null);
              void performRemove(target);
            }}
          />
        )}
      </Show>
      <Show when={pendingClearAll()}>
        <ConfirmDelete
          title="Clear all memories"
          detail={`${status()?.entries.length ?? 0} memor${
            (status()?.entries.length ?? 0) === 1 ? "y" : "ies"
          }`}
          note="Every saved fact will be removed."
          onCancel={() => setPendingClearAll(false)}
          onConfirm={() => {
            setPendingClearAll(false);
            void performClearAll();
          }}
        />
      </Show>
    </div>
  );
};

const MemoryViewer: Component<{
  row: MemoryRow;
  formattedDate: string;
  onClose: () => void;
}> = (props) => {
  // Capture-phase + stopImmediatePropagation so the parent <Settings>'s
  // window-level Esc handler doesn't fire alongside this one and close
  // the whole panel underneath the viewer.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onClose();
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });
  return (
    <div class="prompt-viewer-overlay" role="dialog" aria-modal="true">
      <div class="prompt-viewer memory-viewer">
        <header class="prompt-viewer-header">
          <div>
            <h3>Memory</h3>
            <p class="settings-meta">
              Saved {props.formattedDate} ·{" "}
              <code>{props.row.id}</code>
            </p>
          </div>
          <div class="prompt-viewer-actions">
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
          <pre class="prompt-viewer-source">{props.row.text}</pre>
        </div>
      </div>
    </div>
  );
};

const PluginsTab: Component = () => {
  const [status, { refetch }] = createResource<PluginsStatus>(() =>
    ipc.pluginsStatus(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [adding, setAdding] = createSignal(false);
  const [draftName, setDraftName] = createSignal("");
  const [draftDescription, setDraftDescription] = createSignal("");
  const [draftBody, setDraftBody] = createSignal(DEFAULT_PLUGIN_BODY);
  const [draftConfirm, setDraftConfirm] = createSignal(true);
  const [draftTimeout, setDraftTimeout] = createSignal("");
  const [saving, setSaving] = createSignal(false);

  const resetDraft = () => {
    setDraftName("");
    setDraftDescription("");
    setDraftBody(DEFAULT_PLUGIN_BODY);
    setDraftConfirm(true);
    setDraftTimeout("");
  };

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

  const validName = () => /^[A-Za-z0-9_-]+$/.test(draftName().trim());
  const clash = () => {
    const n = draftName().trim();
    return (
      n !== "" && (status()?.plugins ?? []).some((p) => p.name === n)
    );
  };

  const save = async () => {
    setError(null);
    setFeedback(null);
    if (!validName()) {
      setError("Invalid name — letters, numbers, underscore, or dash only.");
      return;
    }
    if (draftDescription().trim() === "") {
      setError("Description is empty.");
      return;
    }
    if (draftBody().trim() === "") {
      setError("Entrypoint script is empty.");
      return;
    }
    let timeoutSecs: number | undefined;
    const t = draftTimeout().trim();
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
        `A plugin named "${draftName().trim()}" already exists. Overwrite it?`,
      );
      if (!ok) return;
    }
    setSaving(true);
    try {
      const outcome = await ipc.pluginSave({
        name: draftName().trim(),
        description: draftDescription().trim(),
        body: draftBody(),
        requiresConfirmation: draftConfirm(),
        timeoutSecs,
        overwrite: clash(),
      });
      setFeedback(
        `plugin ${outcome === "overwritten" ? "overwritten" : "installed"} — restart desktop so the agent can call it`,
      );
      resetDraft();
      setAdding(false);
      await refetch();
    } catch (err) {
      setError(`${err}`);
    } finally {
      setSaving(false);
    }
  };

  const remove = async (name: string) => {
    if (!window.confirm(`Delete plugin "${name}"?`)) return;
    setError(null);
    setFeedback(null);
    try {
      await ipc.pluginDelete(name);
      await refetch();
      setFeedback(`plugin "${name}" deleted`);
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
      <div class="settings-actions">
        <button
          type="button"
          onClick={() => {
            if (adding()) {
              resetDraft();
            }
            setAdding((v) => !v);
          }}
        >
          {adding() ? "Cancel" : "Add plugin"}
        </button>
      </div>
      <Show when={adding()}>
        <div class="settings-row settings-row-stack">
          <label>New plugin</label>
          <div class="settings-control-line">
            <input
              type="text"
              class="settings-text-input"
              placeholder="name (letters, numbers, _, -)"
              value={draftName()}
              onInput={(e) => setDraftName(e.currentTarget.value)}
            />
            <input
              type="text"
              class="settings-text-input"
              placeholder="short description shown to the model"
              value={draftDescription()}
              onInput={(e) => setDraftDescription(e.currentTarget.value)}
            />
          </div>
          <Show when={draftName() !== "" && !validName()}>
            <p class="settings-hint" style={{ color: "var(--text-danger, #c33)" }}>
              Use only letters, numbers, underscore, or dash.
            </p>
          </Show>
          <Show when={validName() && clash()}>
            <p class="settings-hint">
              A plugin with this name already exists — saving will prompt to overwrite.
            </p>
          </Show>
          <textarea
            class="settings-textarea"
            rows={10}
            placeholder="entrypoint script — receives the tool body on stdin, prints the result on stdout"
            value={draftBody()}
            onInput={(e) => setDraftBody(e.currentTarget.value)}
            spellcheck={false}
            style={{ "font-family": "var(--mono, monospace)" }}
          />
          <p class="settings-hint">
            Saved as <code>~/.aictl/plugins/&lt;name&gt;/run</code> and chmod 755.
            Start with a shebang (<code>#!/bin/sh</code>, <code>#!/usr/bin/env python3</code>, …).
          </p>
          <BoolRow
            label="Requires confirmation"
            help="When on, every call goes through the tool-confirm dialog."
            on={draftConfirm()}
            onChange={(v) => setDraftConfirm(v)}
          />
          <div class="settings-control-line">
            <input
              type="number"
              min="1"
              class="settings-num-input"
              placeholder="timeout"
              value={draftTimeout()}
              onInput={(e) => setDraftTimeout(e.currentTarget.value)}
            />
            <span class="settings-suffix">s timeout (optional)</span>
            <button type="button" disabled={saving()} onClick={() => void save()}>
              {saving() ? "Saving…" : "Save"}
            </button>
          </div>
        </div>
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
              <th />
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
                  <td class="settings-keys-actions">
                    <button
                      type="button"
                      class="ghost mini danger"
                      onClick={() => void remove(row.name)}
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

const SessionsTab: Component<{
  /// Wrap the App-level deleteSession so the sidebar list refreshes and
  /// the chat resets when the active session is the one being removed.
  /// Settings-local refetch still runs after to refresh this tab's table.
  onDeleteSession: (id: string) => Promise<void>;
  /// Same wrapper for bulk clear — App-level handler ensures a fresh
  /// session window opens whenever the active conversation is wiped.
  onClearAll: () => Promise<void>;
}> = (props) => {
  const [rows, { refetch }] = createResource<SessionRow[]>(() =>
    ipc.listSessions(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  // Both delete paths are destructive and irreversible (`clear_sessions`
  // walks ~/.aictl/sessions/ and unlinks every file; per-row delete is
  // a single unlink), so each is gated behind a ConfirmDelete modal —
  // mirrors the memory tab's pattern.
  const [pendingDelete, setPendingDelete] = createSignal<SessionRow | null>(
    null,
  );
  const [pendingDeleteAll, setPendingDeleteAll] = createSignal(false);
  const [viewing, setViewing] = createSignal<SessionRow | null>(null);
  // Transient "Copied" state keyed by session id so each row's button
  // can flip back independently after the 1.2s window.
  const [copiedId, setCopiedId] = createSignal<string | null>(null);

  const copyId = async (id: string) => {
    const ok = await copyToClipboard(id);
    if (!ok) {
      setError("failed to copy session id");
      return;
    }
    setCopiedId(id);
    setFeedback(null);
    setError(null);
    window.setTimeout(() => {
      if (copiedId() === id) setCopiedId(null);
    }, 1200);
  };

  const performRemove = async (row: SessionRow) => {
    setError(null);
    try {
      await props.onDeleteSession(row.id);
      await refetch();
      setFeedback("session deleted");
    } catch (err) {
      setError(`${err}`);
    }
  };

  const performClearAll = async () => {
    setError(null);
    try {
      await props.onClearAll();
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

  const fmtAbsolute = (secs: number) =>
    new Date(secs * 1000).toLocaleString();

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
        <button
          type="button"
          onClick={() => {
            if ((rows() ?? []).length > 0) {
              setPendingDeleteAll(true);
            }
          }}
        >
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
        <table class="settings-keys-table settings-sessions-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Id</th>
              <th class="settings-sessions-size-col">Size</th>
              <th class="settings-sessions-modified-col">Modified</th>
              <th class="settings-sessions-actions-col" />
            </tr>
          </thead>
          <tbody>
            <For each={rows() ?? []}>
              {(row) => (
                <tr>
                  <td class="settings-sessions-name">
                    {row.name ?? <em>unnamed</em>}
                  </td>
                  <td class="settings-sessions-id">
                    <code>{row.id}</code>
                    <Show when={row.active}>
                      {" "}
                      <span class="badge">active</span>
                    </Show>
                  </td>
                  <td>{fmtSize(row.size)}</td>
                  <td>{fmtAge(row.modified_secs)}</td>
                  <td class="settings-keys-actions-cell">
                    <div class="settings-keys-actions">
                      <button
                        type="button"
                        class="ghost mini"
                        onClick={() => setViewing(row)}
                      >
                        View
                      </button>
                      <button
                        type="button"
                        class="ghost mini"
                        title="Copy session id"
                        onClick={() => void copyId(row.id)}
                      >
                        {copiedId() === row.id ? "Copied" : "Copy"}
                      </button>
                      <button
                        type="button"
                        class="ghost mini danger"
                        onClick={() => setPendingDelete(row)}
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
      <Show when={viewing()}>
        {(row) => (
          <SessionViewer
            row={row()}
            sizeLabel={fmtSize(row().size)}
            modifiedAbsolute={fmtAbsolute(row().modified_secs)}
            modifiedRelative={fmtAge(row().modified_secs)}
            copied={copiedId() === row().id}
            onCopyId={() => void copyId(row().id)}
            onClose={() => setViewing(null)}
          />
        )}
      </Show>
      <Show when={pendingDelete()}>
        {(row) => (
          <ConfirmDelete
            title="Delete session"
            detail={row().name ?? row().id}
            note={
              row().active
                ? "This is the active session — the chat pane will be reset to a fresh session."
                : "The conversation file will be removed. This cannot be undone."
            }
            onCancel={() => setPendingDelete(null)}
            onConfirm={() => {
              const target = row();
              setPendingDelete(null);
              void performRemove(target);
            }}
          />
        )}
      </Show>
      <Show when={pendingDeleteAll()}>
        <ConfirmDelete
          title="Delete all sessions"
          detail={`${(rows() ?? []).length} session${
            (rows() ?? []).length === 1 ? "" : "s"
          }`}
          note="Every saved conversation under ~/.aictl/sessions/ will be removed. This cannot be undone."
          onCancel={() => setPendingDeleteAll(false)}
          onConfirm={() => {
            setPendingDeleteAll(false);
            void performClearAll();
          }}
        />
      </Show>
    </div>
  );
};

const SessionViewer: Component<{
  row: SessionRow;
  sizeLabel: string;
  modifiedAbsolute: string;
  modifiedRelative: string;
  copied: boolean;
  onCopyId: () => void;
  onClose: () => void;
}> = (props) => {
  // Capture-phase + stopImmediatePropagation so the parent <Settings>'s
  // window-level Esc handler doesn't fire alongside this one and close
  // the whole panel underneath the viewer.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onClose();
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });

  return (
    <div class="tool-modal" role="dialog" aria-modal="true">
      <div class="panel">
        <h2>Session</h2>
        <dl class="settings-session-fields">
          <dt>Name</dt>
          <dd>
            {props.row.name ?? <em>unnamed</em>}
          </dd>
          <dt>Id</dt>
          <dd class="settings-session-id-row">
            <code>{props.row.id}</code>
            <button
              type="button"
              class="ghost mini"
              onClick={props.onCopyId}
            >
              {props.copied ? "Copied" : "Copy"}
            </button>
          </dd>
          <dt>Size</dt>
          <dd>{props.sizeLabel}</dd>
          <dt>Modified</dt>
          <dd>
            {props.modifiedAbsolute}{" "}
            <span class="settings-meta">({props.modifiedRelative})</span>
          </dd>
          <dt>Active</dt>
          <dd>{props.row.active ? "yes" : "no"}</dd>
          <dt>Path</dt>
          <dd>
            <code>~/.aictl/sessions/{props.row.id}</code>
          </dd>
        </dl>
        <div class="actions">
          <button type="button" data-variant="allow" onClick={props.onClose}>
            Close Esc
          </button>
        </div>
      </div>
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

const DAILY_RANGE = 30;

type DailyMetric = "requests" | "tokens" | "cost";

const StatsTab: Component = () => {
  const [snap, { refetch }] = createResource<StatsSnapshot>(() =>
    ipc.statsSnapshot(),
  );
  const [daily, { refetch: refetchDaily }] = createResource<DailyPoint[]>(() =>
    ipc.statsDaily(DAILY_RANGE),
  );
  const [metric, setMetric] = createSignal<DailyMetric>("requests");
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);

  const clear = async () => {
    setError(null);
    try {
      await ipc.statsClear();
      await Promise.all([refetch(), refetchDaily()]);
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
      <Show when={daily()}>
        {(points) => (
          <DailyChart
            points={points()}
            metric={metric()}
            onMetricChange={setMetric}
          />
        )}
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
              <BucketCard
                label="Overall"
                bucket={s().overall}
                wide
              />
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

const METRIC_LABELS: Record<DailyMetric, string> = {
  requests: "Requests",
  tokens: "Tokens",
  cost: "Cost (USD)",
};

const DailyChart: Component<{
  points: DailyPoint[];
  metric: DailyMetric;
  onMetricChange: (m: DailyMetric) => void;
}> = (props) => {
  const valueOf = (p: DailyPoint): number => {
    if (props.metric === "requests") return p.requests;
    if (props.metric === "tokens") return p.input_tokens + p.output_tokens;
    return p.cost_usd;
  };
  const fmtValue = (v: number): string => {
    if (props.metric === "cost") return `$${v.toFixed(4)}`;
    if (props.metric === "tokens") return v.toLocaleString();
    return `${v}`;
  };

  const width = 720;
  const height = 180;
  const padX = 48;
  const padY = 16;
  const innerW = () => width - padX * 2;
  const innerH = () => height - padY * 2;
  const max = () => {
    const values = props.points.map(valueOf);
    const m = Math.max(0, ...values);
    return m === 0 ? 1 : m;
  };
  const total = () => props.points.reduce((acc, p) => acc + valueOf(p), 0);
  const peak = () => {
    let best: { date: string; value: number } | null = null;
    for (const p of props.points) {
      const v = valueOf(p);
      if (best === null || v > best.value) best = { date: p.date, value: v };
    }
    return best;
  };
  const barWidth = () => {
    const n = props.points.length || 1;
    return Math.max(1, innerW() / n - 2);
  };
  const xFor = (i: number) => {
    const n = props.points.length || 1;
    return padX + (i * innerW()) / n + 1;
  };
  const yFor = (v: number) => {
    return padY + innerH() - (v / max()) * innerH();
  };
  const tickValues = () => [0, 0.5, 1].map((f) => f * max());

  return (
    <div class="settings-stats-chart">
      <div class="settings-stats-chart-header">
        <h4>Last {props.points.length} days</h4>
        <div class="settings-stats-chart-tabs" role="tablist">
          <For each={Object.keys(METRIC_LABELS) as DailyMetric[]}>
            {(m) => (
              <button
                type="button"
                role="tab"
                aria-selected={props.metric === m}
                class="settings-stats-chart-tab"
                classList={{ active: props.metric === m }}
                onClick={() => props.onMetricChange(m)}
              >
                {METRIC_LABELS[m]}
              </button>
            )}
          </For>
        </div>
      </div>
      <svg
        class="settings-stats-chart-svg"
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={`Daily ${METRIC_LABELS[props.metric]}`}
      >
        <For each={tickValues()}>
          {(t) => (
            <>
              <line
                x1={padX}
                x2={width - padX}
                y1={yFor(t)}
                y2={yFor(t)}
                class="settings-stats-chart-grid"
              />
              <text
                x={padX - 6}
                y={yFor(t) + 3}
                class="settings-stats-chart-axis"
                text-anchor="end"
              >
                {axisLabel(t, props.metric)}
              </text>
            </>
          )}
        </For>
        <For each={props.points}>
          {(p, i) => {
            const v = valueOf(p);
            const h = v === 0 ? 0 : Math.max(1, (v / max()) * innerH());
            return (
              <rect
                x={xFor(i())}
                y={padY + innerH() - h}
                width={barWidth()}
                height={h}
                class="settings-stats-chart-bar"
                classList={{ empty: v === 0 }}
              >
                <title>
                  {p.date} · {fmtValue(v)}
                </title>
              </rect>
            );
          }}
        </For>
        <Show when={props.points.length > 0}>
          <text
            x={padX}
            y={height - 2}
            class="settings-stats-chart-axis"
            text-anchor="start"
          >
            {props.points[0].date}
          </text>
          <text
            x={width - padX}
            y={height - 2}
            class="settings-stats-chart-axis"
            text-anchor="end"
          >
            {props.points[props.points.length - 1].date}
          </text>
        </Show>
      </svg>
      <div class="settings-stats-chart-summary">
        <span>
          Total <strong>{fmtValue(total())}</strong>
        </span>
        <Show when={peak() && peak()!.value > 0}>
          <span>
            Peak <strong>{fmtValue(peak()!.value)}</strong> on{" "}
            <code>{peak()!.date}</code>
          </span>
        </Show>
      </div>
    </div>
  );
};

const axisLabel = (v: number, metric: DailyMetric): string => {
  if (metric === "cost") return v === 0 ? "$0" : `$${v.toFixed(2)}`;
  if (metric === "tokens") {
    if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
    if (v >= 1_000) return `${(v / 1_000).toFixed(1)}k`;
    return `${Math.round(v)}`;
  }
  return `${Math.round(v)}`;
};

const BucketCard: Component<{
  label: string;
  bucket: StatsBucket;
  wide?: boolean;
}> = (props) => {
  const tokenTotal = () =>
    props.bucket.input_tokens + props.bucket.output_tokens;
  const inputPct = () => {
    const t = tokenTotal();
    if (t === 0) return 0;
    return (props.bucket.input_tokens / t) * 100;
  };
  const topModels = () => props.bucket.models.slice(0, 5);
  const modelMax = () => {
    const m = Math.max(0, ...topModels().map((row) => row.count));
    return m === 0 ? 1 : m;
  };
  return (
    <div
      class="settings-stats-card"
      classList={{ "settings-stats-card-wide": props.wide }}
    >
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
        <dt>Cost (USD)</dt>
        <dd>${props.bucket.cost_usd.toFixed(4)}</dd>
      </dl>
      <Show when={tokenTotal() > 0}>
        <h5>Tokens</h5>
        <div
          class="settings-stats-tokenbar"
          role="img"
          aria-label={`Input ${props.bucket.input_tokens.toLocaleString()}, output ${props.bucket.output_tokens.toLocaleString()}`}
        >
          <div
            class="settings-stats-tokenbar-input"
            style={{ width: `${inputPct()}%` }}
          />
          <div
            class="settings-stats-tokenbar-output"
            style={{ width: `${100 - inputPct()}%` }}
          />
        </div>
        <div class="settings-stats-tokenbar-legend">
          <span>
            <i class="dot input" /> in{" "}
            {props.bucket.input_tokens.toLocaleString()}
          </span>
          <span>
            <i class="dot output" /> out{" "}
            {props.bucket.output_tokens.toLocaleString()}
          </span>
        </div>
      </Show>
      <Show when={topModels().length > 0}>
        <h5>Top models</h5>
        <ul class="settings-stats-modelbars">
          <For each={topModels()}>
            {(m) => (
              <li>
                <div class="settings-stats-modelbars-row">
                  <code title={m.model}>{m.model}</code>
                  <span>{m.count}</span>
                </div>
                <div
                  class="settings-stats-modelbars-track"
                  aria-hidden="true"
                >
                  <div
                    class="settings-stats-modelbars-fill"
                    style={{ width: `${(m.count / modelMax()) * 100}%` }}
                  />
                </div>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </div>
  );
};

export const REDACTION_DETECTORS = [
  { slug: "api_key", label: "API keys" },
  { slug: "aws", label: "AWS access keys" },
  { slug: "aws_secret", label: "AWS secret keys" },
  { slug: "jwt", label: "JWTs" },
  { slug: "private_key", label: "Private keys (PEM)" },
  { slug: "connection_string", label: "Connection strings" },
  { slug: "credit_card", label: "Credit cards" },
  { slug: "iban", label: "IBAN" },
  { slug: "email", label: "Emails" },
  { slug: "phone", label: "Phone numbers" },
  { slug: "url_secret", label: "URL query-parameter secrets" },
  { slug: "ssn", label: "US SSNs" },
  { slug: "pesel", label: "Polish PESEL" },
  { slug: "ip_address", label: "IP addresses" },
  { slug: "mac_address", label: "MAC addresses" },
  { slug: "high_entropy", label: "High-entropy strings" },
  { slug: "person_name", label: "Person names (NER)" },
  { slug: "location", label: "Locations (NER)" },
  { slug: "organization", label: "Organizations (NER)" },
];

const RedactionTab: Component = () => {
  const [config, { refetch }] = createResource<ConfigEntry[]>(() =>
    ipc.configDump(),
  );
  const [ner, { refetch: refetchNer }] = createResource<NerStatus>(() =>
    ipc.nerStatus(),
  );
  const [error, setError] = createSignal<string | null>(null);
  const [feedback, setFeedback] = createSignal<string | null>(null);
  const [nerDownload, setNerDownload] = createSignal<{
    id: number | null;
    label: string;
    current: number;
    total: number | null;
    message: string | null;
  } | null>(null);
  // `window.confirm` is unreliable inside the Tauri webview, so both
  // prompts go through the in-app ConfirmDelete modal. The checkbox
  // path stashes its DOM input ref so cancel can snap `.checked` back
  // to false — Solid's one-way `checked={...}` binding doesn't re-apply
  // when the underlying signal hasn't changed.
  const [pendingNerDownload, setPendingNerDownload] = createSignal<{
    source: "button" | "checkbox";
    checkbox: HTMLInputElement | null;
  } | null>(null);
  const [pendingNerRemove, setPendingNerRemove] = createSignal(false);
  // Set to true when the user opted into NER via the checkbox edge
  // (which triggered a download). Read on the final `progress_end`
  // event so we flip `AICTL_REDACTION_NER=true` only once the model
  // actually lands on disk — enabling earlier would mean the redaction
  // policy logs a one-shot warning and silently skips Layer C.
  const [enableNerAfterPull, setEnableNerAfterPull] = createSignal(false);

  // Listen for progress_* events so the NER pull renders an inline bar.
  // The pull happens on a background thread and its id is minted
  // server-side; we correlate by `label` (the per-file label the
  // download_model helper passes to progress_begin) until the first
  // event arrives.
  onMount(() => {
    let unlisten: (() => void) | null = null;
    void ipc
      .onAgentEvent((evt) => {
        if (evt.kind === "progress_begin") {
          // Adopt this id only if no download is active yet (we kicked
          // off a pull and are waiting on the first event). Two files
          // download in sequence; on the second begin we swap labels.
          setNerDownload((prev) =>
            prev
              ? {
                  ...prev,
                  id: evt.id,
                  label: evt.label,
                  current: 0,
                  total: evt.total,
                  message: null,
                }
              : prev,
          );
        } else if (evt.kind === "progress_update") {
          setNerDownload((prev) =>
            prev && (prev.id === null || prev.id === evt.id)
              ? { ...prev, current: evt.current, message: evt.message }
              : prev,
          );
        } else if (evt.kind === "progress_end") {
          setNerDownload((prev) => {
            if (!prev || (prev.id !== null && prev.id !== evt.id)) return prev;
            // The NER pull fires two Begin/End cycles (tokenizer.json,
            // then onnx/model.onnx). The second End is the real wrap-up
            // — we detect it by checking the label prefix; the helper
            // labels every file as "[idx/total] <name>".
            const isFinal = prev.label.startsWith("[2/2]");
            if (!isFinal) {
              // Reset id so the next ProgressBegin gets adopted.
              return { ...prev, id: null };
            }
            void refetchNer();
            setFeedback(`downloaded NER model`);
            // If the user opted into NER via the checkbox edge, flip
            // the config flag on now that the model is actually on
            // disk. The deferred enable is the whole point of gating
            // the checkbox in the first place.
            if (enableNerAfterPull()) {
              setEnableNerAfterPull(false);
              void setConfig("AICTL_REDACTION_NER", "true");
            }
            return null;
          });
        }
      })
      .then((u) => {
        unlisten = u;
      });
    onCleanup(() => {
      if (unlisten) unlisten();
    });
  });

  const get = (key: string): string => {
    const entry = (config() ?? []).find((e) => e.key === key);
    return entry?.value ?? "";
  };

  // Shared between the explicit Download button and the enable-NER
  // checkbox so both paths kick off the same background pull and seed
  // the progress bar with a placeholder until the first
  // `progress_begin` event arrives.
  const startNerPull = (spec: string) => {
    setError(null);
    setFeedback(null);
    setNerDownload({
      id: null,
      label: spec,
      current: 0,
      total: null,
      message: null,
    });
    void ipc.nerPull(spec).catch((err) => {
      setNerDownload(null);
      // The deferred enable only makes sense if the download actually
      // happens — clear it on IPC failure so a later, unrelated pull
      // doesn't inherit the stale intent.
      setEnableNerAfterPull(false);
      setError(`${err}`);
    });
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

  const mode = () => {
    const raw = get("AICTL_SECURITY_REDACTION").trim().toLowerCase();
    if (raw === "redact" || raw === "block") return raw;
    return "off";
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
    if (v === "") {
      // These keys default to OFF when unset, mirroring the Rust policy.
      return (
        key !== "AICTL_REDACTION_NER" &&
        key !== "AICTL_SECURITY_REDACTION_LOCAL"
      );
    }
    return v !== "false" && v !== "0";
  };

  return (
    <div class="settings-tab-content">
      <h3>Redaction</h3>
      <p class="settings-hint">
        Strip secrets from outbound LLM payloads. Pick a mode below, then
        tune which detectors fire and add project-specific allow/deny
        patterns.
      </p>
      <Show when={error()}>
        <p class="settings-error">{error()}</p>
      </Show>
      <Show when={feedback()}>
        <p class="settings-success">{feedback()}</p>
      </Show>

      <h4 class="settings-subhead">Mode</h4>
      <div class="settings-row settings-row-stack">
        <label>Outbound redaction</label>
        <div class="settings-control-line">
          <Dropdown
            value={mode()}
            onChange={(v) =>
              void setConfig(
                "AICTL_SECURITY_REDACTION",
                v === "off" ? "" : v,
              )
            }
            options={[
              { value: "off", label: "Off — pass through unchanged" },
              { value: "redact", label: "Redact — replace matches with [REDACTED:<KIND>]" },
              { value: "block", label: "Block — abort the turn on any match" },
            ]}
          />
        </div>
        <p class="settings-hint">
          Off is the default. Redact lets the turn continue with secrets
          masked; Block aborts and surfaces the matched kinds.
        </p>
      </div>
      <BoolRow
        label="Apply redaction to local providers too"
        help="Off by default — Ollama / GGUF / MLX run on your machine, so the network-boundary argument doesn't apply."
        on={isOn("AICTL_SECURITY_REDACTION_LOCAL")}
        onChange={(v) =>
          void setConfig("AICTL_SECURITY_REDACTION_LOCAL", v ? "true" : "")
        }
      />

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
      <Show
        when={ner()}
        fallback={<p class="settings-hint">Loading NER status…</p>}
      >
        {(s) => (
          <>
            <p class="settings-meta">
              <Show
                when={s().inference_available}
                fallback={
                  <>
                    This build was not compiled with{" "}
                    <code>--features redaction-ner</code> — the model can
                    still be downloaded, but inference will be skipped at
                    runtime.
                  </>
                }
              >
                Inference enabled. Models live in <code>{s().dir}</code>.
              </Show>
            </p>
            <div class="settings-row settings-row-stack">
              <label>Model</label>
              <div class="settings-control-line">
                <code>{s().configured_model}</code>
              </div>
              <p class="settings-meta">
                <Show
                  when={s().configured_model_present}
                  fallback={<strong>Not downloaded</strong>}
                >
                  Downloaded · {fmtBytes(s().configured_model_size)}
                </Show>
              </p>
              <p class="settings-hint">
                Default model: <code>{s().default_spec}</code>. Override
                via <code>AICTL_REDACTION_NER_MODEL</code> in the General
                tab to pull a different gline-rs–compatible model.
              </p>
            </div>

            <Show when={nerDownload()}>
              {(d) => (
                <div class="settings-downloads">
                  <div class="settings-download-row">
                    <div class="settings-download-label">
                      {d().label}
                      <Show when={d().message}>
                        {(m) => <span class="settings-meta"> · {m()}</span>}
                      </Show>
                    </div>
                    <progress
                      class="settings-download-bar"
                      value={d().current}
                      max={d().total ?? undefined}
                    />
                    <div class="settings-download-meta">
                      {fmtBytes(d().current)}
                      <Show when={d().total}>
                        {(t) => <> / {fmtBytes(t())}</>}
                      </Show>
                    </div>
                  </div>
                </div>
              )}
            </Show>

            <div
              class="settings-keys-bulk"
              style={{ "margin-bottom": "var(--space-3)" }}
            >
              <Show when={!s().configured_model_present}>
                <button
                  type="button"
                  disabled={nerDownload() !== null}
                  onClick={() =>
                    setPendingNerDownload({
                      source: "button",
                      checkbox: null,
                    })
                  }
                >
                  Download NER model
                </button>
              </Show>
              <Show when={s().configured_model_present}>
                <button
                  type="button"
                  class="danger"
                  onClick={() => setPendingNerRemove(true)}
                >
                  Remove model
                </button>
              </Show>
            </div>

            {/* Inline checkbox rather than BoolRow so the cancel path
                can reset the DOM `.checked` property directly. Solid's
                one-way `checked={...}` binding only re-applies when the
                underlying signal changes, so declining the download
                without this would leave the box visually checked even
                though the config flag was never written. */}
            <div class="settings-row settings-row-stack">
              <div class="settings-bool-line">
                <label>
                  <input
                    type="checkbox"
                    checked={isOn("AICTL_REDACTION_NER")}
                    onChange={(e) => {
                      const target = e.currentTarget;
                      const v = target.checked;
                      // Only gate the on→true edge when the model isn't
                      // present yet; turning NER off, or flipping it on
                      // when the model is already downloaded, falls
                      // straight through to the config write.
                      if (v && !s().configured_model_present) {
                        setPendingNerDownload({
                          source: "checkbox",
                          checkbox: target,
                        });
                        return;
                      }
                      void setConfig("AICTL_REDACTION_NER", v ? "true" : "");
                    }}
                  />
                  <span>Enable NER (people, locations, organizations)</span>
                </label>
              </div>
              <p class="settings-hint">
                <Show
                  when={s().configured_model_present}
                  fallback={
                    <>
                      Download the NER model first — enabling without it
                      has no effect at runtime.
                    </>
                  }
                >
                  Layer C of redaction. Adds ~1 s of latency on the first
                  turn while the model loads.
                </Show>
              </p>
            </div>
          </>
        )}
      </Show>

      {/* Both prompts route through the in-app ConfirmDelete modal —
          window.confirm() is unreliable inside the Tauri webview. The
          download prompt covers both entry points (button click and
          enable-NER checkbox edge); the checkbox source stashes its
          DOM ref so cancel can snap `.checked` back to false. */}
      <Show when={pendingNerDownload() && ner()}>
        <ConfirmDelete
          title="Download NER model"
          detail={ner()!.default_spec}
          note="Pulls tokenizer.json and the ONNX weights from Hugging Face (~200 MB on disk for the default model). Required before the redaction NER pass can run."
          confirmLabel="Download"
          confirmVariant="allow"
          onCancel={() => {
            const p = pendingNerDownload();
            // Reset the checkbox DOM when the user backs out of the
            // checkbox-triggered prompt — otherwise the click leaves
            // it visually checked even though no config was written.
            if (p?.source === "checkbox" && p.checkbox) {
              p.checkbox.checked = false;
            }
            setEnableNerAfterPull(false);
            setPendingNerDownload(null);
          }}
          onConfirm={() => {
            const p = pendingNerDownload();
            setPendingNerDownload(null);
            // The flag stays off until the model actually lands on
            // disk. Snap the checkbox DOM back to false so the visual
            // state matches the un-written config; the final
            // progress_end handler will flip the flag on (and the
            // resource refetch re-checks the box) once the pull
            // completes.
            if (p?.source === "checkbox" && p.checkbox) {
              p.checkbox.checked = false;
            }
            // Remember the user's intent only for the checkbox path;
            // the bare Download button must not silently turn NER on.
            setEnableNerAfterPull(p?.source === "checkbox");
            startNerPull(ner()!.default_spec);
          }}
        />
      </Show>
      <Show when={pendingNerRemove() && ner()}>
        <ConfirmDelete
          title="Remove NER model"
          detail={ner()!.configured_model}
          note="The model directory will be deleted from disk. Enabling NER again will require re-downloading the model."
          onCancel={() => setPendingNerRemove(false)}
          onConfirm={() => {
            setPendingNerRemove(false);
            setError(null);
            setFeedback(null);
            const name = ner()!.configured_model;
            void (async () => {
              try {
                await ipc.nerRemove(name);
                if (isOn("AICTL_REDACTION_NER")) {
                  await setConfig("AICTL_REDACTION_NER", "");
                }
                await refetchNer();
                setFeedback(`removed ${name}`);
              } catch (err) {
                setError(`${err}`);
              }
            })();
          }}
        />
      </Show>
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
          <Dropdown
            value={theme()}
            onChange={(v) => void setConfig("AICTL_DESKTOP_THEME", v)}
            options={[
              { value: "dark", label: "Dark" },
              { value: "light", label: "Light" },
              { value: "system", label: "Follow system" },
            ]}
          />
        </div>
        <p class="settings-hint">
          Light theme is a higher-contrast variant of the brutalist palette.
        </p>
      </div>

      <h4 class="settings-subhead">Density</h4>
      <div class="settings-row settings-row-stack">
        <label>Chat density</label>
        <div class="settings-control-line">
          <Dropdown
            value={density()}
            onChange={(v) => void setConfig("AICTL_DESKTOP_DENSITY", v)}
            options={[
              { value: "comfortable", label: "Comfortable" },
              { value: "compact", label: "Compact" },
              { value: "cozy", label: "Cozy" },
            ]}
          />
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

interface AboutTabProps {
  onShowUpdate?: (info: UpdateInfo | null) => void;
}

const AboutTab: Component<AboutTabProps> = (props) => {
  const [version] = createResource<string>(() => ipc.version());
  const [profile] = createResource<"debug" | "release">(() =>
    ipc.buildProfile(),
  );
  const [buildTime] = createResource<string>(() => ipc.buildTime());
  const [buildCommit] = createResource<string>(() => ipc.buildCommit());
  // The About tab now drives the updater plugin's manifest probe
  // directly — same call the launch-time check uses, so what's
  // displayed here matches what the install flow will pull.
  const [updateCheck, setUpdateCheck] = createSignal<UpdateInfo | null>(null);
  const [checked, setChecked] = createSignal(false);
  const [checking, setChecking] = createSignal(false);
  const formattedBuildTime = (): string | null => {
    const raw = buildTime();
    if (!raw) return null;
    const secs = Number.parseInt(raw, 10);
    if (!Number.isFinite(secs) || secs <= 0) return null;
    return new Date(secs * 1000).toLocaleString();
  };
  const [error, setError] = createSignal<string | null>(null);
  const refreshVersion = async () => {
    setError(null);
    setChecking(true);
    try {
      setUpdateCheck(await checkUpdate());
      setChecked(true);
    } catch (err) {
      setError(`${err}`);
    } finally {
      setChecking(false);
    }
  };
  // Auto-check on mount so the user opening About sees the result
  // without a click. `checkUpdate` hits the GitHub Releases manifest;
  // a flaky network surfaces in `error` rather than blocking the tab.
  onMount(() => {
    void refreshVersion();
  });
  const latestLabel = (): string => {
    if (checking()) return "checking…";
    if (!checked()) return "—";
    const u = updateCheck();
    if (!u) return version() ?? "—";
    return `${u.version} (update available)`;
  };
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
        <label>Latest</label>
        <div class="settings-value settings-control-line">
          <code>{latestLabel()}</code>
          <button
            type="button"
            onClick={() => void refreshVersion()}
            disabled={checking()}
          >
            {checking() ? "checking…" : "Check now"}
          </button>
          <Show when={updateCheck() && props.onShowUpdate}>
            <button
              type="button"
              class="primary"
              onClick={() => props.onShowUpdate?.(updateCheck())}
            >
              Install update
            </button>
          </Show>
        </div>
      </div>
      <div class="settings-row">
        <label>Build</label>
        <div class="settings-value">
          <code>{profile() ?? "…"}</code>
        </div>
      </div>
      <div class="settings-row">
        <label>Built</label>
        <div class="settings-value">
          <code>{formattedBuildTime() ?? "…"}</code>
        </div>
      </div>
      <div class="settings-row">
        <label>Commit</label>
        <div class="settings-value">
          <code>{buildCommit() ?? "…"}</code>
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
          Piotr Wittchen |{" "}
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

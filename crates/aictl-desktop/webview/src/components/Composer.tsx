import type { Component } from "solid-js";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
} from "solid-js";
import { Portal } from "solid-js/web";

import {
  ipc,
  type ActiveModel,
  type AgentRow,
  type ModelEntry,
  type SkillRow,
} from "../lib/ipc";
import SecurityShield, {
  type ShieldCheck,
  type ShieldState,
} from "./SecurityShield";

interface Props {
  disabled: boolean;
  onSend: (text: string) => void | Promise<void>;
  autoAccept: boolean;
  onAutoAcceptChange: (next: boolean) => void;
  /// Mirror of `AICTL_TOOLS_ENABLED`. When `false` the composer hides
  /// the auto-accept dropdown — the agent runs chat-only so there are
  /// no tool calls to approve.
  toolsEnabled: boolean;
  /// Master tools toggle — flips `AICTL_TOOLS_ENABLED` and cascades
  /// to the web and image subset toggles so the icons reflect a
  /// single global on/off state.
  onToolsEnabledChange: (next: boolean) => Promise<void>;
  /// Plugins master switch (cube icon) — mirrors
  /// `AICTL_PLUGINS_ENABLED`. Flipping it reloads the engine's plugin
  /// catalogue immediately so the change applies to the current
  /// session as well as future ones.
  pluginsEnabled: boolean;
  onPluginsEnabledChange: (next: boolean) => Promise<void>;
  /// MCP master switch (server-rack icon) — mirrors
  /// `AICTL_MCP_ENABLED`. Same real-time reload semantics as the
  /// plugins toggle, but covers external MCP servers (stdio + remote)
  /// instead of locally-installed plugin tools.
  mcpEnabled: boolean;
  onMcpEnabledChange: (next: boolean) => Promise<void>;
  /// Set by the parent when /retry surfaces the previous prompt — the
  /// composer fills its textarea with the value and immediately calls
  /// `onPrefillConsumed` so the same prefill isn't reapplied on every
  /// re-render.
  prefill: string | null;
  onPrefillConsumed: () => void;
  /// Model picker state lives in `App` so the Settings overlay and the
  /// composer dropdown stay in sync — neither side owns the value.
  models: ModelEntry[];
  activeModel: ActiveModel;
  onChangeModel: (provider: string, model: string) => Promise<void>;
  /// Name of the skill currently pinned to every turn, or `null` when
  /// none is loaded. Owned by `App` so a window reload (which re-reads
  /// `skill_loaded` from the backend) survives without the picker
  /// re-fetching from disk.
  loadedSkill: string | null;
  onLoadedSkillChange: (next: string | null) => void;
  /// Same shape for the active agent. The engine keeps the body in a
  /// process-wide static; we only mirror the name for the icon's
  /// highlight state.
  loadedAgent: string | null;
  onLoadedAgentChange: (next: string | null) => void;
  /// Globe icon — single switch for the three web tools
  /// (`search_web`, `fetch_url`, `extract_website`). Active state =
  /// all three enabled. Toggling writes through to
  /// `AICTL_SECURITY_DISABLED_TOOLS` via the parent so the Settings
  /// → Tools panel reflects the change without a restart.
  webEnabled: boolean;
  onWebEnabledChange: (next: boolean) => Promise<void>;
  /// Picture icon — sibling toggle for the two image tools
  /// (`read_image`, `generate_image`). Same `AICTL_SECURITY_DISABLED_TOOLS`
  /// plumbing as the web toggle.
  imageEnabled: boolean;
  onImageEnabledChange: (next: boolean) => Promise<void>;
  /// Aggregated posture for the shield icon. `App` reads the relevant
  /// config keys + keyring presence and pushes the result down here so
  /// every other composer toggle keeps its single-source-of-truth shape.
  securityState: ShieldState;
  securityChecks: ShieldCheck[];
  /// Open Settings on the Security tab — used by the shield modal's
  /// "Open Settings" button. App owns the overlay state so the button
  /// just delegates upwards.
  onOpenSecuritySettings: () => void;
}

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

interface Group {
  provider: string;
  label: string;
  models: string[];
}

const groupModels = (entries: ModelEntry[]): Group[] => {
  const order: string[] = [];
  const buckets = new Map<string, string[]>();
  for (const e of entries) {
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
};

const Composer: Component<Props> = (props) => {
  const [text, setText] = createSignal("");
  const [pickerError, setPickerError] = createSignal<string | null>(null);
  // Transient flash next to the auto-accept toggle. Cleared after a
  // short delay so the message doesn't linger past its useful life.
  const [autoFlash, setAutoFlash] = createSignal<string | null>(null);
  let autoFlashTimer: number | undefined;

  // Skill picker — opens a dropdown of available skills next to the
  // bolt icon. The list is fetched lazily when the menu opens and
  // refreshed every open so newly-authored skills show up without a
  // restart.
  const [skillMenuOpen, setSkillMenuOpen] = createSignal(false);
  const [skillList, setSkillList] = createSignal<SkillRow[]>([]);
  const [skillError, setSkillError] = createSignal<string | null>(null);
  const [skillFlash, setSkillFlash] = createSignal<string | null>(null);
  let skillFlashTimer: number | undefined;
  let skillButtonRef: HTMLButtonElement | undefined;
  let skillMenuRef: HTMLDivElement | undefined;

  const flashSkill = (msg: string) => {
    if (skillFlashTimer !== undefined) {
      window.clearTimeout(skillFlashTimer);
    }
    setSkillFlash(msg);
    skillFlashTimer = window.setTimeout(() => setSkillFlash(null), 1800);
  };

  const refreshSkills = async () => {
    try {
      const rows = await ipc.skillsList();
      setSkillList(rows);
      setSkillError(null);
    } catch (err) {
      setSkillError(`${err}`);
    }
  };

  const openSkillMenu = () => {
    setSkillMenuOpen(true);
    void refreshSkills();
  };

  const closeSkillMenu = () => setSkillMenuOpen(false);

  const toggleSkillMenu = () => {
    if (props.disabled) return;
    if (skillMenuOpen()) {
      closeSkillMenu();
    } else {
      openSkillMenu();
    }
  };

  const selectSkill = async (name: string) => {
    closeSkillMenu();
    if (props.loadedSkill === name) {
      // Re-clicking the active skill unloads it so the icon doubles as
      // the deselect affordance — no separate "clear" entry needed.
      await unloadSkill();
      return;
    }
    try {
      await ipc.skillLoad(name);
      props.onLoadedSkillChange(name);
      flashSkill(`skill "${name}" loaded`);
    } catch (err) {
      flashSkill(`failed to load skill: ${err}`);
    }
  };

  const unloadSkill = async () => {
    const previous = props.loadedSkill;
    try {
      await ipc.skillUnload();
      props.onLoadedSkillChange(null);
      if (previous) {
        flashSkill(`skill "${previous}" unloaded`);
      } else {
        flashSkill("skill unloaded");
      }
    } catch (err) {
      flashSkill(`failed to unload skill: ${err}`);
    }
  };

  // Agent picker — same UX as the skill picker. Stored separately so a
  // user can have one of each loaded simultaneously.
  const [agentMenuOpen, setAgentMenuOpen] = createSignal(false);
  const [agentList, setAgentList] = createSignal<AgentRow[]>([]);
  const [agentError, setAgentError] = createSignal<string | null>(null);
  const [agentFlash, setAgentFlash] = createSignal<string | null>(null);
  let agentFlashTimer: number | undefined;
  let agentButtonRef: HTMLButtonElement | undefined;
  let agentMenuRef: HTMLDivElement | undefined;

  const flashAgent = (msg: string) => {
    if (agentFlashTimer !== undefined) {
      window.clearTimeout(agentFlashTimer);
    }
    setAgentFlash(msg);
    agentFlashTimer = window.setTimeout(() => setAgentFlash(null), 1800);
  };

  const refreshAgents = async () => {
    try {
      const rows = await ipc.agentsList();
      setAgentList(rows);
      setAgentError(null);
    } catch (err) {
      setAgentError(`${err}`);
    }
  };

  const openAgentMenu = () => {
    setAgentMenuOpen(true);
    void refreshAgents();
  };

  const closeAgentMenu = () => setAgentMenuOpen(false);

  const toggleAgentMenu = () => {
    if (props.disabled) return;
    if (agentMenuOpen()) {
      closeAgentMenu();
    } else {
      openAgentMenu();
    }
  };

  const selectAgent = async (name: string) => {
    closeAgentMenu();
    if (props.loadedAgent === name) {
      await unloadAgent();
      return;
    }
    try {
      await ipc.agentLoad(name);
      props.onLoadedAgentChange(name);
      flashAgent(`agent "${name}" loaded`);
    } catch (err) {
      flashAgent(`failed to load agent: ${err}`);
    }
  };

  const unloadAgent = async () => {
    const previous = props.loadedAgent;
    try {
      await ipc.agentUnload();
      props.onLoadedAgentChange(null);
      if (previous) {
        flashAgent(`agent "${previous}" unloaded`);
      } else {
        flashAgent("agent unloaded");
      }
    } catch (err) {
      flashAgent(`failed to unload agent: ${err}`);
    }
  };

  // Outside-click + Esc dismissal. Mirrors the model picker's behavior
  // so the menu doesn't trap the user.
  const onDocPointer = (e: MouseEvent) => {
    const target = e.target;
    if (!(target instanceof Node)) return;
    if (skillMenuOpen()) {
      const insideSkill =
        skillMenuRef?.contains(target) || skillButtonRef?.contains(target);
      if (!insideSkill) closeSkillMenu();
    }
    if (agentMenuOpen()) {
      const insideAgent =
        agentMenuRef?.contains(target) || agentButtonRef?.contains(target);
      if (!insideAgent) closeAgentMenu();
    }
  };
  const onDocKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    if (skillMenuOpen()) {
      e.preventDefault();
      closeSkillMenu();
    }
    if (agentMenuOpen()) {
      e.preventDefault();
      closeAgentMenu();
    }
  };
  document.addEventListener("mousedown", onDocPointer);
  document.addEventListener("keydown", onDocKey);
  onCleanup(() => {
    document.removeEventListener("mousedown", onDocPointer);
    document.removeEventListener("keydown", onDocKey);
    if (skillFlashTimer !== undefined) {
      window.clearTimeout(skillFlashTimer);
    }
    if (agentFlashTimer !== undefined) {
      window.clearTimeout(agentFlashTimer);
    }
    if (webFlashTimer !== undefined) {
      window.clearTimeout(webFlashTimer);
    }
    if (imageFlashTimer !== undefined) {
      window.clearTimeout(imageFlashTimer);
    }
    if (toolsFlashTimer !== undefined) {
      window.clearTimeout(toolsFlashTimer);
    }
    if (pluginsFlashTimer !== undefined) {
      window.clearTimeout(pluginsFlashTimer);
    }
    if (mcpFlashTimer !== undefined) {
      window.clearTimeout(mcpFlashTimer);
    }
  });

  // MCP toggle (server-rack icon) — flips AICTL_MCP_ENABLED via the
  // parent and reloads the engine's catalogue so spawned children stop
  // (or start) without an app restart.
  const [mcpFlash, setMcpFlash] = createSignal<string | null>(null);
  let mcpFlashTimer: number | undefined;
  const toggleMcp = async () => {
    if (props.disabled) return;
    const next = !props.mcpEnabled;
    try {
      await props.onMcpEnabledChange(next);
      if (mcpFlashTimer !== undefined) {
        window.clearTimeout(mcpFlashTimer);
      }
      setMcpFlash(next ? "MCP servers enabled" : "MCP servers disabled");
      mcpFlashTimer = window.setTimeout(() => setMcpFlash(null), 1800);
    } catch (err) {
      if (mcpFlashTimer !== undefined) {
        window.clearTimeout(mcpFlashTimer);
      }
      setMcpFlash(`failed to toggle MCP: ${err}`);
      mcpFlashTimer = window.setTimeout(() => setMcpFlash(null), 1800);
    }
  };

  // Plugins toggle (cube icon) — flips AICTL_PLUGINS_ENABLED via the
  // parent, which also reloads the engine's plugin catalogue so the
  // change applies in real time. Same flash pattern as the siblings.
  const [pluginsFlash, setPluginsFlash] = createSignal<string | null>(null);
  let pluginsFlashTimer: number | undefined;
  const togglePlugins = async () => {
    if (props.disabled) return;
    const next = !props.pluginsEnabled;
    try {
      await props.onPluginsEnabledChange(next);
      if (pluginsFlashTimer !== undefined) {
        window.clearTimeout(pluginsFlashTimer);
      }
      setPluginsFlash(next ? "plugins enabled" : "plugins disabled");
      pluginsFlashTimer = window.setTimeout(() => setPluginsFlash(null), 1800);
    } catch (err) {
      if (pluginsFlashTimer !== undefined) {
        window.clearTimeout(pluginsFlashTimer);
      }
      setPluginsFlash(`failed to toggle plugins: ${err}`);
      pluginsFlashTimer = window.setTimeout(() => setPluginsFlash(null), 1800);
    }
  };

  // Master tools toggle — flips AICTL_TOOLS_ENABLED via the parent and
  // cascades to web + image subset toggles so the three icons share a
  // single on/off semantic. Same flash pattern as the siblings.
  const [toolsFlash, setToolsFlash] = createSignal<string | null>(null);
  let toolsFlashTimer: number | undefined;
  const toggleTools = async () => {
    if (props.disabled) return;
    const next = !props.toolsEnabled;
    try {
      await props.onToolsEnabledChange(next);
      if (toolsFlashTimer !== undefined) {
        window.clearTimeout(toolsFlashTimer);
      }
      setToolsFlash(next ? "tools enabled" : "tools disabled");
      toolsFlashTimer = window.setTimeout(() => setToolsFlash(null), 1800);
    } catch (err) {
      if (toolsFlashTimer !== undefined) {
        window.clearTimeout(toolsFlashTimer);
      }
      setToolsFlash(`failed to toggle tools: ${err}`);
      toolsFlashTimer = window.setTimeout(() => setToolsFlash(null), 1800);
    }
  };

  // Globe toggle — flips all three web tools as one unit. Same toast
  // pattern as the auto-accept button so feedback feels consistent.
  const [webFlash, setWebFlash] = createSignal<string | null>(null);
  let webFlashTimer: number | undefined;
  const toggleWeb = async () => {
    if (props.disabled || !props.toolsEnabled) return;
    const next = !props.webEnabled;
    try {
      await props.onWebEnabledChange(next);
      if (webFlashTimer !== undefined) {
        window.clearTimeout(webFlashTimer);
      }
      setWebFlash(next ? "web tools enabled" : "web tools disabled");
      webFlashTimer = window.setTimeout(() => setWebFlash(null), 1800);
    } catch (err) {
      if (webFlashTimer !== undefined) {
        window.clearTimeout(webFlashTimer);
      }
      setWebFlash(`failed to toggle web tools: ${err}`);
      webFlashTimer = window.setTimeout(() => setWebFlash(null), 1800);
    }
  };

  // Picture toggle — same shape as the globe; flips both image tools.
  const [imageFlash, setImageFlash] = createSignal<string | null>(null);
  let imageFlashTimer: number | undefined;
  const toggleImage = async () => {
    if (props.disabled || !props.toolsEnabled) return;
    const next = !props.imageEnabled;
    try {
      await props.onImageEnabledChange(next);
      if (imageFlashTimer !== undefined) {
        window.clearTimeout(imageFlashTimer);
      }
      setImageFlash(next ? "image tools enabled" : "image tools disabled");
      imageFlashTimer = window.setTimeout(() => setImageFlash(null), 1800);
    } catch (err) {
      if (imageFlashTimer !== undefined) {
        window.clearTimeout(imageFlashTimer);
      }
      setImageFlash(`failed to toggle image tools: ${err}`);
      imageFlashTimer = window.setTimeout(() => setImageFlash(null), 1800);
    }
  };

  const toggleAutoAccept = () => {
    if (props.disabled || !props.toolsEnabled) return;
    const next = !props.autoAccept;
    props.onAutoAcceptChange(next);
    if (autoFlashTimer !== undefined) {
      window.clearTimeout(autoFlashTimer);
    }
    setAutoFlash(next ? "auto-accept enabled" : "human-in-the-loop");
    autoFlashTimer = window.setTimeout(() => setAutoFlash(null), 1800);
  };

  const groups = createMemo(() => groupModels(props.models));

  const encode = (provider: string, model: string) => `${provider}|${model}`;

  const activeKey = createMemo(() => {
    const a = props.activeModel;
    return a.provider && a.model ? encode(a.provider, a.model) : "";
  });

  createEffect(() => {
    const value = props.prefill;
    if (value !== null) {
      setText(value);
      props.onPrefillConsumed();
    }
  });

  const submit = async () => {
    if (props.disabled) return;
    const value = text().trim();
    if (!value) return;
    setText("");
    await props.onSend(value);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    // ⌘↩ — send. Plain ↩ inserts a newline so users can compose
    // multi-line prompts without fighting the chat surface.
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void submit();
    }
  };

  const onModelChange = async (e: Event & { currentTarget: HTMLSelectElement }) => {
    const value = e.currentTarget.value;
    if (!value) return;
    const sep = value.indexOf("|");
    if (sep < 0) return;
    const provider = value.slice(0, sep);
    const model = value.slice(sep + 1);
    setPickerError(null);
    try {
      await props.onChangeModel(provider, model);
    } catch (err) {
      setPickerError(`${err}`);
    }
  };

  return (
    <div class="composer">
      <textarea
        placeholder={
          props.disabled ? "Pick a workspace to start chatting…" : "Type a message"
        }
        value={text()}
        disabled={props.disabled}
        onInput={(e) => setText(e.currentTarget.value)}
        onKeyDown={onKeyDown}
      />
      <div class="footer">
        <select
          class="model-picker"
          value={activeKey()}
          onChange={onModelChange}
          disabled={props.disabled}
          title={pickerError() ?? "Switch active model"}
        >
          <Show when={!activeKey()}>
            <option value="" disabled>
              select model…
            </option>
          </Show>
          <For each={groups()}>
            {(group) => (
              <optgroup label={group.label}>
                <For each={group.models}>
                  {(model) => (
                    <option value={encode(group.provider, model)}>{model}</option>
                  )}
                </For>
              </optgroup>
            )}
          </For>
        </select>
        <SecurityShield
          state={props.securityState}
          checks={props.securityChecks}
          disabled={props.disabled}
          onOpenSettings={props.onOpenSecuritySettings}
        />
        <button
          type="button"
          class="agent-icon"
          ref={(el) => (agentButtonRef = el)}
          data-active={String(props.loadedAgent !== null)}
          disabled={props.disabled}
          aria-haspopup="menu"
          aria-expanded={agentMenuOpen() ? "true" : "false"}
          aria-label={
            props.loadedAgent
              ? `Agent "${props.loadedAgent}" loaded — click to change or unload`
              : "Load an agent"
          }
          title={
            props.loadedAgent
              ? `agent "${props.loadedAgent}" loaded — click to change or unload`
              : "load an agent"
          }
          onClick={toggleAgentMenu}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 20 20"
            fill="currentColor"
            aria-hidden="true"
          >
            <path d="M15.98 1.804a1 1 0 0 0-1.96 0l-.24 1.192a1 1 0 0 1-.784.785l-1.192.238a1 1 0 0 0 0 1.962l1.192.238a1 1 0 0 1 .785.785l.238 1.192a1 1 0 0 0 1.962 0l.238-1.192a1 1 0 0 1 .785-.785l1.192-.238a1 1 0 0 0 0-1.962l-1.192-.238a1 1 0 0 1-.785-.785l-.238-1.192ZM6.949 5.684a1 1 0 0 0-1.898 0l-.683 2.051a1 1 0 0 1-.633.633l-2.051.683a1 1 0 0 0 0 1.898l2.051.684a1 1 0 0 1 .633.632l.683 2.051a1 1 0 0 0 1.898 0l.683-2.051a1 1 0 0 1 .633-.633l2.051-.683a1 1 0 0 0 0-1.898l-2.051-.683a1 1 0 0 1-.633-.633L6.95 5.684ZM13.949 13.684a1 1 0 0 0-1.898 0l-.184.551a1 1 0 0 1-.632.633l-.551.183a1 1 0 0 0 0 1.898l.551.183a1 1 0 0 1 .633.633l.183.551a1 1 0 0 0 1.898 0l.184-.551a1 1 0 0 1 .632-.633l.551-.183a1 1 0 0 0 0-1.898l-.551-.184a1 1 0 0 1-.633-.632l-.183-.551Z" />
          </svg>
        </button>
        <Show when={agentMenuOpen()}>
          <div
            class="skill-menu agent-menu"
            role="menu"
            ref={(el) => (agentMenuRef = el)}
          >
            <div class="skill-menu-header">
              <span>agents</span>
              <Show when={props.loadedAgent}>
                <button
                  type="button"
                  class="skill-menu-clear"
                  onClick={() => void unloadAgent()}
                >
                  unload
                </button>
              </Show>
            </div>
            <Show when={agentError()}>
              <div class="skill-menu-error">{agentError()}</div>
            </Show>
            <Show
              when={agentList().length > 0}
              fallback={
                <Show when={!agentError()}>
                  <div class="skill-menu-empty">no agents installed</div>
                </Show>
              }
            >
              <ul class="skill-menu-list">
                <For each={agentList()}>
                  {(a) => (
                    <li>
                      <button
                        type="button"
                        class="skill-menu-item"
                        role="menuitemradio"
                        data-active={String(props.loadedAgent === a.name)}
                        aria-checked={
                          props.loadedAgent === a.name ? "true" : "false"
                        }
                        onClick={() => void selectAgent(a.name)}
                      >
                        <span class="skill-menu-item-name">{a.name}</span>
                        <Show when={a.description}>
                          <span class="skill-menu-item-desc">
                            {a.description}
                          </span>
                        </Show>
                      </button>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </div>
        </Show>
        <Show when={agentFlash()}>
          {(msg) => (
            <Portal mount={document.body}>
              <div class="auto-accept-toast" role="status" aria-live="polite">
                <div class="panel">{msg()}</div>
              </div>
            </Portal>
          )}
        </Show>
        <button
          type="button"
          class="skill-icon"
          ref={(el) => (skillButtonRef = el)}
          data-active={String(props.loadedSkill !== null)}
          disabled={props.disabled}
          aria-haspopup="menu"
          aria-expanded={skillMenuOpen() ? "true" : "false"}
          aria-label={
            props.loadedSkill
              ? `Skill "${props.loadedSkill}" loaded — click to change or unload`
              : "Load a skill"
          }
          title={
            props.loadedSkill
              ? `skill "${props.loadedSkill}" loaded — click to change or unload`
              : "load a skill"
          }
          onClick={toggleSkillMenu}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 20 20"
            fill="currentColor"
            aria-hidden="true"
          >
            <path d="M11.983 1.907a.75.75 0 0 0-1.292-.657l-8.5 9.5A.75.75 0 0 0 2.75 12h6.572l-1.305 6.093a.75.75 0 0 0 1.292.657l8.5-9.5A.75.75 0 0 0 17.25 8h-6.572l1.305-6.093Z" />
          </svg>
        </button>
        <Show when={skillMenuOpen()}>
          <div
            class="skill-menu"
            role="menu"
            ref={(el) => (skillMenuRef = el)}
          >
            <div class="skill-menu-header">
              <span>skills</span>
              <Show when={props.loadedSkill}>
                <button
                  type="button"
                  class="skill-menu-clear"
                  onClick={() => void unloadSkill()}
                >
                  unload
                </button>
              </Show>
            </div>
            <Show when={skillError()}>
              <div class="skill-menu-error">{skillError()}</div>
            </Show>
            <Show
              when={skillList().length > 0}
              fallback={
                <Show when={!skillError()}>
                  <div class="skill-menu-empty">no skills installed</div>
                </Show>
              }
            >
              <ul class="skill-menu-list">
                <For each={skillList()}>
                  {(s) => (
                    <li>
                      <button
                        type="button"
                        class="skill-menu-item"
                        role="menuitemradio"
                        data-active={String(props.loadedSkill === s.name)}
                        aria-checked={
                          props.loadedSkill === s.name ? "true" : "false"
                        }
                        onClick={() => void selectSkill(s.name)}
                      >
                        <span class="skill-menu-item-name">{s.name}</span>
                        <Show when={s.description}>
                          <span class="skill-menu-item-desc">
                            {s.description}
                          </span>
                        </Show>
                      </button>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </div>
        </Show>
        <Show when={skillFlash()}>
          {(msg) => (
            <Portal mount={document.body}>
              <div class="auto-accept-toast" role="status" aria-live="polite">
                <div class="panel">{msg()}</div>
              </div>
            </Portal>
          )}
        </Show>
        <button
          type="button"
          class="mcp-icon"
          data-active={String(props.mcpEnabled)}
          disabled={props.disabled}
          aria-pressed={props.mcpEnabled ? "true" : "false"}
          aria-label={
            props.mcpEnabled
              ? "MCP servers enabled (click to disable every Model Context Protocol server)"
              : "MCP servers disabled (click to re-enable)"
          }
          title={
            props.mcpEnabled
              ? "MCP servers enabled — click to disable"
              : "MCP servers disabled — click to enable"
          }
          onClick={() => void toggleMcp()}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 16 17"
            fill="currentColor"
            aria-hidden="true"
          >
            <path
              fill-rule="evenodd"
              d="m14.557 8.468l-.055.054l-5.804 5.691a.183.183 0 0 0-.003.259l.003.003l1.192 1.17a.55.55 0 0 1 .011.776l-.01.01a.575.575 0 0 1-.803 0l-1.192-1.168a1.28 1.28 0 0 1 0-1.836l5.805-5.692a1.647 1.647 0 0 0 .031-2.328l-.031-.032l-.034-.032a1.725 1.725 0 0 0-2.405-.002l-4.781 4.69h-.002l-.065.065a.575.575 0 0 1-.803 0a.55.55 0 0 1-.01-.776l.01-.01l4.849-4.756c.65-.636.663-1.678.027-2.329l-.029-.03a1.725 1.725 0 0 0-2.407 0L1.635 8.489a.575.575 0 0 1-.802 0a.55.55 0 0 1-.011-.776l.011-.01L7.25 1.407a2.875 2.875 0 0 1 4.01 0c.63.613.929 1.49.803 2.36c.88-.125 1.77.166 2.406.787l.034.033a2.743 2.743 0 0 1 .053 3.88m-1.691-1.553a.55.55 0 0 0 .01-.776l-.01-.01a.575.575 0 0 0-.803 0l-4.746 4.654a1.725 1.725 0 0 1-2.407 0a1.647 1.647 0 0 1 0-2.36l4.747-4.655a.55.55 0 0 0 .011-.776l-.011-.01a.575.575 0 0 0-.803 0L4.108 7.635a2.743 2.743 0 0 0 0 3.933a2.876 2.876 0 0 0 4.011 0z"
              clip-rule="evenodd"
            />
          </svg>
        </button>
        <Show when={mcpFlash()}>
          {(msg) => (
            <Portal mount={document.body}>
              <div class="auto-accept-toast" role="status" aria-live="polite">
                <div class="panel">{msg()}</div>
              </div>
            </Portal>
          )}
        </Show>
        <button
          type="button"
          class="plugins-icon"
          data-active={String(props.pluginsEnabled)}
          disabled={props.disabled}
          aria-pressed={props.pluginsEnabled ? "true" : "false"}
          aria-label={
            props.pluginsEnabled
              ? "Plugins enabled (click to disable user plugin tools)"
              : "Plugins disabled (click to re-enable user plugin tools)"
          }
          title={
            props.pluginsEnabled
              ? "plugins enabled — click to disable"
              : "plugins disabled — click to enable"
          }
          onClick={() => void togglePlugins()}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 16 16"
            fill="currentColor"
            aria-hidden="true"
          >
            <path d="M8.372 1.349a.75.75 0 0 0-.744 0l-4.81 2.748L8 7.131l5.182-3.034-4.81-2.748ZM14 5.357 8.75 8.43v6.005l4.872-2.784A.75.75 0 0 0 14 11V5.357ZM7.25 14.435V8.43L2 5.357V11c0 .27.144.518.378.651l4.872 2.784Z" />
          </svg>
        </button>
        <Show when={pluginsFlash()}>
          {(msg) => (
            <Portal mount={document.body}>
              <div class="auto-accept-toast" role="status" aria-live="polite">
                <div class="panel">{msg()}</div>
              </div>
            </Portal>
          )}
        </Show>
        <button
          type="button"
          class="tools-icon"
          data-active={String(props.toolsEnabled)}
          disabled={props.disabled}
          aria-pressed={props.toolsEnabled ? "true" : "false"}
          aria-label={
            props.toolsEnabled
              ? "All tools enabled (click to disable every tool)"
              : "All tools disabled (click to re-enable)"
          }
          title={
            props.toolsEnabled
              ? "all tools enabled — click to disable"
              : "all tools disabled — click to enable"
          }
          onClick={() => void toggleTools()}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 16 16"
            fill="currentColor"
            aria-hidden="true"
          >
            <path
              fill-rule="evenodd"
              d="M15 4.5A3.5 3.5 0 0 1 11.435 8c-.99-.019-2.093.132-2.7.913l-4.13 5.31a2.015 2.015 0 1 1-2.827-2.828l5.309-4.13c.78-.607.932-1.71.914-2.7L8 4.5a3.5 3.5 0 0 1 4.477-3.362c.325.094.39.497.15.736L10.6 3.902a.48.48 0 0 0-.033.653c.271.314.565.608.879.879a.48.48 0 0 0 .653-.033l2.027-2.027c.239-.24.642-.175.736.15.09.31.138.637.138.976ZM3.75 13a.75.75 0 1 1-1.5 0 .75.75 0 0 1 1.5 0Z"
              clip-rule="evenodd"
            />
            <path d="M11.5 9.5c.313 0 .62-.029.917-.084l1.962 1.962a2.121 2.121 0 0 1-3 3l-2.81-2.81 1.35-1.734c.05-.064.158-.158.426-.233.278-.078.639-.11 1.062-.102l.093.001ZM5 4l1.446 1.445a2.256 2.256 0 0 1-.047.21c-.075.268-.169.377-.233.427l-.61.474L4 5H2.655a.25.25 0 0 1-.224-.139l-1.35-2.7a.25.25 0 0 1 .047-.289l.745-.745a.25.25 0 0 1 .289-.047l2.7 1.35A.25.25 0 0 1 5 2.654V4Z" />
          </svg>
        </button>
        <Show when={toolsFlash()}>
          {(msg) => (
            <Portal mount={document.body}>
              <div class="auto-accept-toast" role="status" aria-live="polite">
                <div class="panel">{msg()}</div>
              </div>
            </Portal>
          )}
        </Show>
        <button
          type="button"
          class="image-icon"
          data-active={String(props.imageEnabled)}
          disabled={props.disabled || !props.toolsEnabled}
          aria-pressed={props.imageEnabled ? "true" : "false"}
          aria-label={
            props.imageEnabled
              ? "Image tools enabled (click to disable read_image, generate_image)"
              : "Image tools disabled (click to enable read_image, generate_image)"
          }
          title={
            !props.toolsEnabled
              ? "tools master switch is off — enable it to use image tools"
              : props.imageEnabled
                ? "image tools enabled — click to disable"
                : "image tools disabled — click to enable"
          }
          onClick={() => void toggleImage()}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 16 16"
            fill="currentColor"
            aria-hidden="true"
          >
            <path
              fill-rule="evenodd"
              d="M2 4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V4Zm10.5 5.707a.5.5 0 0 0-.146-.353l-1-1a.5.5 0 0 0-.708 0L9.354 9.646a.5.5 0 0 1-.708 0L6.354 7.354a.5.5 0 0 0-.708 0l-2 2a.5.5 0 0 0-.146.353V12a.5.5 0 0 0 .5.5h8a.5.5 0 0 0 .5-.5V9.707ZM12 5a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z"
              clip-rule="evenodd"
            />
          </svg>
        </button>
        <Show when={imageFlash()}>
          {(msg) => (
            <Portal mount={document.body}>
              <div class="auto-accept-toast" role="status" aria-live="polite">
                <div class="panel">{msg()}</div>
              </div>
            </Portal>
          )}
        </Show>
        <button
          type="button"
          class="web-icon"
          data-active={String(props.webEnabled)}
          disabled={props.disabled || !props.toolsEnabled}
          aria-pressed={props.webEnabled ? "true" : "false"}
          aria-label={
            props.webEnabled
              ? "Web tools enabled (click to disable search_web, fetch_url, extract_website)"
              : "Web tools disabled (click to enable search_web, fetch_url, extract_website)"
          }
          title={
            !props.toolsEnabled
              ? "tools master switch is off — enable it to use web tools"
              : props.webEnabled
                ? "web tools enabled — click to disable"
                : "web tools disabled — click to enable"
          }
          onClick={() => void toggleWeb()}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 16 16"
            fill="currentColor"
            aria-hidden="true"
          >
            <path
              fill-rule="evenodd"
              d="M3.757 4.5c.18.217.376.42.586.608.153-.61.354-1.175.596-1.678A5.53 5.53 0 0 0 3.757 4.5ZM8 1a6.994 6.994 0 0 0-7 7 7 7 0 1 0 7-7Zm0 1.5c-.476 0-1.091.386-1.633 1.427-.293.564-.531 1.267-.683 2.063A5.48 5.48 0 0 0 8 6.5a5.48 5.48 0 0 0 2.316-.51c-.152-.796-.39-1.499-.683-2.063C9.09 2.886 8.476 2.5 8 2.5Zm3.657 2.608a8.823 8.823 0 0 0-.596-1.678c.444.298.842.659 1.182 1.07-.18.217-.376.42-.586.608Zm-1.166 2.436A6.983 6.983 0 0 1 8 8a6.983 6.983 0 0 1-2.49-.456 10.703 10.703 0 0 0 .202 2.6c.72.231 1.49.356 2.288.356.798 0 1.568-.125 2.29-.356a10.705 10.705 0 0 0 .2-2.6Zm1.433 1.85a12.652 12.652 0 0 0 .018-2.609c.405-.276.78-.594 1.117-.947a5.48 5.48 0 0 1 .44 2.262 7.536 7.536 0 0 1-1.575 1.293Zm-2.172 2.435a9.046 9.046 0 0 1-3.504 0c.039.084.078.166.12.244C6.907 13.114 7.523 13.5 8 13.5s1.091-.386 1.633-1.427c.04-.078.08-.16.12-.244Zm1.31.74a8.5 8.5 0 0 0 .492-1.298c.457-.197.893-.43 1.307-.696a5.526 5.526 0 0 1-1.8 1.995Zm-6.123 0a8.507 8.507 0 0 1-.493-1.298 8.985 8.985 0 0 1-1.307-.696 5.526 5.526 0 0 0 1.8 1.995ZM2.5 8.1c.463.5.993.935 1.575 1.293a12.652 12.652 0 0 1-.018-2.608 7.037 7.037 0 0 1-1.117-.947 5.48 5.48 0 0 0-.44 2.262Z"
              clip-rule="evenodd"
            />
          </svg>
        </button>
        <Show when={webFlash()}>
          {(msg) => (
            <Portal mount={document.body}>
              <div class="auto-accept-toast" role="status" aria-live="polite">
                <div class="panel">{msg()}</div>
              </div>
            </Portal>
          )}
        </Show>
        <button
          type="button"
          class="auto-accept-icon"
          data-active={String(props.autoAccept)}
          disabled={props.disabled || !props.toolsEnabled}
          aria-pressed={props.autoAccept ? "true" : "false"}
          aria-label={
            props.autoAccept
              ? "Auto-accept tools (click to disable)"
              : "Human-in-the-loop (click to auto-accept)"
          }
          title={
            !props.toolsEnabled
              ? "tools master switch is off — enable it to use auto-accept"
              : props.autoAccept
                ? "auto-accept enabled — click for human-in-the-loop"
                : "human-in-the-loop — click to auto-accept tools"
          }
          onClick={toggleAutoAccept}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 20 20"
            fill="currentColor"
            aria-hidden="true"
          >
            <path
              fill-rule="evenodd"
              d="M10 4.5c1.215 0 2.417.055 3.604.162a.68.68 0 0 1 .615.597c.124 1.038.208 2.088.25 3.15l-1.689-1.69a.75.75 0 0 0-1.06 1.061l2.999 3a.75.75 0 0 0 1.06 0l3.001-3a.75.75 0 1 0-1.06-1.06l-1.748 1.747a41.31 41.31 0 0 0-.264-3.386 2.18 2.18 0 0 0-1.97-1.913 41.512 41.512 0 0 0-7.477 0 2.18 2.18 0 0 0-1.969 1.913 41.16 41.16 0 0 0-.16 1.61.75.75 0 1 0 1.495.12c.041-.52.093-1.038.154-1.552a.68.68 0 0 1 .615-.597A40.012 40.012 0 0 1 10 4.5ZM5.281 9.22a.75.75 0 0 0-1.06 0l-3.001 3a.75.75 0 1 0 1.06 1.06l1.748-1.747c.042 1.141.13 2.27.264 3.386a2.18 2.18 0 0 0 1.97 1.913 41.533 41.533 0 0 0 7.477 0 2.18 2.18 0 0 0 1.969-1.913c.064-.534.117-1.071.16-1.61a.75.75 0 1 0-1.495-.12c-.041.52-.093 1.037-.154 1.552a.68.68 0 0 1-.615.597 40.013 40.013 0 0 1-7.208 0 .68.68 0 0 1-.615-.597 39.785 39.785 0 0 1-.25-3.15l1.689 1.69a.75.75 0 0 0 1.06-1.061l-2.999-3Z"
              clip-rule="evenodd"
            />
          </svg>
        </button>
        <Show when={autoFlash()}>
          {(msg) => (
            <Portal mount={document.body}>
              <div class="auto-accept-toast" role="status" aria-live="polite">
                <div class="panel">{msg()}</div>
              </div>
            </Portal>
          )}
        </Show>
        <button type="button" disabled={props.disabled} onClick={submit}>
          Send{" "}
          <kbd>
            ⌘
            <svg
              xmlns="http://www.w3.org/2000/svg"
              viewBox="0 0 16 16"
              fill="currentColor"
              aria-hidden="true"
            >
              <path
                fill-rule="evenodd"
                d="M13.25 2a.75.75 0 0 0-.75.75v6.5H4.56l.97-.97a.75.75 0 0 0-1.06-1.06L2.22 9.47a.75.75 0 0 0 0 1.06l2.25 2.25a.75.75 0 0 0 1.06-1.06l-.97-.97h8.69A.75.75 0 0 0 14 10V2.75a.75.75 0 0 0-.75-.75Z"
                clip-rule="evenodd"
              />
            </svg>
          </kbd>
        </button>
      </div>
    </div>
  );
};

export default Composer;

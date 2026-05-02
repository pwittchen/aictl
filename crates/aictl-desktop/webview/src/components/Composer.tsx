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

interface Props {
  disabled: boolean;
  onSend: (text: string) => void | Promise<void>;
  autoAccept: boolean;
  onAutoAcceptChange: (next: boolean) => void;
  /// Mirror of `AICTL_TOOLS_ENABLED`. When `false` the composer hides
  /// the auto-accept dropdown — the agent runs chat-only so there are
  /// no tool calls to approve.
  toolsEnabled: boolean;
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
  });

  const toggleAutoAccept = () => {
    if (props.disabled) return;
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
        <Show when={props.toolsEnabled}>
          <button
            type="button"
            class="auto-accept-icon"
            data-active={String(props.autoAccept)}
            disabled={props.disabled}
            aria-pressed={props.autoAccept ? "true" : "false"}
            aria-label={
              props.autoAccept
                ? "Auto-accept tools (click to disable)"
                : "Human-in-the-loop (click to auto-accept)"
            }
            title={
              props.autoAccept
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
        </Show>
        <button type="button" disabled={props.disabled} onClick={submit}>
          Send <kbd>⌘↩</kbd>
        </button>
      </div>
    </div>
  );
};

export default Composer;

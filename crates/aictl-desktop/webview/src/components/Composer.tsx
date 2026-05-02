import type { Component } from "solid-js";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { Portal } from "solid-js/web";

import type { ActiveModel, ModelEntry } from "../lib/ipc";

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
              viewBox="0 0 24 24"
              fill="currentColor"
              aria-hidden="true"
            >
              <path
                fill-rule="evenodd"
                d="M12 5.25c1.213 0 2.415.046 3.605.135a3.256 3.256 0 0 1 3.01 3.01c.044.583.077 1.17.1 1.759L17.03 8.47a.75.75 0 1 0-1.06 1.06l3 3a.75.75 0 0 0 1.06 0l3-3a.75.75 0 0 0-1.06-1.06l-1.752 1.751c-.023-.65-.06-1.296-.108-1.939a4.756 4.756 0 0 0-4.392-4.392 49.422 49.422 0 0 0-7.436 0A4.756 4.756 0 0 0 3.89 8.282c-.017.224-.033.447-.046.672a.75.75 0 1 0 1.497.092c.013-.217.028-.434.044-.651a3.256 3.256 0 0 1 3.01-3.01c1.19-.09 2.392-.135 3.605-.135Zm-6.97 6.22a.75.75 0 0 0-1.06 0l-3 3a.75.75 0 1 0 1.06 1.06l1.752-1.751c.023.65.06 1.296.108 1.939a4.756 4.756 0 0 0 4.392 4.392 49.413 49.413 0 0 0 7.436 0 4.756 4.756 0 0 0 4.392-4.392c.017-.223.032-.447.046-.672a.75.75 0 0 0-1.497-.092c-.013.217-.028.434-.044.651a3.256 3.256 0 0 1-3.01 3.01 47.953 47.953 0 0 1-7.21 0 3.256 3.256 0 0 1-3.01-3.01 47.759 47.759 0 0 1-.1-1.759L6.97 15.53a.75.75 0 0 0 1.06-1.06l-3-3Z"
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

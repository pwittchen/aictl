import type { Component } from "solid-js";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";

import type { ActiveModel, ModelEntry } from "../lib/ipc";

interface Props {
  disabled: boolean;
  onSend: (text: string) => void | Promise<void>;
  autoAccept: boolean;
  onAutoAcceptChange: (next: boolean) => void;
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
        <select
          class="auto-accept"
          value={props.autoAccept ? "auto" : "ask"}
          onChange={(e) => props.onAutoAcceptChange(e.currentTarget.value === "auto")}
          disabled={props.disabled}
          title="Choose whether tool calls auto-approve or ask for confirmation."
        >
          <option value="ask">Ask for tools</option>
          <option value="auto">Auto-accept tools</option>
        </select>
        <button type="button" disabled={props.disabled} onClick={submit}>
          Send <kbd>⌘↩</kbd>
        </button>
      </div>
    </div>
  );
};

export default Composer;

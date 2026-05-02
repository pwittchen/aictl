import type { Component } from "solid-js";
import { For, Show, createMemo, createSignal, onMount } from "solid-js";

import { ipc, type ModelEntry } from "../lib/ipc";

interface Props {
  disabled: boolean;
  onSend: (text: string) => void | Promise<void>;
  autoAccept: boolean;
  onAutoAcceptChange: (next: boolean) => void;
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
  const [models, setModels] = createSignal<ModelEntry[]>([]);
  const [active, setActive] = createSignal<string>("");
  const [pickerError, setPickerError] = createSignal<string | null>(null);

  const groups = createMemo(() => groupModels(models()));

  const encode = (provider: string, model: string) => `${provider}|${model}`;

  onMount(async () => {
    try {
      const [list, current] = await Promise.all([
        ipc.listModels(),
        ipc.getActiveModel(),
      ]);
      setModels(list);
      if (current.provider && current.model) {
        setActive(encode(current.provider, current.model));
      }
    } catch (err) {
      setPickerError(`${err}`);
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
    const previous = active();
    setActive(value);
    setPickerError(null);
    try {
      await ipc.setActiveModel(provider, model);
    } catch (err) {
      setActive(previous);
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
          value={active()}
          onChange={onModelChange}
          title={pickerError() ?? "Switch active model"}
        >
          <Show when={!active()}>
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

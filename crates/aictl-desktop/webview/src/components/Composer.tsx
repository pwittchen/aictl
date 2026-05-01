import type { Component } from "solid-js";
import { createSignal } from "solid-js";

interface Props {
  disabled: boolean;
  onSend: (text: string) => void | Promise<void>;
}

const Composer: Component<Props> = (props) => {
  const [text, setText] = createSignal("");

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
        <span>aictl · same engine, same config as the CLI</span>
        <button type="button" disabled={props.disabled} onClick={submit}>
          Send <kbd>⌘↩</kbd>
        </button>
      </div>
    </div>
  );
};

export default Composer;

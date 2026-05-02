import type { Component } from "solid-js";
import { For, Show, createEffect, createMemo } from "solid-js";

import type { Message } from "../App";
import { renderMarkdown } from "../lib/markdown";

interface Props {
  messages: Message[];
  streamingText: string;
  streaming: boolean;
  busy: boolean;
}

const Chat: Component<Props> = (props) => {
  let scroller: HTMLDivElement | undefined;

  // Auto-scroll on every message / stream chunk change. Solid effects
  // re-run whenever any tracked signal upstream updates.
  createEffect(() => {
    void props.messages.length;
    void props.streamingText;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  });

  const streamingHtml = createMemo(() =>
    props.streaming ? renderMarkdown(props.streamingText) : "",
  );

  return (
    <div class="message-list" ref={scroller}>
      <For each={props.messages}>
        {(m) => <MessageView msg={m} />}
      </For>
      <Show when={props.streaming}>
        <div class="message" data-role="assistant">
          <div class="meta">assistant · streaming</div>
          <div class="body markdown" innerHTML={streamingHtml()} />
        </div>
      </Show>
      <Show when={props.busy && !props.streaming}>
        <div class="message" data-role="assistant">
          <div class="meta">working…</div>
        </div>
      </Show>
    </div>
  );
};

const MessageView: Component<{ msg: Message }> = (props) => {
  switch (props.msg.kind) {
    case "user":
      return (
        <div class="message" data-role="user">
          <div class="meta">you</div>
          <div class="body">{props.msg.text}</div>
        </div>
      );
    case "assistant":
      return (
        <div class="message" data-role="assistant">
          <div class="meta">assistant</div>
          <div class="body markdown" innerHTML={renderMarkdown(props.msg.text)} />
        </div>
      );
    case "reasoning":
      return (
        <div class="message" data-role="assistant">
          <div class="meta">reasoning</div>
          <div class="body" style={{ color: "var(--fg-soft)" }}>
            {props.msg.text}
          </div>
        </div>
      );
    case "tool":
      return (
        <div class="tool-callout">
          <span class="tag">tool · {props.msg.tool}</span>
          <div style={{ color: "var(--fg-soft)" }}>{props.msg.input}</div>
          <Show when={props.msg.result !== undefined}>
            <div
              style={{
                "margin-top": "8px",
                "border-top": "1px solid var(--border)",
                "padding-top": "8px",
              }}
            >
              {props.msg.result}
            </div>
          </Show>
        </div>
      );
    case "error":
      return (
        <div class="message" data-role="error">
          <div class="meta" style={{ color: "var(--danger)" }}>
            error
          </div>
          <div class="body" style={{ color: "var(--danger)" }}>
            {props.msg.text}
          </div>
        </div>
      );
    case "warning":
      return (
        <div class="message" data-role="warning">
          <div class="meta" style={{ color: "var(--accent)" }}>
            warning
          </div>
          <div class="body" style={{ color: "var(--fg-soft)" }}>
            {props.msg.text}
          </div>
        </div>
      );
  }
};

export default Chat;

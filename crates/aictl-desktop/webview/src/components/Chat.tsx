import type { Component } from "solid-js";
import { For, Show, createEffect, createMemo, createResource } from "solid-js";

import type { Message } from "../App";
import { ipc } from "../lib/ipc";
import { renderMarkdown } from "../lib/markdown";

// Tool results from `generate_image` open with this exact phrase (see
// `crates/aictl-core/src/tools/image.rs::save_image`). Anchor on it so a
// stray "image saved to" inside an LLM-authored summary doesn't trigger
// a filesystem read. `read_image` deliberately does *not* trigger an
// inline preview — its only job is to feed the model.
const SAVED_IMAGE_RE = /^Image saved to (\S+\.(?:png|jpe?g|gif|webp|bmp|svg))\b/i;

function extractSavedImagePath(result: string | undefined): string | null {
  if (!result) return null;
  const match = result.match(SAVED_IMAGE_RE);
  return match ? match[1] : null;
}

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
    case "tool": {
      // `props.msg` is a Solid getter; a read after a sibling statement
      // can in principle resolve to a different variant, so narrow on
      // every access rather than caching `props.msg.result` once.
      const result = () =>
        props.msg.kind === "tool" ? props.msg.result : undefined;
      return (
        <div class="tool-callout">
          <span class="tag">tool · {props.msg.tool}</span>
          <div style={{ color: "var(--fg-soft)" }}>{props.msg.input}</div>
          <Show when={result() !== undefined}>
            <div
              style={{
                "margin-top": "8px",
                "border-top": "1px solid var(--border)",
                "padding-top": "8px",
              }}
            >
              {result()}
            </div>
          </Show>
          <Show when={extractSavedImagePath(result())}>
            {(p) => <ToolImagePreview path={p()} />}
          </Show>
        </div>
      );
    }
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

const ToolImagePreview: Component<{ path: string }> = (props) => {
  const [data] = createResource(
    () => props.path,
    (p) => ipc.readWorkspaceImage(p),
  );

  // Solid's `createResource` makes `data()` throw when the resource is
  // in an errored state; reading it inside `<Show when={data()}>` then
  // tears down the surrounding effect and the UI freezes on the last
  // rendered branch (typically "loading preview…"). Gate explicitly on
  // `state === "ready"` so the error and ready branches are mutually
  // exclusive and never raise during render.
  const ready = () => (data.state === "ready" ? data() : undefined);

  return (
    <div style={{ "margin-top": "8px" }}>
      <Show when={data.loading}>
        <div style={{ color: "var(--fg-faint)", "font-size": "11px" }}>
          loading preview…
        </div>
      </Show>
      <Show when={!data.loading && data.error}>
        <div style={{ color: "var(--fg-faint)", "font-size": "11px" }}>
          preview unavailable: {String(data.error)}
        </div>
      </Show>
      <Show when={ready()}>
        {(d) => (
          <img
            src={`data:${d().media_type};base64,${d().base64}`}
            alt={props.path}
            style={{
              "max-width": "100%",
              "max-height": "480px",
              display: "block",
              "border-radius": "4px",
              border: "1px solid var(--border)",
            }}
          />
        )}
      </Show>
    </div>
  );
};

export default Chat;

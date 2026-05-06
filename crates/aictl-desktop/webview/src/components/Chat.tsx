import type { Component } from "solid-js";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
} from "solid-js";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import type { Message } from "../App";
import { ipc } from "../lib/ipc";
import { renderMarkdown } from "../lib/markdown";

const COPY_ICON = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M5.5 3.5A1.5 1.5 0 0 1 7 2h2.879a1.5 1.5 0 0 1 1.06.44l2.122 2.12a1.5 1.5 0 0 1 .439 1.061V9.5A1.5 1.5 0 0 1 12 11V8.621a3 3 0 0 0-.879-2.121L9 4.379A3 3 0 0 0 6.879 3.5H5.5Z" /><path d="M4 5a1.5 1.5 0 0 0-1.5 1.5v6A1.5 1.5 0 0 0 4 14h5a1.5 1.5 0 0 0 1.5-1.5V8.621a1.5 1.5 0 0 0-.44-1.06L7.94 5.439A1.5 1.5 0 0 0 6.878 5H4Z" /></svg>`;

async function copyText(text: string): Promise<boolean> {
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

// Walk the rendered markdown tree and attach a copy button to every
// `<pre>` block. The button reads the inner `<code>` text at click
// time so highlight.js spans don't pollute the clipboard payload.
// `data-copy-attached` guards against re-decoration when the effect
// re-runs (e.g. after a streaming chunk extends the message).
function decorateCodeBlocks(root: HTMLElement) {
  root.querySelectorAll("pre").forEach((preEl) => {
    const pre = preEl as HTMLElement;
    if (pre.dataset.copyAttached === "1") return;
    pre.dataset.copyAttached = "1";
    const code = pre.querySelector("code");
    const text = code?.textContent ?? pre.textContent ?? "";
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "code-copy";
    btn.title = "Copy code";
    btn.setAttribute("aria-label", "Copy code");
    btn.innerHTML = COPY_ICON;
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const ok = await copyText(text);
      if (!ok) return;
      btn.classList.add("copied");
      window.setTimeout(() => btn.classList.remove("copied"), 1200);
    });
    pre.appendChild(btn);
  });
}

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
          <div class="meta">
            assistant · streaming
            <LoadingDots />
          </div>
          <div class="body markdown" innerHTML={streamingHtml()} />
        </div>
      </Show>
      <Show when={props.busy && !props.streaming}>
        <div class="message" data-role="assistant">
          <div class="meta">working…</div>
          <LoadingDots />
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
    case "assistant": {
      const text = () =>
        props.msg.kind === "assistant" ? props.msg.text : "";
      return <AssistantMessage text={text()} />;
    }
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

const AssistantMessage: Component<{ text: string }> = (props) => {
  let bodyRef: HTMLDivElement | undefined;
  const [copied, setCopied] = createSignal(false);
  let resetCopiedTimer: number | undefined;

  // The body's `innerHTML` is set by Solid from `renderMarkdown`. Solid
  // doesn't expose a post-set hook, so re-run decoration whenever the
  // text changes — `queueMicrotask` defers the query until after Solid
  // has flushed the new HTML into the DOM.
  createEffect(() => {
    void props.text;
    queueMicrotask(() => {
      if (bodyRef) decorateCodeBlocks(bodyRef);
    });
  });

  const onCopy = async () => {
    const ok = await copyText(props.text);
    if (!ok) return;
    setCopied(true);
    if (resetCopiedTimer !== undefined) window.clearTimeout(resetCopiedTimer);
    resetCopiedTimer = window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div class="message" data-role="assistant">
      <div class="meta">assistant</div>
      <div
        class="body markdown"
        ref={bodyRef}
        innerHTML={renderMarkdown(props.text)}
      />
      <div class="message-actions">
        <button
          type="button"
          class="message-copy"
          onClick={onCopy}
          title={copied() ? "Copied" : "Copy response"}
          aria-label={copied() ? "Copied" : "Copy response"}
        >
          <span class="icon" innerHTML={COPY_ICON} />
          <span class="label">{copied() ? "Copied" : "Copy"}</span>
        </button>
      </div>
    </div>
  );
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

const LoadingDots: Component = () => (
  <span class="loading-dots" role="status" aria-label="loading">
    <span /><span /><span />
  </span>
);

export default Chat;

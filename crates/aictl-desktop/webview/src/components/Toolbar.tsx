import type { Component } from "solid-js";
import { Show } from "solid-js";

import type { ActiveSession } from "../lib/ipc";

interface Props {
  activeSession: ActiveSession;
  messageCount: number;
  turnInFlight: boolean;
  onClear: () => void | Promise<void>;
  onRetry: () => void | Promise<void>;
  onUndo: () => void | Promise<void>;
  onCompact: () => void | Promise<void>;
}

const shortId = (id: string): string => id.slice(0, 8);

const Toolbar: Component<Props> = (props) => {
  const labelText = () => {
    if (props.activeSession.incognito) return "incognito";
    if (props.activeSession.name) return props.activeSession.name;
    if (props.activeSession.id) return shortId(props.activeSession.id);
    return "new session";
  };

  // Disable transcript-mutating buttons while a turn is in flight to
  // avoid the obvious race (clear/undo/retry while the engine is still
  // appending to the same vec).
  const disabled = () => props.turnInFlight || props.messageCount === 0;

  return (
    <div class="chat-toolbar">
      <div class="chat-toolbar-meta">
        <span class="chat-toolbar-label">session</span>
        <span class="chat-toolbar-id" data-incognito={String(props.activeSession.incognito)}>
          {labelText()}
        </span>
        <Show when={props.messageCount > 0}>
          <span class="chat-toolbar-count">{props.messageCount} msg</span>
        </Show>
      </div>
      <div class="chat-toolbar-actions">
        <button
          type="button"
          class="ghost"
          disabled={disabled()}
          title="Clear the chat (drops everything except the system prompt)"
          onClick={() => void props.onClear()}
        >
          Clear
        </button>
        <button
          type="button"
          class="ghost"
          disabled={disabled()}
          title="Re-send the last user prompt"
          onClick={() => void props.onRetry()}
        >
          Retry
        </button>
        <button
          type="button"
          class="ghost"
          disabled={disabled()}
          title="Drop the last user/assistant exchange"
          onClick={() => void props.onUndo()}
        >
          Undo
        </button>
        <button
          type="button"
          class="ghost"
          disabled={disabled()}
          title="Replace the transcript with a model-summarized version"
          onClick={() => void props.onCompact()}
        >
          Compact
        </button>
      </div>
    </div>
  );
};

export default Toolbar;

import { Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import type { Component } from "solid-js";

import { ipc, type AgentEvent, type WorkspaceState } from "./lib/ipc";
import Chat from "./components/Chat";
import Composer from "./components/Composer";
import ToolApproval from "./components/ToolApproval";
import EmptyWorkspace from "./components/EmptyWorkspace";
import Titlebar from "./components/Titlebar";
import Sidebar from "./components/Sidebar";

export type Message =
  | { kind: "user"; text: string }
  | { kind: "assistant"; text: string }
  | { kind: "reasoning"; text: string }
  | { kind: "tool"; tool: string; input: string; result?: string }
  | { kind: "error"; text: string }
  | { kind: "warning"; text: string };

export interface PendingApproval {
  id: number;
  tool: string;
  input: string;
}

const App: Component = () => {
  const [workspace, setWorkspace] = createSignal<WorkspaceState>({
    path: null,
    stale: false,
    error: null,
  });
  const [messages, setMessages] = createSignal<Message[]>([]);
  const [streaming, setStreaming] = createSignal(false);
  const [streamBuffer, setStreamBuffer] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [pending, setPending] = createSignal<PendingApproval | null>(null);
  const [sidebarVisible, setSidebarVisible] = createSignal(true);

  const append = (msg: Message) => setMessages((prev) => [...prev, msg]);

  const handleEvent = (e: AgentEvent) => {
    switch (e.kind) {
      case "spinner_start":
        setBusy(true);
        break;
      case "spinner_stop":
        setBusy(false);
        break;
      case "stream_begin":
        setStreaming(true);
        setStreamBuffer("");
        break;
      case "stream_chunk":
        setStreamBuffer((b) => b + e.text);
        break;
      case "stream_suspend":
        // Suspend means "next tokens are tool XML — don't show them" —
        // the engine has already filtered them out before this event
        // fires, so the surface just freezes the current buffer.
        break;
      case "stream_end": {
        const final = streamBuffer();
        if (final.trim().length > 0) {
          append({ kind: "assistant", text: final });
        }
        setStreaming(false);
        setStreamBuffer("");
        break;
      }
      case "reasoning":
        append({ kind: "reasoning", text: e.text });
        break;
      case "tool_auto":
        append({ kind: "tool", tool: e.tool, input: e.input });
        break;
      case "tool_result": {
        // Attach the result to the most recent tool message if it
        // doesn't already have one. Otherwise emit as a fresh entry —
        // shouldn't happen but keeps the UI honest.
        setMessages((prev) => {
          const next = [...prev];
          for (let i = next.length - 1; i >= 0; i--) {
            const m = next[i];
            if (m.kind === "tool" && m.result === undefined) {
              next[i] = { ...m, result: e.text };
              return next;
            }
          }
          return [...next, { kind: "tool", tool: "?", input: "", result: e.text }];
        });
        break;
      }
      case "tool_approval_request":
        setPending({ id: e.id, tool: e.tool, input: e.input });
        break;
      case "answer":
        // Streaming path already emitted the answer via stream_end. The
        // non-streaming path (e.g. provider that doesn't support SSE)
        // emits `answer` without ever streaming.
        if (!streaming() && streamBuffer() === "") {
          append({ kind: "assistant", text: e.text });
        }
        break;
      case "error":
        append({ kind: "error", text: e.text });
        setBusy(false);
        setStreaming(false);
        break;
      case "warning":
        append({ kind: "warning", text: e.text });
        break;
      default:
        // token_usage / summary / progress are emitted by the engine
        // but not yet rendered — Phase 6 wires up the stats sidebar.
        break;
    }
  };

  onMount(async () => {
    try {
      setWorkspace(await ipc.getWorkspace());
    } catch (err) {
      append({ kind: "error", text: `failed to read workspace: ${err}` });
    }

    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "\\") {
        e.preventDefault();
        setSidebarVisible((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));

    const offEvent = await ipc.onAgentEvent(handleEvent);
    const offWs = await ipc.onWorkspaceChanged(async () => {
      setWorkspace(await ipc.getWorkspace());
      append({
        kind: "warning",
        text: "workspace changed — subsequent tool calls will run in the new directory.",
      });
    });
    onCleanup(() => {
      offEvent();
      offWs();
    });
  });

  const send = async (text: string) => {
    if (!workspace().path) return;
    if (!text.trim()) return;
    append({ kind: "user", text });
    try {
      await ipc.sendMessage(text);
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const stop = async () => {
    // Optimistically clear UI state so the composer unlocks immediately.
    // The backend cancels the turn asynchronously; without this, in-flight
    // spinner_start / stream_begin events leave busy()/streaming() pinned
    // because their matching stop/end events never fire after the abort.
    const partial = streamBuffer();
    if (partial.trim().length > 0) {
      append({ kind: "assistant", text: partial });
    }
    setStreaming(false);
    setStreamBuffer("");
    setBusy(false);
    setPending(null);
    try {
      await ipc.stopTurn();
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const respond = async (decision: "allow" | "deny" | "auto_accept") => {
    const cur = pending();
    if (!cur) return;
    setPending(null);
    try {
      await ipc.toolApprovalResponse(cur.id, decision);
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const pickWorkspace = async () => {
    try {
      const picked = await ipc.pickWorkspace();
      if (picked) {
        const next = await ipc.setWorkspace(picked);
        setWorkspace(next);
      }
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const composerDisabled = createMemo(
    () => !workspace().path || busy() || streaming(),
  );
  const turnInFlight = createMemo(() => busy() || streaming());

  return (
    <div class="app" data-sidebar-hidden={String(!sidebarVisible())}>
      <Titlebar
        workspace={workspace()}
        onPickWorkspace={pickWorkspace}
        turnInFlight={turnInFlight()}
        onStop={stop}
        sidebarVisible={sidebarVisible()}
        onToggleSidebar={() => setSidebarVisible((v) => !v)}
      />
      <Sidebar />
      <main class="main">
        <Show
          when={workspace().path}
          fallback={
            <EmptyWorkspace
              workspace={workspace()}
              onPick={pickWorkspace}
            />
          }
        >
          <div class="chat">
            <Chat
              messages={messages()}
              streamingText={streamBuffer()}
              streaming={streaming()}
              busy={busy()}
            />
            <Composer
              disabled={composerDisabled()}
              onSend={send}
            />
          </div>
        </Show>
      </main>
      <Show when={pending()}>
        {(p) => (
          <ToolApproval
            request={p()}
            onAllow={() => respond("allow")}
            onDeny={() => respond("deny")}
            onAlways={() => respond("auto_accept")}
          />
        )}
      </Show>
    </div>
  );
};

export default App;

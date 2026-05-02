import { Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import type { Component } from "solid-js";

import {
  ipc,
  type ActiveSession,
  type AgentEvent,
  type LoadedMessage,
  type TranscriptMessage,
  type WorkspaceState,
} from "./lib/ipc";
import Chat from "./components/Chat";
import Composer from "./components/Composer";
import ToolApproval from "./components/ToolApproval";
import EmptyWorkspace from "./components/EmptyWorkspace";
import Titlebar from "./components/Titlebar";
import Sidebar from "./components/Sidebar";
import Toolbar from "./components/Toolbar";
import Settings from "./components/Settings";

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

/// Bridge between the Rust-side session projection (system/user/assistant/
/// tool_result) and the webview-side `Message` discriminated union. The
/// system prompt is kept in the engine-side transcript but hidden in the
/// chat surface — it would just be noise in a UI scrollback.
const projectFromBackend = (rows: LoadedMessage[] | TranscriptMessage[]): Message[] => {
  const out: Message[] = [];
  for (const m of rows) {
    if (m.kind === "system") continue;
    if (m.kind === "user") out.push({ kind: "user", text: m.text });
    else if (m.kind === "assistant") out.push({ kind: "assistant", text: m.text });
    else if (m.kind === "tool_result") {
      const trimmed = m.text.replace(/^<tool_result>\n?/, "").replace(/\n?<\/tool_result>\s*$/, "");
      out.push({ kind: "tool", tool: "tool", input: "", result: trimmed });
    }
  }
  return out;
};

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
  const [autoAccept, setAutoAccept] = createSignal(false);
  const [activeSession, setActiveSession] = createSignal<ActiveSession>({
    id: null,
    name: null,
    incognito: false,
  });
  const [sessionRefreshKey, setSessionRefreshKey] = createSignal(0);
  const [composerPrefill, setComposerPrefill] = createSignal<string | null>(null);
  const [showSettings, setShowSettings] = createSignal(false);

  const bumpSessions = () => setSessionRefreshKey((k) => k + 1);
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
        break;
      case "stream_end": {
        const final = streamBuffer();
        if (final.trim().length > 0) {
          append({ kind: "assistant", text: final });
        }
        setStreaming(false);
        setStreamBuffer("");
        // The backend just appended an assistant message and persisted
        // the transcript; refresh the sidebar so size/mtime update and
        // the active session shows up if this was the first turn.
        bumpSessions();
        void ipc.getActiveSession().then(setActiveSession);
        break;
      }
      case "reasoning":
        append({ kind: "reasoning", text: e.text });
        break;
      case "tool_auto":
        append({ kind: "tool", tool: e.tool, input: e.input });
        break;
      case "tool_result": {
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
        break;
    }
  };

  onMount(async () => {
    try {
      setWorkspace(await ipc.getWorkspace());
      setActiveSession(await ipc.getActiveSession());
    } catch (err) {
      append({ kind: "error", text: `failed to read app state: ${err}` });
    }

    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "\\") {
        e.preventDefault();
        setSidebarVisible((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));

    const onClick = (e: MouseEvent) => {
      const target = e.target;
      if (!(target instanceof Element)) return;
      const anchor = target.closest("a");
      if (!anchor) return;
      const href = anchor.getAttribute("href");
      if (!href) return;
      const isExternal =
        href.startsWith("http://") ||
        href.startsWith("https://") ||
        href.startsWith("mailto:");
      if (!isExternal) return;
      e.preventDefault();
      void ipc.openUrl(href).catch((err) => {
        append({ kind: "error", text: `failed to open link: ${err}` });
      });
    };
    document.addEventListener("click", onClick);
    onCleanup(() => document.removeEventListener("click", onClick));

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
      await ipc.sendMessage(text, autoAccept());
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const stop = async () => {
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

  const switchToSession = async (id: string) => {
    try {
      const result = await ipc.loadSession(id);
      setMessages(projectFromBackend(result.messages));
      setActiveSession({
        id: result.id,
        name: result.name,
        incognito: false,
      });
      bumpSessions();
    } catch (err) {
      append({ kind: "error", text: `failed to load session: ${err}` });
    }
  };

  const startNewSession = async () => {
    try {
      await ipc.newSession();
      setMessages([]);
      setStreamBuffer("");
      setStreaming(false);
      setActiveSession({ id: null, name: null, incognito: false });
      bumpSessions();
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const startIncognito = async () => {
    try {
      await ipc.newIncognitoSession();
      setMessages([]);
      setStreamBuffer("");
      setStreaming(false);
      setActiveSession({ id: null, name: null, incognito: true });
      bumpSessions();
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const deleteSession = async (id: string) => {
    try {
      await ipc.deleteSession(id);
      const cur = activeSession();
      if (cur.id === id) {
        setMessages([]);
        setActiveSession({ id: null, name: null, incognito: false });
      }
      bumpSessions();
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const clearAllSessions = async () => {
    try {
      await ipc.clearSessions();
      setMessages([]);
      setActiveSession({ id: null, name: null, incognito: false });
      bumpSessions();
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const renameSession = async (id: string, name: string) => {
    try {
      await ipc.renameSession(id, name);
      bumpSessions();
      if (activeSession().id === id) {
        setActiveSession(await ipc.getActiveSession());
      }
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const clearChat = async () => {
    try {
      const update = await ipc.clearChat();
      setMessages(projectFromBackend(update.messages));
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const retryLast = async () => {
    try {
      const update = await ipc.retryLast();
      setMessages(projectFromBackend(update.messages));
      if (update.prompt !== null) {
        setComposerPrefill(update.prompt);
      }
      bumpSessions();
    } catch (err) {
      append({ kind: "error", text: `${err}` });
    }
  };

  const undoLast = async () => {
    try {
      const update = await ipc.undoLast(1);
      setMessages(projectFromBackend(update.messages));
      bumpSessions();
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
      <Sidebar
        activeSession={activeSession()}
        refreshKey={sessionRefreshKey()}
        onSelectSession={switchToSession}
        onNewSession={startNewSession}
        onNewIncognito={startIncognito}
        onDeleteSession={deleteSession}
        onClearAll={clearAllSessions}
        onRenameSession={renameSession}
        onOpenSettings={() => setShowSettings(true)}
      />
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
            <Toolbar
              activeSession={activeSession()}
              messageCount={messages().length}
              turnInFlight={turnInFlight()}
              onClear={clearChat}
              onRetry={retryLast}
              onUndo={undoLast}
            />
            <Chat
              messages={messages()}
              streamingText={streamBuffer()}
              streaming={streaming()}
              busy={busy()}
            />
            <Composer
              disabled={composerDisabled()}
              onSend={send}
              autoAccept={autoAccept()}
              onAutoAcceptChange={setAutoAccept}
              prefill={composerPrefill()}
              onPrefillConsumed={() => setComposerPrefill(null)}
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
      <Show when={showSettings()}>
        <Settings
          workspace={workspace()}
          onPickWorkspace={pickWorkspace}
          onClose={() => setShowSettings(false)}
        />
      </Show>
    </div>
  );
};

export default App;

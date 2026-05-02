import { Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import type { Component } from "solid-js";

import {
  ipc,
  type ActiveModel,
  type ActiveSession,
  type AgentEvent,
  type LoadedMessage,
  type ModelEntry,
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
  const [toolsEnabled, setToolsEnabled] = createSignal(true);
  const [models, setModels] = createSignal<ModelEntry[]>([]);
  const [activeModel, setActiveModel] = createSignal<ActiveModel>({
    provider: null,
    model: null,
  });
  // Context-window usage — fed by the engine's `token_usage` event so
  // the titlebar meter updates in real time. Null until the first turn
  // emits a reading.
  const [contextPct, setContextPct] = createSignal<number | null>(null);
  const [contextTokens, setContextTokens] = createSignal<{
    input: number;
    limit: number;
  } | null>(null);

  const bumpSessions = () => setSessionRefreshKey((k) => k + 1);
  const append = (msg: Message) => setMessages((prev) => [...prev, msg]);

  // Tools master switch (`AICTL_TOOLS_ENABLED`) — read on mount and
  // refreshed every time the Settings overlay closes so the composer's
  // tool-approval picker can hide itself when the engine is in
  // chat-only mode.
  const refreshToolsEnabled = async () => {
    try {
      const raw = await ipc.configValue("AICTL_TOOLS_ENABLED");
      setToolsEnabled(raw !== "false" && raw !== "0");
    } catch {
      setToolsEnabled(true);
    }
  };

  // Tool-approval default (`AICTL_TOOL_APPROVAL`) — picked up on mount
  // and re-read every time Settings closes so a freshly-saved choice
  // takes effect without a desktop restart. The composer's local
  // toggle still overrides for the active conversation.
  const refreshApprovalDefault = async () => {
    try {
      const raw = await ipc.configValue("AICTL_TOOL_APPROVAL");
      setAutoAccept(raw === "auto");
    } catch {
      setAutoAccept(false);
    }
  };

  // Notification preference (`AICTL_DESKTOP_NOTIFICATIONS`). Cached so
  // the answer-arrived branch doesn't have to round-trip on every
  // turn.
  const [notificationsOn, setNotificationsOn] = createSignal(true);
  const refreshNotifications = async () => {
    try {
      const raw = await ipc.configValue("AICTL_DESKTOP_NOTIFICATIONS");
      setNotificationsOn(raw !== "false" && raw !== "0");
    } catch {
      setNotificationsOn(true);
    }
  };

  /// Fire a native notification when the desktop window is not focused
  /// and the user opted in. The browser API works inside the Tauri
  /// webview without an extra plugin; if the user denied permission we
  /// silently skip the call.
  const notifyIfBackgrounded = (body: string) => {
    if (!notificationsOn()) return;
    if (typeof document === "undefined") return;
    if (document.hasFocus()) return;
    if (typeof Notification === "undefined") return;
    if (Notification.permission !== "granted") return;
    try {
      const trimmed = body.trim();
      const preview = trimmed.length > 140 ? `${trimmed.slice(0, 140)}…` : trimmed;
      new Notification("aictl: response ready", { body: preview || "Response ready" });
    } catch {
      // Notification API can throw in iframes / closed contexts. No
      // fallback worth implementing here.
    }
  };

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
        bumpSessions();
        void ipc.getActiveSession().then(setActiveSession);
        notifyIfBackgrounded(final);
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
          notifyIfBackgrounded(e.text);
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
      case "token_usage":
        // Pin the latest reading on the titlebar meter. The engine
        // already computed `context_pct` (max of token-usage % and
        // message-buffer %), so we just relay it; the limit comes
        // from a follow-up context_status fetch so the titlebar can
        // also show the absolute "x / y tokens" tooltip.
        setContextPct(Math.min(100, Math.max(0, e.context_pct)));
        void ipc.contextStatus().then((c) => {
          setContextTokens({
            input: c.last_input_tokens,
            limit: c.context_limit,
          });
        });
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

    try {
      const [list, current] = await Promise.all([
        ipc.listModels(),
        ipc.getActiveModel(),
      ]);
      setModels(list);
      setActiveModel(current);
    } catch (err) {
      append({ kind: "error", text: `failed to read models: ${err}` });
    }

    void refreshToolsEnabled();
    void refreshApprovalDefault();
    void refreshNotifications();
    if (typeof Notification !== "undefined" && Notification.permission === "default") {
      void Notification.requestPermission().catch(() => {});
    }

    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "\\") {
        e.preventDefault();
        setSidebarVisible((v) => !v);
        return;
      }
      // ⌘K / Ctrl-K toggles the Settings overlay. Settings has its
      // own Esc handler for the close path, so we only flip the open
      // state here and leave the close to the panel itself when the
      // overlay is visible.
      if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        setShowSettings((v) => !v);
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
    // On approval the engine runs the tool and emits `tool_result`
    // without a preceding `tool_auto`; seed the message here so the
    // result patches into a callout that knows its tool name + input
    // (the chat surface needs both to render an image preview for
    // `read_image`, and the picker hides as soon as we clear it).
    if (decision === "allow" || decision === "auto_accept") {
      append({ kind: "tool", tool: cur.tool, input: cur.input });
    }
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

  const compactChat = async () => {
    try {
      const update = await ipc.compactChat();
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

  /// Single writer for the active model so the composer dropdown and the
  /// Settings → Provider tab stay in sync — whichever surface triggers
  /// the change, both reflect it on the next render.
  const changeModel = async (provider: string, model: string) => {
    const next = await ipc.setActiveModel(provider, model);
    setActiveModel(next);
  };

  return (
    <div class="app" data-sidebar-hidden={String(!sidebarVisible())}>
      <Titlebar
        workspace={workspace()}
        onPickWorkspace={pickWorkspace}
        turnInFlight={turnInFlight()}
        onStop={stop}
        sidebarVisible={sidebarVisible()}
        onToggleSidebar={() => setSidebarVisible((v) => !v)}
        contextPct={contextPct()}
        contextTokens={contextTokens()}
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
              onCompact={compactChat}
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
              toolsEnabled={toolsEnabled()}
              prefill={composerPrefill()}
              onPrefillConsumed={() => setComposerPrefill(null)}
              models={models()}
              activeModel={activeModel()}
              onChangeModel={changeModel}
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
          onClose={() => {
            setShowSettings(false);
            void refreshToolsEnabled();
            void refreshApprovalDefault();
            void refreshNotifications();
          }}
          models={models()}
          activeModel={activeModel()}
          onChangeModel={changeModel}
        />
      </Show>
    </div>
  );
};

export default App;

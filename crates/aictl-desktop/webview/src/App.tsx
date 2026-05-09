import {
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import type { Component } from "solid-js";
import {
  LogicalSize,
  currentMonitor,
  getCurrentWindow,
} from "@tauri-apps/api/window";

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
import FilePane from "./components/FilePane";
import EditorPane from "./components/EditorPane";
import Settings, { type Tab as SettingsTab } from "./components/Settings";
import ContextDetails from "./components/ContextDetails";
import UpdateModal from "./components/UpdateModal";
import { checkUpdate, type UpdateInfo } from "./lib/updater";
import ProviderSetup, {
  type ProviderSetupTarget,
} from "./components/ProviderSetup";

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

/// Verbs that, when used as a one-word command (or as a prefix to a
/// task), trigger the loaded skill. Mirrors the CLI's `InvokeSkill`
/// rewrite: a bare verb expands to a default trigger; `verb <task>`
/// uses `<task>` as the user message so the skill body (already
/// merged into the system prompt by the engine) drives the turn with
/// that parameter. Matched case-insensitively against the first word.
const SKILL_INVOKE_VERBS = ["run", "execute", "start", "go"];

const expandSkillCommand = (
  input: string,
  skillName: string | null,
): string => {
  if (!skillName) return input;
  const trimmed = input.trim();
  if (trimmed === "") return input;
  const lower = trimmed.toLowerCase();
  for (const verb of SKILL_INVOKE_VERBS) {
    if (lower === verb) {
      return `Run the "${skillName}" skill.`;
    }
    const prefix = `${verb} `;
    if (lower.startsWith(prefix)) {
      const task = trimmed.slice(prefix.length).trim();
      if (task === "") return `Run the "${skillName}" skill.`;
      return task;
    }
  }
  return input;
};

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
  // Persisted across launches via `AICTL_DESKTOP_SIDEBAR_VISIBLE` so a
  // user who hides the sidebar finds it hidden next time. The signal
  // defaults to true; the stored value only flips it when explicitly
  // "false". Toggles fire-and-forget the write — failures just mean
  // the next launch falls back to the default.
  const toggleSidebar = () => {
    setSidebarVisible((v) => {
      const next = !v;
      void ipc.configWrite(
        "AICTL_DESKTOP_SIDEBAR_VISIBLE",
        next ? "true" : "false",
      );
      if (next && workspace().path) {
        void growWindowTo(
          requiredLayoutWidth(true, openFilePath() !== null, filesVisible()),
        );
      }
      return next;
    });
  };
  // File-pane visibility — closed by default per the spec, persisted
  // across launches the same way the sidebar toggle is.
  const [filesVisible, setFilesVisible] = createSignal(false);
  const toggleFiles = () => {
    setFilesVisible((v) => {
      const next = !v;
      void ipc.configWrite(
        "AICTL_DESKTOP_FILES_VISIBLE",
        next ? "true" : "false",
      );
      if (next && workspace().path) {
        void growWindowTo(
          requiredLayoutWidth(
            sidebarVisible(),
            openFilePath() !== null,
            true,
          ),
        );
      }
      return next;
    });
  };
  // Path of the file currently shown in the editor pane. The pane
  // itself sits between the chat and the tree; null means no editor
  // is rendered. Hiding the tree does not close the editor — the user
  // may want to keep editing the file with more screen real estate.
  const [openFilePath, setOpenFilePath] = createSignal<string | null>(null);
  // Persisted across launches via `AICTL_DESKTOP_OPEN_FILE` so reopening
  // the app restores the same editor pane. The wrapper is used by every
  // call site that reflects a user-initiated change (open from tree,
  // close button, workspace switch); raw `setOpenFilePath` is reserved
  // for the hydration path so the read-then-set round-trip doesn't
  // immediately rewrite its own source value.
  const setOpenFile = (path: string | null) => {
    const wasOpen = openFilePath() !== null;
    setOpenFilePath(path);
    if (path === null) {
      void ipc.configClear("AICTL_DESKTOP_OPEN_FILE").catch(() => {});
    } else {
      void ipc.configWrite("AICTL_DESKTOP_OPEN_FILE", path).catch(() => {});
      if (!wasOpen && workspace().path) {
        void growWindowTo(
          requiredLayoutWidth(sidebarVisible(), true, filesVisible()),
        );
      }
    }
  };
  // Pane widths in pixels. Drag handles between adjacent visible panes
  // mutate these signals; a debounced effect persists them through
  // AICTL_DESKTOP_*_WIDTH keys so launches restore the user's layout.
  // The chat (main) is always the 1fr column — it absorbs whatever the
  // user takes from or gives back to its neighbours.
  const SIDEBAR_DEFAULT = 240;
  const EDITOR_DEFAULT = 480;
  const FILES_DEFAULT = 280;
  const SIDEBAR_MIN = 160;
  const SIDEBAR_MAX = 600;
  const EDITOR_MIN = 280;
  const EDITOR_MAX = 1000;
  const FILES_MIN = 200;
  const FILES_MAX = 700;
  // Floor for the chat column when computing whether the layout fits
  // the current window. Sized to keep the composer footer (model
  // picker + agent / skill / mcp / plugins / tools / image / web /
  // auto-accept icons + Send button with its ⌘↵ chip) on a single row
  // alongside the standard horizontal padding — auto-grow reserves at
  // least this many pixels, and auto-close drops side panes once the
  // chat column would dip below it. Bumped each time we add a new
  // composer toggle so the model picker keeps a readable minimum
  // instead of collapsing to a sliver.
  const CHAT_MIN_WIDTH = 720;
  const [sidebarWidth, setSidebarWidth] = createSignal(SIDEBAR_DEFAULT);
  const [editorWidth, setEditorWidth] = createSignal(EDITOR_DEFAULT);
  const [filesWidth, setFilesWidth] = createSignal(FILES_DEFAULT);
  // Total CSS-pixel width the layout needs given which panes are
  // visible. Uses each pane's *current* width (not just its min) so
  // the auto-grow path lands on the user's persisted layout instead of
  // collapsing every pane to its minimum.
  const requiredLayoutWidth = (
    sidebarOn: boolean,
    editorOn: boolean,
    filesOn: boolean,
  ) => {
    let total = CHAT_MIN_WIDTH;
    if (sidebarOn) total += sidebarWidth();
    if (editorOn) total += editorWidth();
    if (filesOn) total += filesWidth();
    return total;
  };
  // Grow the window to at least `target` CSS pixels wide, capped at
  // the current monitor's work area. Height is left alone. Called on
  // every pane-open so a small window auto-expands instead of leaving
  // the new pane crammed into a too-narrow chat column. Failures are
  // swallowed — the worst-case fallback is a tight layout the user
  // can resize manually.
  const growWindowTo = async (target: number) => {
    if (typeof window === "undefined") return;
    if (window.innerWidth >= target) return;
    try {
      const win = getCurrentWindow();
      const [innerPhys, scale, monitor] = await Promise.all([
        win.innerSize(),
        win.scaleFactor(),
        currentMonitor(),
      ]);
      const curLogicalWidth = innerPhys.width / scale;
      const curLogicalHeight = innerPhys.height / scale;
      let next = Math.ceil(target);
      if (monitor) {
        const maxLogicalWidth =
          monitor.workArea.size.width / monitor.scaleFactor;
        next = Math.min(next, Math.floor(maxLogicalWidth));
      }
      if (next <= curLogicalWidth) return;
      await win.setSize(new LogicalSize(next, Math.ceil(curLogicalHeight)));
    } catch (err) {
      console.warn("auto-resize failed", err);
    }
  };
  // Close the most-ancillary visible pane until the rest of the
  // layout fits the current window width. Order: files → editor →
  // sidebar (chat stays). Each close is persisted through the same
  // config keys the toggles use so a relaunch reflects the
  // auto-collapsed layout.
  const closeExcessPanes = () => {
    if (typeof window === "undefined") return;
    if (!workspace().path) return;
    const avail = window.innerWidth;
    let s = sidebarVisible();
    let e = openFilePath() !== null;
    let f = filesVisible();
    if (requiredLayoutWidth(s, e, f) <= avail) return;
    if (f) {
      setFilesVisible(false);
      void ipc
        .configWrite("AICTL_DESKTOP_FILES_VISIBLE", "false")
        .catch(() => {});
      f = false;
      if (requiredLayoutWidth(s, e, f) <= avail) return;
    }
    if (e) {
      setOpenFile(null);
      e = false;
      if (requiredLayoutWidth(s, e, f) <= avail) return;
    }
    if (s) {
      setSidebarVisible(false);
      void ipc
        .configWrite("AICTL_DESKTOP_SIDEBAR_VISIBLE", "false")
        .catch(() => {});
    }
  };
  // Bumped every time the backend's recursive `notify` watcher reports a
  // change inside the workspace. The file pane and editor read this as
  // a refresh signal — they re-fetch their current view rather than
  // diffing the event payload (which is just a coalesced "something
  // moved" pulse anyway).
  const [fsTick, setFsTick] = createSignal(0);
  const [autoAccept, setAutoAccept] = createSignal(false);
  const [activeSession, setActiveSession] = createSignal<ActiveSession>({
    id: null,
    name: null,
    incognito: false,
  });
  const [sessionRefreshKey, setSessionRefreshKey] = createSignal(0);
  const [composerPrefill, setComposerPrefill] = createSignal<string | null>(null);
  const [showSettings, setShowSettings] = createSignal(false);
  const [settingsInitialTab, setSettingsInitialTab] = createSignal<
    SettingsTab | undefined
  >(undefined);
  const [showContextDetails, setShowContextDetails] = createSignal(false);
  // In-app updater dialog — opened from the titlebar update banner or
  // from the About tab's "Install update" button. Carries the
  // pre-fetched manifest entry so the modal can skip its own re-check
  // when the user clicks straight from the badge.
  const [showUpdate, setShowUpdate] = createSignal(false);
  const [updateInfo, setUpdateInfo] = createSignal<UpdateInfo | null>(null);
  const openUpdate = (info: UpdateInfo | null) => {
    setUpdateInfo(info);
    setShowUpdate(true);
  };
  // First-run nudge: when the user has no usable model provider
  // configured we surface a dialog with deep links into the relevant
  // Settings tabs. `dismissed` flips on Skip so we don't re-pop it
  // mid-session even if the check still says "nothing configured".
  const [showProviderSetup, setShowProviderSetup] = createSignal(false);
  const [providerSetupDismissed, setProviderSetupDismissed] =
    createSignal(false);
  const [toolsEnabled, setToolsEnabled] = createSignal(true);
  // Plugins master switch — mirror of `AICTL_PLUGINS_ENABLED`. The CLI
  // defaults to disabled (third-party code must be opted in), but the
  // desktop opts in on first launch so the cube icon's default-on state
  // matches the engine's runtime behaviour. `refreshPluginsEnabled`
  // writes `true` when the key is missing so the UI and engine never
  // disagree about the current state.
  const [pluginsEnabled, setPluginsEnabled] = createSignal(true);
  // MCP master switch — same default-on opt-in as plugins. The CLI
  // gate (`AICTL_MCP_ENABLED`) defaults to off, so the desktop writes
  // `true` on first launch to make the disk icon's default-on visual
  // honest. Toggling reloads the engine's MCP catalogue immediately.
  const [mcpEnabled, setMcpEnabled] = createSignal(true);
  // Composer's globe icon mirrors the per-tool disabled list in
  // `AICTL_SECURITY_DISABLED_TOOLS` for the three web-facing tools
  // (`search_web`, `fetch_url`, `extract_website`). Active when none of
  // them are disabled — flipping the icon writes through to the same
  // config key the Settings → Tools panel manages, so the two surfaces
  // round-trip without an app restart.
  const [webEnabled, setWebEnabled] = createSignal(true);
  // Sibling toggle for the two image tools (`read_image`,
  // `generate_image`). Same `AICTL_SECURITY_DISABLED_TOOLS` plumbing as
  // the web toggle so the Settings → Tools panel stays in sync.
  const [imageEnabled, setImageEnabled] = createSignal(true);
  // Memory icon — mirrors `AICTL_MEMORY_ENABLED` (default on). Round-
  // trips through the same `memory_set_enabled` Tauri command the
  // Settings → Memory panel calls so the two surfaces stay in sync.
  const [memoryEnabled, setMemoryEnabled] = createSignal(true);
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
  // Skill currently pinned to every turn via the composer's bolt-icon
  // picker. The backend persists the selection across IPC calls; we
  // hydrate this on mount so the icon's highlight survives a window
  // reload.
  const [loadedSkill, setLoadedSkill] = createSignal<string | null>(null);
  // Same idea for the agent slot — the engine keeps the agent in a
  // process-wide static (`agents::LOADED_AGENT`), which the system
  // prompt builder reads. We mirror the name here so the composer's
  // sparkles icon can light up.
  const [loadedAgent, setLoadedAgent] = createSignal<string | null>(null);
  // Latest upstream release advertised by the on-launch version check.
  // Drives the titlebar update badge; null while the probe is in flight
  // or when the build is already on master. The probe hits a raw GitHub
  // asset with a short timeout, so a flaky network just leaves the
  // signal null and the badge stays hidden.
  const [latestVersion, setLatestVersion] = createSignal<string | null>(null);
  // Aggregated security/redaction/keyring posture for the composer's
  // shield icon. Populated by `refreshSecurityStatus` on mount and
  // every Settings close. The shape mirrors `SecurityShield`'s `checks`
  // prop so the composer can pass it straight through.
  const [securityState, setSecurityState] = createSignal<
    "ok" | "warn" | "error"
  >("ok");
  const [securityChecks, setSecurityChecks] = createSignal<
    { label: string; ok: boolean; hint?: string }[]
  >([]);

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

  // Web tools toggle — derived from `AICTL_SECURITY_DISABLED_TOOLS`.
  // Active iff none of the three web tools sit in the disabled list.
  // Re-read whenever Settings closes so a per-tool flip from the
  // Tools panel reflects on the composer icon.
  const WEB_TOOLS = ["search_web", "fetch_url", "extract_website"];
  const refreshWebEnabled = async () => {
    try {
      const raw =
        (await ipc.configValue("AICTL_SECURITY_DISABLED_TOOLS")) ?? "";
      const disabled = raw
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      setWebEnabled(!WEB_TOOLS.some((t) => disabled.includes(t)));
    } catch {
      setWebEnabled(true);
    }
  };

  /// Flip every web tool to `next` in one shot. The Settings → Tools
  /// panel reads from the same config key, so a refetch on the next
  /// open reflects the change. Failures bubble back through the
  /// composer's flash so the user knows the toggle didn't stick.
  const setWebTools = async (next: boolean) => {
    const disable = !next;
    for (const name of WEB_TOOLS) {
      await ipc.toolSetDisabled(name, disable);
    }
    setWebEnabled(next);
  };

  // Image tools toggle — mirror of the web flow for `read_image` and
  // `generate_image`. Same `AICTL_SECURITY_DISABLED_TOOLS` gate.
  const IMAGE_TOOLS = ["read_image", "generate_image"];
  const refreshImageEnabled = async () => {
    try {
      const raw =
        (await ipc.configValue("AICTL_SECURITY_DISABLED_TOOLS")) ?? "";
      const disabled = raw
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      setImageEnabled(!IMAGE_TOOLS.some((t) => disabled.includes(t)));
    } catch {
      setImageEnabled(true);
    }
  };

  const setImageTools = async (next: boolean) => {
    const disable = !next;
    for (const name of IMAGE_TOOLS) {
      await ipc.toolSetDisabled(name, disable);
    }
    setImageEnabled(next);
  };

  // Memory master switch — reads the engine's MemoryStatus rather than
  // the raw config key so the icon follows the same source of truth as
  // the Settings panel.
  const refreshMemoryEnabled = async () => {
    try {
      const status = await ipc.memoryStatus();
      setMemoryEnabled(status.enabled);
    } catch {
      setMemoryEnabled(true);
    }
  };

  const setMemoryEnabledMaster = async (next: boolean) => {
    const status = await ipc.memorySetEnabled(next);
    setMemoryEnabled(status.enabled);
  };

  // Plugins toggle — reads `AICTL_PLUGINS_ENABLED` and treats a missing
  // key as the desktop's default-on opt-in (the desktop writes `true`
  // on first read so the engine matches what the cube icon shows). A
  // value of "false" or "0" pins the icon to the disabled state.
  const refreshPluginsEnabled = async () => {
    try {
      const raw = await ipc.configValue("AICTL_PLUGINS_ENABLED");
      if (raw === null || raw === undefined || raw === "") {
        // First launch — opt the desktop into plugins so the icon's
        // default-on state matches the engine. The reload below picks
        // up the change without an app restart.
        await ipc.configWrite("AICTL_PLUGINS_ENABLED", "true");
        await ipc.pluginsReload();
        setPluginsEnabled(true);
        return;
      }
      setPluginsEnabled(raw !== "false" && raw !== "0");
    } catch {
      setPluginsEnabled(true);
    }
  };

  /// Plugins master switch — flips `AICTL_PLUGINS_ENABLED` and reloads
  /// the in-memory plugin catalogue so the change takes effect on the
  /// next agent turn instead of waiting for an app restart. Failures
  /// bubble back to the composer's flash so the icon doesn't lie about
  /// what landed in config.
  const setPluginsEnabledMaster = async (next: boolean) => {
    await ipc.configWrite("AICTL_PLUGINS_ENABLED", next ? "true" : "false");
    await ipc.pluginsReload();
    setPluginsEnabled(next);
  };

  // MCP master switch — same lifecycle as plugins. First-launch read
  // writes `true` so the engine spawns configured servers in line with
  // the icon's default-on visual; subsequent reads just mirror config.
  const refreshMcpEnabled = async () => {
    try {
      const raw = await ipc.configValue("AICTL_MCP_ENABLED");
      if (raw === null || raw === undefined || raw === "") {
        await ipc.configWrite("AICTL_MCP_ENABLED", "true");
        await ipc.mcpReload();
        setMcpEnabled(true);
        return;
      }
      setMcpEnabled(raw !== "false" && raw !== "0");
    } catch {
      setMcpEnabled(true);
    }
  };

  /// MCP master switch — flips `AICTL_MCP_ENABLED` and reloads the
  /// engine's MCP catalogue so the change applies in real time. The
  /// reload tears down spawned child processes when disabled and
  /// re-spawns them when enabled.
  const setMcpEnabledMaster = async (next: boolean) => {
    await ipc.configWrite("AICTL_MCP_ENABLED", next ? "true" : "false");
    await ipc.mcpReload();
    setMcpEnabled(next);
  };

  /// Master tools toggle — flips `AICTL_TOOLS_ENABLED` and cascades to
  /// the web + image subset toggles so the composer's three icons share
  /// a single semantic. Turning master OFF disables every tool the
  /// agent can call (the engine reads `AICTL_TOOLS_ENABLED` to gate
  /// dispatch entirely) AND clears the web/image active state so their
  /// icons reflect the disabled status. Turning master ON re-enables
  /// the subset toggles in lockstep.
  const setToolsMasterEnabled = async (next: boolean) => {
    if (next) {
      await ipc.configClear("AICTL_TOOLS_ENABLED");
    } else {
      await ipc.configWrite("AICTL_TOOLS_ENABLED", "false");
    }
    setToolsEnabled(next);
    await setWebTools(next);
    await setImageTools(next);
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

  /// Check whether the user has at least one usable LLM provider:
  /// an LLM API key, a downloaded local model, an Ollama daemon with
  /// at least one model, or a fully-configured aictl-server. The
  /// Ollama probe is HTTP — racing against a short timeout keeps the
  /// dialog from blocking on a missing daemon. Failures default to
  /// "not available" so the dialog errs on the side of prompting.
  const hasAnyProvider = async (): Promise<boolean> => {
    const checks: Promise<boolean>[] = [];

    checks.push(
      ipc
        .keysStatus()
        .then((rows) =>
          rows.some(
            (r) => r.name.startsWith("LLM_") && r.location !== "unset",
          ),
        )
        .catch(() => false),
    );

    checks.push(
      ipc
        .serverStatus()
        .then((s) => s.fully_configured)
        .catch(() => false),
    );

    checks.push(
      ipc
        .localModelsStatus()
        .then(
          (s) => s.gguf.models.length > 0 || s.mlx.models.length > 0,
        )
        .catch(() => false),
    );

    const ollamaProbe = ipc
      .ollamaProbe()
      .then((p) => p.ok && (p.model_count ?? 0) > 0)
      .catch(() => false);
    const ollamaTimeout = new Promise<boolean>((resolve) =>
      setTimeout(() => resolve(false), 2000),
    );
    checks.push(Promise.race([ollamaProbe, ollamaTimeout]));

    const results = await Promise.all(checks);
    return results.some(Boolean);
  };

  /// Re-evaluate provider availability and show or hide the setup
  /// dialog accordingly. Skips opening when the user has dismissed it
  /// for this session, but always closes it when something becomes
  /// available so a successful configuration removes the nag. The
  /// dialog is also suppressed until a workspace is selected — the
  /// EmptyWorkspace screen comes first so the user picks a folder
  /// before being asked about providers.
  const refreshProviderSetup = async () => {
    if (!workspace().path) {
      setShowProviderSetup(false);
      return;
    }
    const available = await hasAnyProvider();
    if (available) {
      setShowProviderSetup(false);
      return;
    }
    if (providerSetupDismissed()) return;
    setShowProviderSetup(true);
  };

  /// Re-evaluate the composer shield by reading the relevant config
  /// keys plus the keyring presence. The shield turns red the moment
  /// `AICTL_SECURITY` is off (the engine's master gate); otherwise it
  /// goes green only when every recommended hardening knob is on AND
  /// no API key sits in plain text. Anything in between is yellow.
  const refreshSecurityStatus = async () => {
    const isOn = (raw: string | null, defaultOn: boolean): boolean => {
      if (raw === null || raw === undefined || raw === "") return defaultOn;
      return raw !== "false" && raw !== "0";
    };
    try {
      const [
        sec,
        injection,
        audit,
        cwd,
        subshell,
        redactionMode,
        detectorsRaw,
        ner,
        keys,
      ] = await Promise.all([
        ipc.configValue("AICTL_SECURITY"),
        ipc.configValue("AICTL_SECURITY_INJECTION_GUARD"),
        ipc.configValue("AICTL_SECURITY_AUDIT_LOG"),
        ipc.configValue("AICTL_SECURITY_CWD_RESTRICT"),
        ipc.configValue("AICTL_SECURITY_BLOCK_SUBSHELL"),
        ipc.configValue("AICTL_SECURITY_REDACTION"),
        ipc.configValue("AICTL_REDACTION_DETECTORS"),
        ipc.configValue("AICTL_REDACTION_NER"),
        ipc.keysStatus().catch(() => []),
      ]);

      const securityOn = isOn(sec, true);
      const injectionOn = isOn(injection, true);
      const auditOn = isOn(audit, true);
      const cwdOn = isOn(cwd, true);
      const subshellOn = isOn(subshell, true);

      const mode = (redactionMode ?? "").trim().toLowerCase();
      const redactionOn = mode === "redact" || mode === "block";
      const detectorsAllOn = (detectorsRaw ?? "").trim() === "";
      const nerOn = isOn(ner, false);

      const llmKeys = keys.filter(
        (k) => k.name.startsWith("LLM_") && k.location !== "unset",
      );
      // Plain-text and "both" both mean a copy lives on disk in clear
      // text — neither is acceptable for the green state.
      const leakingKeys = llmKeys.filter(
        (k) => k.location === "plain" || k.location === "both",
      );
      const keysOk = leakingKeys.length === 0;

      const checks: { label: string; ok: boolean; hint?: string }[] = [
        {
          label: "Security policy enabled",
          ok: securityOn,
          hint: securityOn
            ? undefined
            : "AICTL_SECURITY is off — CWD jail, shell allow-list, and tool denial are bypassed.",
        },
        {
          label: "Prompt-injection guard",
          ok: injectionOn,
        },
        {
          label: "Audit log",
          ok: auditOn,
        },
        {
          label: "Workspace-only file access",
          ok: cwdOn,
        },
        {
          label: "Block shell metacharacters",
          ok: subshellOn,
        },
        {
          label: "Outbound redaction",
          ok: redactionOn,
          hint: redactionOn
            ? undefined
            : "Set AICTL_SECURITY_REDACTION to 'redact' or 'block' to strip secrets before they leave the machine.",
        },
        {
          label: "All redaction detectors enabled",
          ok: detectorsAllOn,
          hint: detectorsAllOn
            ? undefined
            : "AICTL_REDACTION_DETECTORS narrows the active detector set.",
        },
        {
          label: "NER pass enabled",
          ok: nerOn,
          hint: nerOn
            ? undefined
            : "AICTL_REDACTION_NER is off — names, locations, and organizations are not redacted.",
        },
        {
          label:
            llmKeys.length === 0
              ? "API keys stored in keyring"
              : `API keys stored in keyring (${llmKeys.length} configured)`,
          ok: keysOk,
          hint: keysOk
            ? undefined
            : `${leakingKeys.length} key(s) sit in plain config — lock them through Settings → API Keys.`,
        },
      ];

      let state: "ok" | "warn" | "error";
      if (!securityOn) state = "error";
      else if (checks.every((c) => c.ok)) state = "ok";
      else state = "warn";

      setSecurityState(state);
      setSecurityChecks(checks);
    } catch {
      // On a probe failure leave the shield in its previous state
      // rather than flashing a misleading colour.
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

  // Drive the provider-setup dialog off the workspace path: on mount
  // (`workspace().path` is null until `getWorkspace()` resolves) the
  // effect runs once and short-circuits; once a workspace is picked the
  // effect re-fires with a non-null path and triggers the availability
  // check. Runs again whenever the user switches workspaces.
  createEffect(() => {
    void workspace().path;
    void refreshProviderSetup();
  });

  onMount(async () => {
    // Hydrate sidebar visibility before workspace loads so the layout
    // settles in the persisted state on first paint instead of flashing
    // visible-then-hidden.
    try {
      const raw = await ipc.configValue("AICTL_DESKTOP_SIDEBAR_VISIBLE");
      if (raw === "false") setSidebarVisible(false);
    } catch {
      // Default-true if the read fails.
    }

    try {
      const raw = await ipc.configValue("AICTL_DESKTOP_FILES_VISIBLE");
      if (raw === "true") setFilesVisible(true);
    } catch {
      // Default-false if the read fails.
    }

    // Hydrate persisted pane widths. Bad/out-of-range values are
    // ignored so a hand-edited config can't strand the user with a
    // 5px-wide chat.
    const hydrateWidth = (
      key: string,
      min: number,
      max: number,
      setter: (n: number) => void,
    ) => {
      void ipc
        .configValue(key)
        .then((raw) => {
          if (!raw) return;
          const n = Number.parseInt(raw, 10);
          if (Number.isFinite(n) && n >= min && n <= max) setter(n);
        })
        .catch(() => {});
    };
    hydrateWidth("AICTL_DESKTOP_SIDEBAR_WIDTH", SIDEBAR_MIN, SIDEBAR_MAX, setSidebarWidth);
    hydrateWidth("AICTL_DESKTOP_EDITOR_WIDTH", EDITOR_MIN, EDITOR_MAX, setEditorWidth);
    hydrateWidth("AICTL_DESKTOP_FILES_WIDTH", FILES_MIN, FILES_MAX, setFilesWidth);

    try {
      setWorkspace(await ipc.getWorkspace());
      setActiveSession(await ipc.getActiveSession());
    } catch (err) {
      append({ kind: "error", text: `failed to read app state: ${err}` });
    }

    // Restore the previously-open editor file once the workspace is known
    // (the path is workspace-relative). If the file is gone or unreadable,
    // forget the persisted path and force-open the files view so the user
    // lands somewhere useful instead of a missing pane.
    if (workspace().path) {
      try {
        const savedFile = await ipc.configValue("AICTL_DESKTOP_OPEN_FILE");
        if (savedFile && savedFile.trim() !== "") {
          try {
            await ipc.workspaceReadFile(savedFile);
            setOpenFilePath(savedFile);
          } catch {
            void ipc.configClear("AICTL_DESKTOP_OPEN_FILE").catch(() => {});
            if (!filesVisible()) {
              setFilesVisible(true);
              void ipc
                .configWrite("AICTL_DESKTOP_FILES_VISIBLE", "true")
                .catch(() => {});
            }
          }
        }
      } catch {
        // No persisted path / read failed — leave the editor closed.
      }
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
    void refreshWebEnabled();
    void refreshImageEnabled();
    void refreshMemoryEnabled();
    void refreshPluginsEnabled();
    void refreshMcpEnabled();
    void refreshNotifications();
    void refreshSecurityStatus();
    // Fire-and-forget probe of the updater manifest. Populates both the
    // titlebar badge and the prefetched `updateInfo` the modal opens
    // with, so a click on the badge skips its own re-check round-trip.
    // Failures (offline, manifest 404, dismissed in localStorage) just
    // leave the badge hidden.
    void checkUpdate()
      .then((info) => {
        if (!info) return;
        try {
          if (
            typeof window !== "undefined" &&
            window.localStorage.getItem("aictl.update.dismissed") ===
              info.version
          ) {
            return;
          }
        } catch {
          // localStorage unavailable — fall through to showing the badge.
        }
        setUpdateInfo(info);
        setLatestVersion(info.version);
        // Auto-open the update dialog on startup so the user sees the
        // new version without having to spot the titlebar badge.
        // "Not now" only closes for the current launch — the dialog
        // re-opens next time until the badge's X is used or the
        // update is installed.
        setShowUpdate(true);
      })
      .catch(() => {});
    void ipc
      .skillLoaded()
      .then((name) => setLoadedSkill(name))
      .catch(() => setLoadedSkill(null));
    void ipc
      .agentLoaded()
      .then((name) => setLoadedAgent(name))
      .catch(() => setLoadedAgent(null));
    if (typeof Notification !== "undefined" && Notification.permission === "default") {
      void Notification.requestPermission().catch(() => {});
    }

    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "\\") {
        e.preventDefault();
        toggleSidebar();
        return;
      }
      // ⌘. toggles the right-side file pane. ⌘\ already owns the
      // sidebar, so the file pane gets the closest free chord.
      if ((e.metaKey || e.ctrlKey) && e.key === ".") {
        if (!workspace().path) return;
        e.preventDefault();
        toggleFiles();
        return;
      }
      // ⌘, / Ctrl-, toggles the Settings overlay (matches the macOS
      // Preferences convention). Settings has its own Esc handler for
      // the close path, so we only flip the open state here and leave
      // the close to the panel itself when the overlay is visible.
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
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

    // Auto-collapse panes that no longer fit when the user shrinks
    // the window. rAF-coalesced so a corner-drag doesn't hammer the
    // signals on every pixel — only the final settled width matters.
    let resizeRaf = 0;
    const onResize = () => {
      if (resizeRaf) cancelAnimationFrame(resizeRaf);
      resizeRaf = requestAnimationFrame(() => {
        resizeRaf = 0;
        closeExcessPanes();
      });
    };
    window.addEventListener("resize", onResize);
    onCleanup(() => {
      window.removeEventListener("resize", onResize);
      if (resizeRaf) cancelAnimationFrame(resizeRaf);
    });
    // Hydrated state may already overflow the restored window (e.g. a
    // tighter monitor on this launch); collapse panes once after the
    // visibility / open-file hydration above has settled.
    closeExcessPanes();

    const offEvent = await ipc.onAgentEvent(handleEvent);
    const offFs = await ipc.onWorkspaceFsChanged(() => {
      setFsTick((t) => t + 1);
    });
    const offWs = await ipc.onWorkspaceChanged(async () => {
      setWorkspace(await ipc.getWorkspace());
      // Open path is workspace-relative; switching workspaces invalidates
      // it. Clear the persisted key too so the new workspace doesn't try
      // to restore a stale path on next launch.
      setOpenFile(null);
      append({
        kind: "warning",
        text: "workspace changed — subsequent tool calls will run in the new directory.",
      });
    });
    onCleanup(() => {
      offEvent();
      offFs();
      offWs();
    });
  });

  const send = async (text: string) => {
    if (!workspace().path) return;
    if (!text.trim()) return;
    const expanded = expandSkillCommand(text, loadedSkill());
    append({ kind: "user", text: expanded });
    try {
      await ipc.sendMessage(expanded, autoAccept());
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

  const useDefaultWorkspace = async () => {
    try {
      const next = await ipc.useDefaultWorkspace();
      setWorkspace(next);
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
      bumpSessions();
      if (update.prompt !== null) {
        await send(update.prompt);
      }
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
      // Refresh the titlebar meter and any open ContextDetails: the
      // backend resets its cached token counts on success, so a fresh
      // contextStatus() reflects the post-compact state. Failures are
      // swallowed — the rest of the compaction already succeeded.
      try {
        const c = await ipc.contextStatus();
        setContextPct(Math.min(100, Math.max(0, c.context_pct)));
        setContextTokens({
          input: c.last_input_tokens,
          limit: c.context_limit,
        });
      } catch (err) {
        console.warn("failed to refresh context status after compact", err);
      }
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

  /// Re-pull the model catalogue. Called after a local-model download
  /// finishes so the composer dropdown and Settings → Model tab pick up
  /// the new entry without an app restart. Failures are silent — the
  /// existing list stays as-is rather than going blank.
  const refreshModels = async () => {
    try {
      setModels(await ipc.listModels());
    } catch (err) {
      console.warn("failed to refresh model catalogue", err);
    }
  };

  // Hide the sidebar (and its toggle) until a workspace is picked —
  // the EmptyWorkspace screen is the only meaningful interaction at
  // that point, so a session list and settings shortcut would just be
  // noise. Once a workspace is set, the user's own toggle preference
  // takes over again.
  const sidebarHidden = createMemo(
    () => !workspace().path || !sidebarVisible(),
  );
  const filesPaneHidden = createMemo(
    () => !workspace().path || !filesVisible(),
  );
  const editorPaneHidden = createMemo(
    () => !workspace().path || openFilePath() === null,
  );

  /// Computed CSS grid columns. Hidden panes collapse to `0` so the
  /// drag handles for them disappear; the chat column (1fr) takes
  /// whatever's left.
  const gridColumns = createMemo(() => {
    const s = sidebarHidden() ? "0" : `${sidebarWidth()}px`;
    const e = editorPaneHidden() ? "0" : `${editorWidth()}px`;
    const f = filesPaneHidden() ? "0" : `${filesWidth()}px`;
    return `${s} 1fr ${e} ${f}`;
  });

  /// Generic pointer-driven resize. Captures the start position once,
  /// then translates every move into a width delta. The min/max bounds
  /// are baked in so a user dragging into a corner can't reduce a pane
  /// below something usable. Persistence happens once on pointerup so
  /// we don't spam `~/.aictl/config` for every pixel.
  const startResize =
    (which: "sidebar" | "editor" | "files") => (e: PointerEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const initial =
        which === "sidebar"
          ? sidebarWidth()
          : which === "editor"
            ? editorWidth()
            : filesWidth();
      let last = initial;
      const clampWidth = (raw: number) => {
        if (which === "sidebar") {
          return Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, raw));
        }
        if (which === "editor") {
          return Math.max(EDITOR_MIN, Math.min(EDITOR_MAX, raw));
        }
        return Math.max(FILES_MIN, Math.min(FILES_MAX, raw));
      };
      const onMove = (ev: PointerEvent) => {
        const dx = ev.clientX - startX;
        // Sidebar grows right; editor + files grow left, so the delta
        // is inverted for those two.
        const next = clampWidth(
          which === "sidebar" ? initial + dx : initial - dx,
        );
        if (which === "sidebar") setSidebarWidth(next);
        else if (which === "editor") setEditorWidth(next);
        else setFilesWidth(next);
        last = next;
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        const key =
          which === "sidebar"
            ? "AICTL_DESKTOP_SIDEBAR_WIDTH"
            : which === "editor"
              ? "AICTL_DESKTOP_EDITOR_WIDTH"
              : "AICTL_DESKTOP_FILES_WIDTH";
        void ipc.configWrite(key, String(last));
      };
      // Lock cursor + suppress text selection app-wide so nothing on
      // the chat surface highlights as the user sweeps the handle.
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    };

  return (
    <div
      class="app"
      data-sidebar-hidden={String(sidebarHidden())}
      data-files-hidden={String(filesPaneHidden())}
      data-editor-hidden={String(editorPaneHidden())}
      style={{ "grid-template-columns": gridColumns() }}
    >
      <Titlebar
        workspace={workspace()}
        onPickWorkspace={pickWorkspace}
        turnInFlight={turnInFlight()}
        onStop={stop}
        sidebarVisible={sidebarVisible()}
        onToggleSidebar={toggleSidebar}
        contextPct={contextPct()}
        contextTokens={contextTokens()}
        onShowContextDetails={() => setShowContextDetails(true)}
        filesVisible={filesVisible()}
        onToggleFiles={toggleFiles}
        onOpenSettings={() => setShowSettings(true)}
        updateAvailable={latestVersion()}
        onShowUpdate={() => openUpdate(updateInfo())}
        onDismissUpdate={() => {
          const v = latestVersion();
          if (v) {
            try {
              if (typeof window !== "undefined") {
                window.localStorage.setItem("aictl.update.dismissed", v);
              }
            } catch {
              // localStorage unavailable — best-effort only.
            }
          }
          setLatestVersion(null);
        }}
      />
      <Show when={workspace().path}>
        <Sidebar
          activeSession={activeSession()}
          refreshKey={sessionRefreshKey()}
          onSelectSession={switchToSession}
          onNewSession={startNewSession}
          onNewIncognito={startIncognito}
          onDeleteSession={deleteSession}
          onClearAll={clearAllSessions}
          onRenameSession={renameSession}
        />
      </Show>
      <main class="main">
        <Show
          when={workspace().path}
          fallback={
            <EmptyWorkspace
              workspace={workspace()}
              onPick={pickWorkspace}
              onUseDefault={useDefaultWorkspace}
              onOpenSettings={() => setShowSettings(true)}
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
              onToolsEnabledChange={setToolsMasterEnabled}
              pluginsEnabled={pluginsEnabled()}
              onPluginsEnabledChange={setPluginsEnabledMaster}
              mcpEnabled={mcpEnabled()}
              onMcpEnabledChange={setMcpEnabledMaster}
              prefill={composerPrefill()}
              onPrefillConsumed={() => setComposerPrefill(null)}
              models={models()}
              activeModel={activeModel()}
              onChangeModel={changeModel}
              loadedSkill={loadedSkill()}
              onLoadedSkillChange={setLoadedSkill}
              loadedAgent={loadedAgent()}
              onLoadedAgentChange={setLoadedAgent}
              webEnabled={webEnabled()}
              onWebEnabledChange={setWebTools}
              imageEnabled={imageEnabled()}
              onImageEnabledChange={setImageTools}
              memoryEnabled={memoryEnabled()}
              onMemoryEnabledChange={setMemoryEnabledMaster}
              securityState={securityState()}
              securityChecks={securityChecks()}
              onOpenSecuritySettings={() => {
                setSettingsInitialTab("security");
                setShowSettings(true);
              }}
            />
          </div>
        </Show>
      </main>
      <Show when={workspace().path && openFilePath() !== null}>
        <EditorPane
          path={openFilePath()!}
          fsTick={fsTick()}
          onClose={() => setOpenFile(null)}
        />
      </Show>
      <Show when={workspace().path && filesVisible()}>
        <FilePane
          workspaceKey={workspace().path ?? ""}
          fsTick={fsTick()}
          onClose={toggleFiles}
          onOpenFile={(path) => setOpenFile(path)}
        />
      </Show>
      <Show when={!sidebarHidden()}>
        <div
          class="resize-handle"
          aria-hidden="true"
          style={{ left: `${sidebarWidth()}px` }}
          onPointerDown={startResize("sidebar")}
        />
      </Show>
      <Show when={!editorPaneHidden()}>
        <div
          class="resize-handle"
          aria-hidden="true"
          style={{
            right: `${(filesPaneHidden() ? 0 : filesWidth()) + editorWidth()}px`,
          }}
          onPointerDown={startResize("editor")}
        />
      </Show>
      <Show when={!filesPaneHidden()}>
        <div
          class="resize-handle"
          aria-hidden="true"
          style={{ right: `${filesWidth()}px` }}
          onPointerDown={startResize("files")}
        />
      </Show>
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
      <Show when={showContextDetails()}>
        <ContextDetails onClose={() => setShowContextDetails(false)} />
      </Show>
      <Show when={showUpdate()}>
        <UpdateModal
          initial={updateInfo()}
          onClose={() => setShowUpdate(false)}
        />
      </Show>
      <Show when={showProviderSetup()}>
        <ProviderSetup
          onPickTarget={(target: ProviderSetupTarget) => {
            const tab: SettingsTab =
              target === "keys"
                ? "keys"
                : target === "models"
                  ? "models"
                  : "server";
            setSettingsInitialTab(tab);
            setShowProviderSetup(false);
            setShowSettings(true);
          }}
          onDismiss={() => {
            setProviderSetupDismissed(true);
            setShowProviderSetup(false);
          }}
        />
      </Show>
      <Show when={showSettings()}>
        <Settings
          workspace={workspace()}
          onPickWorkspace={pickWorkspace}
          onClose={() => {
            setShowSettings(false);
            setSettingsInitialTab(undefined);
            void refreshToolsEnabled();
            void refreshApprovalDefault();
            void refreshWebEnabled();
            void refreshImageEnabled();
            void refreshMemoryEnabled();
            void refreshPluginsEnabled();
            void refreshNotifications();
            void refreshSecurityStatus();
            void refreshProviderSetup();
          }}
          models={models()}
          activeModel={activeModel()}
          onChangeModel={changeModel}
          onRefreshModels={refreshModels}
          initialTab={settingsInitialTab()}
          onShowUpdate={openUpdate}
        />
      </Show>
    </div>
  );
};

export default App;

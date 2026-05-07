import { Show, createSignal, onCleanup, onMount } from "solid-js";
import type { Component } from "solid-js";

import {
  checkUpdate,
  runUpdate,
  type UpdateInfo,
  type UpdateProgress,
} from "../lib/updater";
import { renderMarkdown } from "../lib/markdown";

interface Props {
  /// Optional pre-fetched update info; lets the caller skip the
  /// re-check round-trip when the titlebar already knows there's an
  /// update available. The modal still re-checks on mount when this
  /// is `null` so the version it offers to install matches the
  /// manifest at the time of click.
  initial: UpdateInfo | null;
  onClose: () => void;
}

const fmtBytes = (n: number): string => {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
};

const phaseLabel = (p: UpdateProgress["phase"]): string => {
  switch (p) {
    case "checking":
      return "Checking for updates…";
    case "downloading":
      return "Downloading new version…";
    case "installing":
      return "Installing…";
    case "restarting":
      return "Restarting aictl…";
    case "done":
      return "Up to date.";
    case "error":
      return "Update failed.";
  }
};

const UpdateModal: Component<Props> = (props) => {
  const [info, setInfo] = createSignal<UpdateInfo | null>(props.initial);
  const [busy, setBusy] = createSignal(false);
  const [progress, setProgress] = createSignal<UpdateProgress | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  const refresh = async () => {
    setError(null);
    try {
      setInfo(await checkUpdate());
    } catch (err) {
      setError(`${err}`);
    }
  };

  const start = async () => {
    if (busy()) return;
    setBusy(true);
    setError(null);
    try {
      await runUpdate((p) => setProgress(p));
      // If runUpdate resolves without restarting (no update / relaunch
      // hand-off), drop the busy flag so the user can dismiss.
      setBusy(false);
    } catch (err) {
      setProgress({
        phase: "error",
        totalBytes: null,
        downloadedBytes: 0,
        error: `${err}`,
      });
      setError(`${err}`);
      setBusy(false);
    }
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !busy()) {
      e.preventDefault();
      props.onClose();
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
    if (!props.initial) void refresh();
  });

  const onBackdropClick = (e: MouseEvent) => {
    if (busy()) return;
    if (e.target === e.currentTarget) props.onClose();
  };

  const pct = (): number | null => {
    const p = progress();
    if (!p) return null;
    if (p.totalBytes && p.totalBytes > 0) {
      return Math.min(
        100,
        Math.round((p.downloadedBytes / p.totalBytes) * 100),
      );
    }
    if (p.phase === "installing" || p.phase === "restarting") return 100;
    return null;
  };

  return (
    <div
      class="ctx-details-overlay"
      role="dialog"
      aria-modal="true"
      onClick={onBackdropClick}
    >
      <div class="ctx-details-panel">
        <header class="ctx-details-header">
          <h2>Update</h2>
          <Show when={!busy()}>
            <button
              type="button"
              class="ctx-details-close"
              aria-label="Close update dialog"
              title="Close (Esc)"
              onClick={props.onClose}
            >
              ✕
            </button>
          </Show>
        </header>
        <div class="ctx-details-body">
          <Show
            when={info()}
            fallback={
              <p class="ctx-details-meta">
                {error() ?? "Checking GitHub for the latest release…"}
              </p>
            }
          >
            {(u) => (
              <>
                <p class="ctx-details-hint">
                  A new version of aictl is available. The download replaces
                  the current bundle and relaunches automatically — no manual
                  drag into <code>/Applications</code> needed.
                </p>
                <div class="ctx-details-row">
                  <label>Current</label>
                  <div class="ctx-details-value">
                    <code>{u().currentVersion}</code>
                  </div>
                </div>
                <div class="ctx-details-row">
                  <label>Available</label>
                  <div class="ctx-details-value">
                    <code>{u().version}</code>
                    <Show when={u().date}>
                      <span class="ctx-details-meta-inline">
                        {" "}
                        · {u().date}
                      </span>
                    </Show>
                  </div>
                </div>
                <Show when={u().notes}>
                  <div class="ctx-details-row ctx-details-row-stack">
                    <label>Release notes</label>
                    {/* Server-side trust: notes come from latest.json
                        which we sign + control end-to-end. markdown-it
                        is configured with `html: false` so embedded
                        raw HTML is escaped, and `linkify: true` turns
                        bare URLs into anchors. The global click
                        delegator in App.tsx routes those anchors
                        through ipc.openUrl, so they open in the OS
                        browser instead of navigating the webview. */}
                    <div
                      class="update-notes"
                      // eslint-disable-next-line solid/no-innerhtml
                      innerHTML={renderMarkdown(u().notes ?? "")}
                    />
                  </div>
                </Show>
              </>
            )}
          </Show>

          <Show when={progress()}>
            {(p) => (
              <div class="update-progress">
                <div class="update-progress-label">{phaseLabel(p().phase)}</div>
                <div class="ctx-details-bar">
                  <div
                    class="ctx-details-fill"
                    classList={{ "update-progress-indet": pct() === null }}
                    style={
                      pct() !== null ? { width: `${pct()}%` } : undefined
                    }
                  />
                </div>
                <Show
                  when={p().phase === "downloading" && p().totalBytes}
                  fallback={
                    <p class="ctx-details-meta">
                      <Show when={p().phase === "downloading"}>
                        {fmtBytes(p().downloadedBytes)} downloaded
                      </Show>
                    </p>
                  }
                >
                  <p class="ctx-details-meta">
                    {fmtBytes(p().downloadedBytes)} /{" "}
                    {fmtBytes(p().totalBytes ?? 0)} ({pct()}%)
                  </p>
                </Show>
              </div>
            )}
          </Show>

          <Show when={error() && !progress()}>
            <p class="ctx-details-error">{error()}</p>
          </Show>
        </div>
        <footer class="ctx-details-footer">
          <Show when={!busy()}>
            <button type="button" onClick={() => void refresh()}>
              Re-check
            </button>
            <button type="button" onClick={props.onClose}>
              Not now
            </button>
          </Show>
          <Show when={info() && !busy()}>
            <button
              type="button"
              class="primary"
              onClick={() => void start()}
            >
              Install update
            </button>
          </Show>
        </footer>
      </div>
    </div>
  );
};

export default UpdateModal;

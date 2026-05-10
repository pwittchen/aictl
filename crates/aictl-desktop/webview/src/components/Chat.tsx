import type { Component } from "solid-js";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import L from "leaflet";
import "leaflet/dist/leaflet.css";

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

// `view_map` (see `crates/aictl-core/src/tools/view_map.rs`) emits a
// single-line marker `[view_map] {json}` that this component parses to
// render an OpenStreetMap embed. The Rust side only emits the marker
// when running under `Role::Desktop`, so the regex never matches in
// CLI-recorded transcripts replayed in the webview by accident.
const VIEW_MAP_RE = /^\[view_map\]\s+(\{[\s\S]*?\})\s*$/m;

interface ViewMapPin {
  lat: number;
  lon: number;
  label: string;
  description: string | null;
}

interface ViewMapData {
  query: string;
  /// Label of the *primary* (first) pin — displayed in the footer
  /// strip. Individual pins also carry their own labels for popups.
  label: string;
  /// Center for single-pin maps. With multiple pins the webview
  /// fits the viewport to enclose all of them and ignores these.
  lat: number;
  lon: number;
  /// `null` when the input had multiple pins (auto-fit instead).
  zoom: number | null;
  pins: ViewMapPin[];
}

// Inline SVG icons for the in-map light/dark toggle button. Inlined
// instead of pulled from a font/CDN so the button works under the
// current CSP without further allowlisting.
const SUN_ICON = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="8" cy="8" r="3"/><path d="M8 1v2"/><path d="M8 13v2"/><path d="M1 8h2"/><path d="M13 8h2"/><path d="M2.93 2.93l1.41 1.41"/><path d="M11.66 11.66l1.41 1.41"/><path d="M2.93 13.07l1.41-1.41"/><path d="M11.66 4.34l1.41-1.41"/></svg>`;
const MOON_ICON = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true"><path d="M6.5 1.5a6.5 6.5 0 1 0 8 8A5.5 5.5 0 0 1 6.5 1.5Z"/></svg>`;
const LAYERS_ICON = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M11.644 1.59a.75.75 0 0 1 .712 0l9.75 5.25a.75.75 0 0 1 0 1.32l-9.75 5.25a.75.75 0 0 1-.712 0l-9.75-5.25a.75.75 0 0 1 0-1.32l9.75-5.25Z" /><path d="m3.265 10.602 7.668 4.129a2.25 2.25 0 0 0 2.134 0l7.668-4.13 1.37.739a.75.75 0 0 1 0 1.32l-9.75 5.25a.75.75 0 0 1-.71 0l-9.75-5.25a.75.75 0 0 1 0-1.32l1.37-.738Z" /><path d="m10.933 19.231-7.668-4.13-1.37.739a.75.75 0 0 0 0 1.32l9.75 5.25c.221.12.489.12.71 0l9.75-5.25a.75.75 0 0 0 0-1.32l-1.37-.738-7.668 4.13a2.25 2.25 0 0 1-2.134-.001Z" /></svg>`;

/// Resolve the effective app theme. Returns `true` when the desktop
/// chrome is rendering in dark mode — used to pick the matching
/// initial map tile style. Mirrors the logic in `applyAppearance`
/// (see Settings.tsx): explicit `data-theme` wins; otherwise we
/// follow the OS preference via `prefers-color-scheme`.
function detectAppIsDark(): boolean {
  const attr = document.documentElement.getAttribute("data-theme");
  if (attr === "light") return false;
  if (attr === "dark") return true;
  // System mode — match the OS. `matches` is `true` when the user
  // has set their OS to dark, `false` for light. No-`matchMedia`
  // fallback (e.g. headless test) defaults to dark since that's the
  // app's default theme.
  if (typeof window === "undefined" || !window.matchMedia) return true;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

// Leaflet renders popup content as raw HTML. Pin labels and
// descriptions originate in tool input authored by the model, so
// escape on the way in to keep the popup contents inert.
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderPopup(pin: ViewMapPin): string {
  const head = `<strong>${escapeHtml(pin.label)}</strong>`;
  const coords = `<div class="aictl-map-popup-coords">lat ${pin.lat.toFixed(5)}, lon ${pin.lon.toFixed(5)}</div>`;
  const body = pin.description
    ? `<div class="aictl-map-popup-desc">${escapeHtml(pin.description)}</div>`
    : "";
  return `${head}${body}${coords}`;
}

function extractViewMap(result: string | undefined): ViewMapData | null {
  if (!result) return null;
  const match = result.match(VIEW_MAP_RE);
  if (!match) return null;
  try {
    const parsed = JSON.parse(match[1]) as Record<string, unknown>;
    if (
      typeof parsed.lat !== "number" ||
      typeof parsed.lon !== "number" ||
      typeof parsed.label !== "string"
    ) {
      return null;
    }
    const rawPins = Array.isArray(parsed.pins) ? parsed.pins : [];
    const pins: ViewMapPin[] = rawPins
      .map((p): ViewMapPin | null => {
        if (!p || typeof p !== "object") return null;
        const obj = p as Record<string, unknown>;
        if (typeof obj.lat !== "number" || typeof obj.lon !== "number") {
          return null;
        }
        const label =
          typeof obj.label === "string" && obj.label.length > 0
            ? obj.label
            : `${obj.lat.toFixed(5)}, ${obj.lon.toFixed(5)}`;
        const description =
          typeof obj.description === "string" && obj.description.length > 0
            ? obj.description
            : null;
        return { lat: obj.lat, lon: obj.lon, label, description };
      })
      .filter((p): p is ViewMapPin => p !== null);
    if (pins.length === 0) {
      // Older marker payloads (and any JSON that lost its `pins`
      // array in transit) still carry the primary lat/lon — treat
      // those as a single-pin map so we never render an empty view.
      pins.push({
        lat: parsed.lat,
        lon: parsed.lon,
        label: parsed.label,
        description: null,
      });
    }
    return {
      query: typeof parsed.query === "string" ? parsed.query : parsed.label,
      label: parsed.label,
      lat: parsed.lat,
      lon: parsed.lon,
      zoom: typeof parsed.zoom === "number" ? parsed.zoom : null,
      pins,
    };
  } catch {
    return null;
  }
}

interface Props {
  messages: Message[];
  streamingText: string;
  streaming: boolean;
  busy: boolean;
}

const Chat: Component<Props> = (props) => {
  let scroller: HTMLDivElement | undefined;

  const scrollToBottom = () => {
    if (!scroller) return;
    // Defer to the next animation frame so Solid's pending DOM
    // mutations and the browser's layout pass are both done before
    // we read `scrollHeight` — otherwise it's a frame stale and the
    // view stops a single message short of the bottom.
    requestAnimationFrame(() => {
      if (!scroller) return;
      scroller.scrollTop = scroller.scrollHeight;
    });
  };

  // Auto-scroll on every message append, stream chunk, stream
  // start/stop, and busy-state change. Solid effects re-run whenever
  // any tracked signal upstream updates.
  createEffect(() => {
    void props.messages.length;
    void props.streamingText;
    void props.streaming;
    void props.busy;
    scrollToBottom();
  });

  // Catch *late* height changes that the prop-driven effect can't see:
  // a Leaflet map measuring its container after `onMount`, an image
  // decoding, a code block getting rewrapped by highlight.js, etc.
  // Without this the view freezes one frame short of the bottom every
  // time a message ends in a tool result that mounts complex content.
  onMount(() => {
    if (!scroller) return;
    const ro = new ResizeObserver(() => scrollToBottom());
    const observeChildren = () => {
      if (!scroller) return;
      for (const child of Array.from(scroller.children)) {
        ro.observe(child);
      }
    };
    observeChildren();
    // The For/Show pair adds and removes message nodes as the
    // conversation grows; re-observe whenever the child list mutates
    // so newly-mounted bubbles are tracked too.
    const mo = new MutationObserver(() => {
      ro.disconnect();
      observeChildren();
    });
    mo.observe(scroller, { childList: true });
    onCleanup(() => {
      ro.disconnect();
      mo.disconnect();
    });
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
          <Show when={extractViewMap(result())}>
            {(d) => <ToolMapView data={d()} />}
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

const ToolMapView: Component<{ data: ViewMapData }> = (props) => {
  let mapEl: HTMLDivElement | undefined;
  let mapInstance: L.Map | undefined;

  const externalUrl = () => {
    const d = props.data;
    return `https://www.openstreetmap.org/?mlat=${d.lat}&mlon=${d.lon}#map=${d.zoom}/${d.lat}/${d.lon}`;
  };

  const openExternal = (e: MouseEvent) => {
    e.preventDefault();
    void ipc.openUrl(externalUrl());
  };

  // Nominatim's `display_name` is comma-separated from most- to
  // least-specific (e.g. "Eiffel Tower, Avenue Anatole France, Paris,
  // Ile-de-France, France"). The first two parts are almost always the
  // useful bit; the rest is admin-region noise that pushes the
  // "open in OpenStreetMap →" link onto a second line in narrow chat
  // columns. Keep the full string in `title` for hover.
  const shortLabel = () => {
    const parts = props.data.label
      .split(",")
      .map((p) => p.trim())
      .filter(Boolean);
    return parts.slice(0, 2).join(", ") || props.data.label;
  };

  const footerText = () => {
    const n = props.data.pins.length;
    if (n <= 1) return shortLabel();
    return `${n} pins · click any pin for details`;
  };

  onMount(() => {
    if (!mapEl) return;
    const d = props.data;
    const map = L.map(mapEl, {
      zoomControl: true,
      attributionControl: true,
    });

    // CartoDB ships matched light + dark variants of an OSM-derived
    // street map (Positron / Dark Matter). Both are free-tier, used
    // widely, and serve the same OSM-contributors data — so swapping
    // between them is a pure visual change without losing geography.
    const cartoAttribution =
      '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors &copy; <a href="https://carto.com/attributions">CARTO</a>';
    const lightTiles = L.tileLayer(
      "https://{s}.basemaps.cartocdn.com/light_all/{z}/{x}/{y}{r}.png",
      { maxZoom: 19, attribution: cartoAttribution, subdomains: "abcd" },
    );
    const darkTiles = L.tileLayer(
      "https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}{r}.png",
      { maxZoom: 19, attribution: cartoAttribution, subdomains: "abcd" },
    );

    const satellite = L.tileLayer(
      "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}",
      {
        maxZoom: 19,
        attribution:
          "Tiles &copy; Esri &mdash; Source: Esri, Maxar, Earthstar Geographics, and the GIS User Community",
      },
    );

    // The "Map" entry in the layer control points at this group; we
    // swap its inner tile layer when the user toggles light/dark, so
    // the layer-control selection remains stable across toggles.
    let isDark = detectAppIsDark();
    const mapBase = L.layerGroup([isDark ? darkTiles : lightTiles]);

    mapBase.addTo(map);

    // Tag the map container with the active tile theme so the popup
    // CSS can flip with the map (not the app) — a light-tile map
    // gets a light popup with dark text, dark-tile gets the inverse,
    // regardless of which theme the surrounding chat is in.
    const applyMapThemeClass = () => {
      if (!mapEl) return;
      mapEl.classList.toggle("aictl-map-light-tiles", !isDark);
      mapEl.classList.toggle("aictl-map-dark-tiles", isDark);
    };
    applyMapThemeClass();

    // Bottom-left layer switcher: a single icon button (matching the
    // size and surface of the zoom buttons) that pops a small panel
    // upward with Map / Satellite radios. Replaces Leaflet's stock
    // `L.control.layers` widget so the chrome stays unobtrusive on
    // the small in-chat map and the user only sees the icon until
    // they want to switch.
    type LayerKind = "map" | "satellite";
    let activeLayer: LayerKind = "map";

    const LayerSwitcher = L.Control.extend({
      onAdd(controlMap: L.Map) {
        const container = L.DomUtil.create(
          "div",
          "aictl-map-layer-switcher leaflet-bar",
        );
        // Stop map drag/click bleed-through for the entire control
        // (button + panel) — without this, dragging on the panel
        // pans the map.
        L.DomEvent.disableClickPropagation(container);
        L.DomEvent.disableScrollPropagation(container);

        const button = L.DomUtil.create(
          "button",
          "aictl-map-layer-button",
          container,
        );
        button.type = "button";
        button.title = "Switch map layer";
        button.setAttribute("aria-label", "Switch map layer");
        button.setAttribute("aria-haspopup", "true");
        button.setAttribute("aria-expanded", "false");
        button.innerHTML = LAYERS_ICON;

        const panel = L.DomUtil.create(
          "div",
          "aictl-map-layer-panel",
          container,
        );
        panel.setAttribute("role", "menu");
        panel.hidden = true;

        const renderPanel = () => {
          panel.innerHTML = "";
          const options: { kind: LayerKind; label: string }[] = [
            { kind: "map", label: "Map" },
            { kind: "satellite", label: "Satellite" },
          ];
          for (const opt of options) {
            const row = L.DomUtil.create(
              "button",
              "aictl-map-layer-option",
              panel,
            );
            row.type = "button";
            row.dataset.layer = opt.kind;
            row.textContent = opt.label;
            row.setAttribute("role", "menuitemradio");
            row.setAttribute(
              "aria-checked",
              opt.kind === activeLayer ? "true" : "false",
            );
            if (opt.kind === activeLayer) row.classList.add("is-active");
            L.DomEvent.on(row, "click", L.DomEvent.stop);
            L.DomEvent.on(row, "click", () => selectLayer(opt.kind));
          }
        };

        const setOpen = (open: boolean) => {
          panel.hidden = !open;
          button.setAttribute("aria-expanded", open ? "true" : "false");
          container.classList.toggle("is-open", open);
        };

        const selectLayer = (kind: LayerKind) => {
          if (kind !== activeLayer) {
            if (kind === "map") {
              controlMap.removeLayer(satellite);
              controlMap.addLayer(mapBase);
            } else {
              controlMap.removeLayer(mapBase);
              controlMap.addLayer(satellite);
            }
            activeLayer = kind;
          }
          setOpen(false);
        };

        L.DomEvent.on(button, "click", L.DomEvent.stop);
        L.DomEvent.on(button, "click", () => {
          const opening = panel.hidden;
          if (opening) renderPanel();
          setOpen(opening);
        });

        // Click anywhere else on the map closes the panel — same
        // affordance as a desktop dropdown.
        controlMap.on("click", () => setOpen(false));

        return container;
      },
    });
    new LayerSwitcher({ position: "bottomleft" }).addTo(map);

    // Custom Leaflet control: a single sun/moon button that swaps
    // the inner tile layer of `mapBase`. Lives top-left so it doesn't
    // collide with the existing layer-control top-right or the zoom
    // buttons (which are also top-left by default — Leaflet stacks
    // them vertically inside the same container).
    const ThemeToggle = L.Control.extend({
      onAdd() {
        const btn = L.DomUtil.create(
          "button",
          "aictl-map-theme-toggle leaflet-bar",
        );
        btn.type = "button";
        btn.title = "Toggle light / dark map tiles";
        btn.setAttribute("aria-label", "Toggle light / dark map tiles");
        const renderIcon = () => {
          // Show the icon for the *target* style — clicking the moon
          // gives you dark, clicking the sun gives you light, which
          // matches the convention used in most app theme toggles.
          btn.innerHTML = isDark ? SUN_ICON : MOON_ICON;
        };
        renderIcon();
        L.DomEvent.on(btn, "click", L.DomEvent.stopPropagation);
        L.DomEvent.on(btn, "mousedown", L.DomEvent.stopPropagation);
        L.DomEvent.on(btn, "dblclick", L.DomEvent.stopPropagation);
        L.DomEvent.on(btn, "click", () => {
          isDark = !isDark;
          mapBase.clearLayers();
          mapBase.addLayer(isDark ? darkTiles : lightTiles);
          renderIcon();
          applyMapThemeClass();
        });
        return btn;
      },
    });
    new ThemeToggle({ position: "topleft" }).addTo(map);

    const pinIcon = L.divIcon({
      className: "aictl-pin-wrap",
      html: '<div class="aictl-pin"></div>',
      iconSize: [14, 14],
      iconAnchor: [7, 7],
    });

    for (const pin of d.pins) {
      const marker = L.marker([pin.lat, pin.lon], { icon: pinIcon }).addTo(
        map,
      );
      marker.bindPopup(renderPopup(pin), {
        // Slightly wider than the default so longer labels and
        // descriptions don't immediately wrap awkwardly.
        maxWidth: 280,
        className: "aictl-map-popup",
      });
    }

    if (d.pins.length > 1) {
      // Auto-fit the viewport so every pin is visible. `padding`
      // keeps the outermost pins comfortably inside the frame
      // instead of pinned right at the edge.
      const bounds = L.latLngBounds(
        d.pins.map((p) => [p.lat, p.lon] as [number, number]),
      );
      map.fitBounds(bounds, { padding: [32, 32], maxZoom: 16 });
    } else {
      map.setView([d.lat, d.lon], d.zoom ?? 13);
    }

    mapInstance = map;

    // Leaflet measures the container synchronously on init; if the
    // chat column is still animating in (or the tool callout grew via
    // streaming), the initial size can be wrong and tiles render
    // misaligned. Schedule a one-shot invalidate on the next frame to
    // pick up the final layout box.
    requestAnimationFrame(() => mapInstance?.invalidateSize());
  });

  onCleanup(() => {
    mapInstance?.remove();
    mapInstance = undefined;
  });

  return (
    <div
      style={{
        "margin-top": "8px",
        "border-radius": "4px",
        border: "1px solid var(--border)",
        overflow: "hidden",
        background: "var(--bg)",
      }}
    >
      <div
        ref={mapEl}
        style={{
          width: "100%",
          height: "320px",
          background: "var(--bg)",
        }}
      />
      <div
        style={{
          padding: "6px 10px",
          "font-size": "11px",
          color: "var(--fg-soft)",
          display: "flex",
          "flex-wrap": "nowrap",
          "justify-content": "space-between",
          "align-items": "center",
          gap: "8px",
          "border-top": "1px solid var(--border)",
        }}
      >
        <span
          style={{
            "white-space": "nowrap",
            overflow: "hidden",
            "text-overflow": "ellipsis",
            "min-width": "0",
            flex: "1 1 auto",
          }}
          title={props.data.label}
        >
          {footerText()}
        </span>
        <a
          href={externalUrl()}
          onClick={openExternal}
          style={{
            color: "var(--accent)",
            "text-decoration": "none",
            "white-space": "nowrap",
            "flex-shrink": "0",
          }}
        >
          open in OpenStreetMap →
        </a>
      </div>
    </div>
  );
};

const LoadingDots: Component = () => (
  <span class="loading-dots" role="status" aria-label="loading">
    <span /><span /><span />
  </span>
);

export default Chat;

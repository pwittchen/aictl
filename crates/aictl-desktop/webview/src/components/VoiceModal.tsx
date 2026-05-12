import type { Component } from "solid-js";
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";

import { ipc, type AgentEvent } from "../lib/ipc";

interface Props {
  /// Called with the transcribed text when the user accepts. The parent
  /// is responsible for inserting the string into the composer's
  /// textarea and dismissing the modal.
  onAccept: (text: string) => void;
  /// Called when the user cancels (button, Esc, or backdrop click). The
  /// parent should dismiss the modal — `VoiceModal` already takes care
  /// of stopping the mic + tearing the AudioContext down.
  onCancel: () => void;
  /// Surfaced inside the modal header so the user knows which Whisper
  /// model is being used. Also used as the auto-download spinner label.
  modelLabel: string;
  /// `false` until `voice_ensure_model` confirms the model is on disk.
  /// When `false` the modal opens in download mode (progress bar,
  /// Cancel button, terminal "ready" state on success); when `true`
  /// it goes straight to the recording flow.
  modelPresent: boolean;
}

type Phase =
  /// Active download — listens to `progress_*` events on the agent
  /// channel and renders a real percentage. Cancelling here aborts the
  /// download and removes the `.part` file.
  | { kind: "downloading"; current: number; total: number | null }
  /// Terminal success state for a download that just completed. The
  /// recording flow does NOT auto-start; the user must close this and
  /// click the mic again, matching the explicit confirmation the
  /// product spec asked for.
  | { kind: "download_complete" }
  | { kind: "asking_permission" }
  | { kind: "recording" }
  | { kind: "transcribing" }
  | { kind: "error"; message: string };

const BAR_COUNT = 36;
const TARGET_SAMPLE_RATE = 16_000;

const VoiceModal: Component<Props> = (props) => {
  const [phase, setPhase] = createSignal<Phase>(
    props.modelPresent
      ? { kind: "asking_permission" }
      : { kind: "downloading", current: 0, total: null },
  );
  const [bars, setBars] = createSignal<number[]>(
    Array.from({ length: BAR_COUNT }, () => 0.05),
  );

  let stream: MediaStream | undefined;
  let audioContext: AudioContext | undefined;
  let analyser: AnalyserNode | undefined;
  let processor: ScriptProcessorNode | undefined;
  let silenceGain: GainNode | undefined;
  let source: MediaStreamAudioSourceNode | undefined;
  let rafHandle: number | undefined;
  // Captured PCM samples at the AudioContext's nominal sample rate
  // (`TARGET_SAMPLE_RATE` when the platform honours the hint, otherwise
  // the device's native rate — we resample below before shipping to
  // Rust so the engine always receives 16 kHz mono).
  let chunks: Float32Array[] = [];
  let nativeSampleRate = TARGET_SAMPLE_RATE;
  // Fired by the keydown handler before `accept` finishes; the second
  // press should be ignored, otherwise we send an empty buffer.
  let acceptInFlight = false;

  // The id minted by `progress_begin` for the model-download bar. We
  // capture it on the first matching `progress_begin` event and use it
  // to filter subsequent updates — without this, an unrelated download
  // (e.g. a GGUF model in the background) would write into our bar.
  let progressId: number | null = null;
  let unlistenProgress: (() => void) | undefined;
  // Set when the user cancels during the download phase. The
  // `progress_end` listener checks it so we don't flip into the
  // "download_complete" phase after we've already torn the modal down.
  let downloadCancelled = false;

  const cleanupAudio = () => {
    if (rafHandle !== undefined) {
      cancelAnimationFrame(rafHandle);
      rafHandle = undefined;
    }
    if (processor) {
      processor.disconnect();
      processor.onaudioprocess = null;
      processor = undefined;
    }
    if (analyser) {
      analyser.disconnect();
      analyser = undefined;
    }
    if (silenceGain) {
      silenceGain.disconnect();
      silenceGain = undefined;
    }
    if (source) {
      source.disconnect();
      source = undefined;
    }
    if (stream) {
      stream.getTracks().forEach((t) => t.stop());
      stream = undefined;
    }
    if (audioContext) {
      void audioContext.close();
      audioContext = undefined;
    }
  };

  const cleanupProgress = () => {
    if (unlistenProgress) {
      unlistenProgress();
      unlistenProgress = undefined;
    }
    progressId = null;
  };

  // Subscribe to the `agent_event` channel and forward the model's
  // progress events into local phase state. Filters by the
  // `model_label` prop so a co-running GGUF/MLX download never bleeds
  // into our bar.
  const wireProgressEvents = async () => {
    const expectedLabel = props.modelLabel;
    unlistenProgress = await ipc.onAgentEvent((e: AgentEvent) => {
      if (e.kind === "progress_begin" && e.label === expectedLabel) {
        progressId = e.id;
        setPhase({
          kind: "downloading",
          current: 0,
          total: e.total,
        });
        return;
      }
      if (progressId === null) return;
      if (e.kind === "progress_update" && e.id === progressId) {
        const cur = phase();
        if (cur.kind === "downloading") {
          setPhase({
            kind: "downloading",
            current: e.current,
            total: cur.total,
          });
        }
        return;
      }
      if (e.kind === "progress_end" && e.id === progressId) {
        progressId = null;
        if (downloadCancelled) return;
        // The download routine emits two distinct end messages; treat
        // the cancel one as an error rather than success since the
        // model isn't on disk in that case.
        const msg = e.message ?? "";
        if (msg.includes("cancelled")) {
          setPhase({
            kind: "error",
            message: "Model download was cancelled.",
          });
        } else {
          setPhase({ kind: "download_complete" });
        }
      }
    });
  };

  const startDownload = async () => {
    downloadCancelled = false;
    await wireProgressEvents();
    try {
      const result = await ipc.voiceEnsureModel();
      // If the model was already present (race with another opener), we
      // jump straight to the ready confirmation so the UX is consistent.
      if (!result.started && result.status.model_present) {
        cleanupProgress();
        setPhase({ kind: "download_complete" });
      }
    } catch (err) {
      cleanupProgress();
      setPhase({
        kind: "error",
        message: `Failed to start download: ${err}`,
      });
    }
  };

  const startRecording = async () => {
    setPhase({ kind: "asking_permission" });
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
    } catch (err) {
      setPhase({
        kind: "error",
        message:
          err instanceof Error && err.name === "NotAllowedError"
            ? "Microphone access denied. Grant access in System Settings → Privacy & Security → Microphone."
            : `Could not start microphone: ${err}`,
      });
      return;
    }

    // Construction may throw in WKWebView when the requested sample rate
    // is unsupported — fall back to the device default and resample on
    // accept.
    try {
      audioContext = new AudioContext({ sampleRate: TARGET_SAMPLE_RATE });
    } catch {
      audioContext = new AudioContext();
    }
    nativeSampleRate = audioContext.sampleRate;

    source = audioContext.createMediaStreamSource(stream);
    analyser = audioContext.createAnalyser();
    analyser.fftSize = 64;
    analyser.smoothingTimeConstant = 0.6;

    processor = audioContext.createScriptProcessor(4096, 1, 1);
    processor.onaudioprocess = (e) => {
      const input = e.inputBuffer.getChannelData(0);
      // Copy: the underlying buffer is reused by the audio engine.
      chunks.push(new Float32Array(input));
    };

    // ScriptProcessor only runs when its output is connected to the
    // graph's destination. Pipe through a 0-gain node so we get the
    // callbacks without echoing the user's voice back into the speakers.
    silenceGain = audioContext.createGain();
    silenceGain.gain.value = 0;

    source.connect(analyser);
    source.connect(processor);
    processor.connect(silenceGain);
    silenceGain.connect(audioContext.destination);

    chunks = [];
    acceptInFlight = false;

    // Live frequency-bar animation. `getByteFrequencyData` returns
    // 0..255 per bin; reduce to BAR_COUNT bins by averaging.
    const freq = new Uint8Array(analyser.frequencyBinCount);
    const tick = () => {
      if (!analyser) return;
      analyser.getByteFrequencyData(freq);
      const next = new Array<number>(BAR_COUNT);
      const binsPerBar = Math.max(1, Math.floor(freq.length / BAR_COUNT));
      for (let i = 0; i < BAR_COUNT; i += 1) {
        let sum = 0;
        for (let j = 0; j < binsPerBar; j += 1) {
          sum += freq[i * binsPerBar + j] ?? 0;
        }
        const avg = sum / binsPerBar / 255;
        // Curve the response so quiet speech still pushes the bars
        // visibly off the floor without saturating on a loud syllable.
        next[i] = Math.max(0.05, Math.min(1, Math.pow(avg, 0.7)));
      }
      setBars(next);
      rafHandle = requestAnimationFrame(tick);
    };
    rafHandle = requestAnimationFrame(tick);

    setPhase({ kind: "recording" });
  };

  const accept = async () => {
    if (acceptInFlight) return;
    if (phase().kind !== "recording") return;
    acceptInFlight = true;
    setPhase({ kind: "transcribing" });

    // Stop the input chain immediately so the textarea doesn't keep
    // capturing while whisper crunches the buffer.
    if (rafHandle !== undefined) {
      cancelAnimationFrame(rafHandle);
      rafHandle = undefined;
    }
    if (processor) {
      processor.disconnect();
      processor.onaudioprocess = null;
    }
    if (source) source.disconnect();
    if (stream) stream.getTracks().forEach((t) => t.stop());

    const totalLen = chunks.reduce((acc, c) => acc + c.length, 0);
    const captured = new Float32Array(totalLen);
    {
      let offset = 0;
      for (const c of chunks) {
        captured.set(c, offset);
        offset += c.length;
      }
    }

    // Resample to 16 kHz mono if the AudioContext didn't honour the hint
    // (Safari pre-15 ignored `sampleRate` and gave back 48 kHz). Linear
    // interpolation is fine for whisper — it does its own preprocessing.
    let samples: Float32Array;
    if (Math.abs(nativeSampleRate - TARGET_SAMPLE_RATE) < 1) {
      samples = captured;
    } else {
      const ratio = TARGET_SAMPLE_RATE / nativeSampleRate;
      const outLen = Math.floor(captured.length * ratio);
      samples = new Float32Array(outLen);
      for (let i = 0; i < outLen; i += 1) {
        const srcIdx = i / ratio;
        const lo = Math.floor(srcIdx);
        const hi = Math.min(captured.length - 1, lo + 1);
        const frac = srcIdx - lo;
        samples[i] =
          (captured[lo] ?? 0) * (1 - frac) + (captured[hi] ?? 0) * frac;
      }
    }

    if (audioContext) {
      void audioContext.close();
      audioContext = undefined;
    }

    try {
      const text = await ipc.voiceTranscribe(samples);
      cleanupAudio();
      props.onAccept(text);
    } catch (err) {
      setPhase({
        kind: "error",
        message: `Transcription failed: ${err}`,
      });
      acceptInFlight = false;
    }
  };

  /// Single dismiss path. Branches by phase: a download in flight gets
  /// torn down through `voice_cancel_download`; everything else just
  /// stops the audio chain and calls back to the parent.
  const cancel = () => {
    const p = phase();
    if (p.kind === "downloading") {
      downloadCancelled = true;
      void ipc.voiceCancelDownload().catch(() => {
        // Best-effort: even if the IPC fails the local flag prevents the
        // pending `progress_end` from flipping us into "complete".
      });
    }
    cleanupAudio();
    cleanupProgress();
    props.onCancel();
  };

  /// Terminal "Close" path used by the download_complete phase. Same
  /// teardown as `cancel` but doesn't fire the cancel IPC (the
  /// download already finished cleanly).
  const close = () => {
    cleanupAudio();
    cleanupProgress();
    props.onCancel();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      cancel();
      return;
    }
    if (e.key === "Enter" && !e.metaKey && !e.ctrlKey && !e.shiftKey) {
      const k = phase().kind;
      if (k === "recording") {
        e.preventDefault();
        e.stopPropagation();
        void accept();
        return;
      }
      if (k === "download_complete") {
        // Plain Enter on the "ready" screen acts as the Close button so
        // a keyboard-only user doesn't have to mouse over to dismiss.
        e.preventDefault();
        e.stopPropagation();
        close();
      }
    }
  };

  onMount(() => {
    document.addEventListener("keydown", onKeyDown, true);
    if (props.modelPresent) {
      void startRecording();
    } else {
      void startDownload();
    }
  });

  onCleanup(() => {
    document.removeEventListener("keydown", onKeyDown, true);
    cleanupAudio();
    cleanupProgress();
  });

  const downloadPercent = () => {
    const p = phase();
    if (p.kind !== "downloading") return null;
    if (!p.total || p.total === 0) return null;
    return Math.min(100, Math.floor((p.current / p.total) * 100));
  };

  const formatBytes = (bytes: number): string => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  };

  return (
    <Portal mount={document.body}>
      <div
        class="voice-modal-backdrop"
        role="dialog"
        aria-modal="true"
        aria-label="Voice transcription"
        onMouseDown={(e) => {
          // Click outside the panel cancels (or closes — `cancel` does
          // the right thing for both download and recording phases).
          if (e.target === e.currentTarget) cancel();
        }}
      >
        <div class="voice-modal" role="document">
          <div class="voice-modal-header">
            <span class="voice-modal-title">
              {phase().kind === "downloading" ||
              phase().kind === "download_complete"
                ? "voice model"
                : "voice transcription"}
            </span>
            <span class="voice-modal-model">{props.modelLabel}</span>
          </div>

          <div class="voice-modal-body">
            <Show
              when={
                phase().kind === "downloading" &&
                (phase() as Extract<Phase, { kind: "downloading" }>)
              }
            >
              {(p) => (
                <div class="voice-modal-download">
                  <div class="voice-modal-message">
                    Downloading Whisper model…
                    <div class="voice-modal-hint">
                      First run only — about 140 MB. Audio is processed
                      on-device once the model is in place.
                    </div>
                  </div>
                  <div
                    class="voice-modal-progress"
                    role="progressbar"
                    aria-valuemin="0"
                    aria-valuemax={p().total ?? undefined}
                    aria-valuenow={p().current}
                  >
                    <div
                      class="voice-modal-progress-fill"
                      data-indeterminate={
                        p().total === null ? "true" : "false"
                      }
                      style={
                        downloadPercent() !== null
                          ? { width: `${downloadPercent()}%` }
                          : {}
                      }
                    />
                  </div>
                  <div class="voice-modal-progress-meta">
                    <span>
                      {downloadPercent() !== null
                        ? `${downloadPercent()}%`
                        : "starting…"}
                    </span>
                    <span>
                      {formatBytes(p().current)}
                      {p().total !== null
                        ? ` / ${formatBytes(p().total ?? 0)}`
                        : ""}
                    </span>
                  </div>
                </div>
              )}
            </Show>
            <Show when={phase().kind === "download_complete"}>
              <div class="voice-modal-status">
                <div class="voice-modal-check" aria-hidden="true">
                  ✓
                </div>
                <div class="voice-modal-message">
                  Whisper model is ready.
                  <div class="voice-modal-hint">
                    Close this dialog and click the microphone icon
                    again to start voice-to-text transcription.
                  </div>
                </div>
              </div>
            </Show>
            <Show when={phase().kind === "asking_permission"}>
              <div class="voice-modal-status">
                <div class="voice-modal-spinner" aria-hidden="true" />
                <div class="voice-modal-message">
                  Waiting for microphone permission…
                </div>
              </div>
            </Show>
            <Show when={phase().kind === "recording"}>
              <div
                class="voice-modal-visualizer"
                aria-label="Live microphone level"
              >
                <For each={bars()}>
                  {(level) => (
                    <span
                      class="voice-modal-bar"
                      style={{
                        transform: `scaleY(${Math.max(0.06, level)})`,
                      }}
                    />
                  )}
                </For>
              </div>
              <div class="voice-modal-message">Listening… speak now.</div>
            </Show>
            <Show when={phase().kind === "transcribing"}>
              <div class="voice-modal-status">
                <div class="voice-modal-spinner" aria-hidden="true" />
                <div class="voice-modal-message">Transcribing…</div>
              </div>
            </Show>
            <Show when={phase().kind === "error"}>
              {(_) => {
                const p = phase();
                if (p.kind !== "error") return null;
                return (
                  <div class="voice-modal-error" role="alert">
                    {p.message}
                  </div>
                );
              }}
            </Show>
          </div>

          <div class="voice-modal-footer">
            <Show
              when={phase().kind === "download_complete"}
              fallback={
                <button
                  type="button"
                  class="voice-modal-cancel"
                  onClick={cancel}
                >
                  Cancel <kbd>Esc</kbd>
                </button>
              }
            >
              <button
                type="button"
                class="voice-modal-cancel"
                onClick={close}
              >
                Close <kbd>Esc</kbd>
              </button>
            </Show>
            <Show
              when={
                phase().kind === "recording" ||
                phase().kind === "transcribing"
              }
            >
              <button
                type="button"
                class="voice-modal-accept"
                disabled={phase().kind !== "recording"}
                onClick={() => void accept()}
              >
                Accept <kbd>↵</kbd>
              </button>
            </Show>
          </div>
        </div>
      </div>
    </Portal>
  );
};

export default VoiceModal;

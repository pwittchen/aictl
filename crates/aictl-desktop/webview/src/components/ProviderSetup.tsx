import type { Component } from "solid-js";
import { onCleanup, onMount } from "solid-js";

export type ProviderSetupTarget = "keys" | "models" | "server";

interface Props {
  onPickTarget: (target: ProviderSetupTarget) => void;
  onDismiss: () => void;
}

const ProviderSetup: Component<Props> = (props) => {
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      props.onDismiss();
    }
  };

  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <div class="tool-modal" role="dialog" aria-modal="true">
      <div class="panel provider-setup">
        <h2>Pick a model provider</h2>
        <p class="provider-setup__lede">
          aictl-desktop needs at least one way to reach a language
          model before you can chat. Set up an API key, run a model
          locally, or point it at an LLM server. You can mix and match
          later from Settings.
        </p>
        <div class="provider-setup__options">
          <button
            type="button"
            class="provider-setup__option"
            onClick={() => props.onPickTarget("keys")}
          >
            <span class="provider-setup__option-title">
              Set up an API key
            </span>
            <span class="provider-setup__option-desc">
              OpenAI, Anthropic, Gemini, Mistral, DeepSeek, Grok, Kimi,
              Z.ai
            </span>
          </button>
          <button
            type="button"
            class="provider-setup__option"
            onClick={() => props.onPickTarget("models")}
          >
            <span class="provider-setup__option-title">
              Run a model locally
            </span>
            <span class="provider-setup__option-desc">
              Download a GGUF or MLX model — no API key, no network
            </span>
          </button>
          <button
            type="button"
            class="provider-setup__option"
            onClick={() => props.onPickTarget("server")}
          >
            <span class="provider-setup__option-title">
              Connect to an LLM server
            </span>
            <span class="provider-setup__option-desc">
              Point at an Ollama daemon or a self-hosted aictl-server
            </span>
          </button>
        </div>
        <div class="provider-setup__footer">
          <button
            type="button"
            class="provider-setup__skip"
            onClick={props.onDismiss}
          >
            Skip for now
          </button>
        </div>
      </div>
    </div>
  );
};

export default ProviderSetup;

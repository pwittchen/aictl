# Plan: Sensitive Data Redaction in aictl

## Context

`aictl` already ships a hardened security layer: a `SecurityPolicy` in `src/security.rs` gates every tool call, the CWD jail constrains file access, environment scrubbing keeps secrets out of shell subprocesses, and `detect_prompt_injection` stops obvious override attempts at the top of `run_agent_turn`. What it does **not** do today is inspect the *payload* of messages flowing to remote LLM providers. A user who pastes a `.env` file, a kubeconfig, a customer record, or a stack trace with a bearer token will quietly send all of it to OpenAI / Anthropic / Gemini / Grok / Mistral / DeepSeek / Kimi / Z.ai. Tool results feed straight back into the conversation history without filtering — a `read_file` on `/etc/passwd`-equivalents (outside the blocked list), a `git diff` on a branch that accidentally committed a secret, a `fetch_url` on an HTML page that embeds a JWT in a data attribute all round-trip through the remote model and land in the session log under `~/.aictl/sessions/<uuid>`.

The roadmap entry calls for a **`Redactor` layer in `src/security.rs`** that catches sensitive data — personal info *and* credentials — on every outbound message and every tool result before it rejoins the history, with three modes (`off`, `redact`, `block`), a skip-by-default for local providers, and integration with the per-session audit log. This plan works out what that layer looks like, where it hooks in, how detection is implemented, and how the feature rolls out without breaking the existing flow.

## Goals & Non-goals

**Goals**
- Intercept every outbound user/assistant/tool-result payload before it reaches a remote provider and every tool result before it is appended to the conversation history.
- Detect two broad classes: **credentials** (API keys, tokens, private keys, JWTs, connection strings, cloud access keys) and **personal data** (emails, phone numbers, IBAN, credit card numbers, national IDs, dates of birth, street addresses, person names).
- Offer three modes — `off`, `redact`, `block` — configurable once in `~/.aictl/config` and overridable per-session via `/security`.
- Keep the `redact` placeholder scheme deterministic and typed (`[REDACTED:API_KEY]`, `[REDACTED:PHONE]`) so the LLM can still reason about the structure of the input.
- Skip redaction by default for local providers (Ollama, GGUF, MLX) since data never leaves the machine, with a toggle for users who still want it.
- Support user-defined patterns and per-workspace allowlists.
- Record every redaction / block event in the per-session audit log alongside tool calls.
- Zero behavioral change when the feature is `off` — that is the default for the first ship.

**Non-goals**
- No full DLP / enterprise policy engine (per-user rules, scoped ACLs, cloud policy sync). Out of scope for a single-binary CLI.
- No encryption-in-transit guarantees beyond what the provider SDKs already give. Redaction is about *what we send*, not *how*.
- No automatic exfiltration detection on outgoing tool calls (e.g. an LLM-directed `fetch_url` to a suspicious host with embedded data). That is a write-then-execute / egress-control problem — separate concern.
- No redaction of content rendered back to the terminal. The UI shows the raw conversation for the user's own eyes; redaction is a *network-boundary* control, not a screen privacy feature.
- No retroactive scrubbing of existing `~/.aictl/sessions/*` files. Sessions written before the feature shipped stay as-is.
- Not promising perfect recall for named entities. Regex + entropy + small on-device NER is a pragmatic floor, not a DLP guarantee — this must be documented clearly.

## How it fits with existing security controls

| Control | Direction | What it catches |
|---|---|---|
| `detect_prompt_injection` | inbound (user → LLM) | Instruction-override phrases, forged tags |
| `validate_tool` | outbound tool call | Shell / path / resource policy violations |
| `scrubbed_env` | outbound shell subprocess | Env vars with secret-shaped names |
| `sanitize_output` | inbound tool result | `<tool>` tag injection in tool output |
| **Redactor (new)** | outbound message payload + inbound tool result payload | Secret / PII *content* inside the text |

The redactor sits at a different seam than any existing control: it inspects **message bodies**, not tool calls or env vars. It runs **after** `detect_prompt_injection` (injection guard blocks instruction-override first; redaction runs on messages that survive), and **before** the provider's `call_X` is invoked. On the inbound path, it runs **after** `sanitize_output` so a secret embedded *inside* escaped `<tool>` text is still caught.

## Design

### 1. `Redactor` type and mode enum

Add a sibling of `SecurityPolicy` in `src/security.rs`:

```rust
pub enum RedactionMode { Off, Redact, Block }

pub struct RedactionPolicy {
    pub mode: RedactionMode,
    pub skip_local: bool,                        // Ollama / GGUF / MLX
    pub detectors: Vec<Detector>,                // regex packs + heuristics
    pub extra_patterns: Vec<(String, Regex)>,    // AICTL_REDACTION_EXTRA_PATTERNS
    pub allowlist: Vec<Regex>,                   // AICTL_REDACTION_ALLOW
    pub ner: Option<NerBackend>,                 // optional on-device NER
}
```

A `Detector` is `(kind: DetectorKind, matcher: DetectorMatcher)` where `DetectorKind` is the typed category (`ApiKey`, `AwsAccessKey`, `Jwt`, `PrivateKey`, `CreditCard`, `Iban`, `Phone`, `Email`, `PersonName`, `Address`, `DateOfBirth`, `Custom(String)`, …) and `DetectorMatcher` is either a compiled `Regex`, a regex-plus-validator pair (Luhn for cards, mod-97 for IBAN), a shape-based entropy scanner, or an NER callout. The `Custom` variant is how user-defined patterns surface in placeholders.

The policy loads once at startup next to `load_policy()`. It lives in its own `OnceLock<RedactionPolicy>` so disabling `AICTL_SECURITY_REDACTION` short-circuits without touching the main policy struct. `policy()` already exists — we expose `redaction_policy()` and `redact(text, ctx) -> RedactionResult` as the public API.

### 2. Detection strategy

Three layers, tried in order for each message:

**Layer A — high-precision regex packs.** One compiled `Regex` per kind, kept strict enough to avoid false positives:
- **API keys**: provider-shaped prefixes (`sk-…`, `sk-proj-…`, `sk-ant-…`, `AIza…`, `ghp_…`, `gho_…`, `xoxb-…`, `xoxp-…`, `hf_…`, `pk_live_…`, `rk_live_…`), length-constrained.
- **AWS**: `AKIA[0-9A-Z]{16}` / `ASIA[0-9A-Z]{16}` for access key IDs; secret access keys detected via entropy (Layer B).
- **GCP**: service-account JSON shape (`"type": "service_account"`), private-key block markers.
- **Azure**: `DefaultEndpointsProtocol=…;AccountKey=…` connection-string shape.
- **JWT**: `eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+` with a sanity-check on base64 decode of header.
- **Private keys**: PEM block markers `-----BEGIN (RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----` … `-----END … PRIVATE KEY-----`, multiline.
- **Credit cards**: digit groups → strip separators → Luhn check. Reject anything that looks like an order number / UUID / phone.
- **IBAN**: country-code + 2 check digits + BBAN + mod-97 == 1.
- **Email**: RFC-5322 practical subset. This one is noisy by nature; off by default for `redact` mode and configurable.
- **Phone**: E.164 plus common national formats, gated on context words ("phone", "tel", "mobile") to cut false positives on arbitrary digit runs.
- **Connection strings**: `postgres://user:pass@…`, `mongodb+srv://…`, `mysql://…`, `redis://:pass@…`, `amqp://user:pass@…`.

Each detector returns `Vec<Match { kind, range, confidence }>`. Ranges let us apply placeholders deterministically and keep byte offsets honest for the audit log.

**Layer B — entropy heuristics for opaque tokens.** Slide a 20–40 byte window over the text; report any contiguous base64/hex/alphanumeric run with Shannon entropy above a threshold (typical for 128-bit+ random strings). Useful for AWS secret keys, generic bearer tokens, and provider keys we don't have a specific pattern for. Bounded by min-length and character-class constraints to avoid flagging UUIDs, commit SHAs, and natural-language prose. Allowlist pass runs after this layer so a user can whitelist a known-good hash.

**Layer C — optional on-device NER for personal data.** The roadmap entry points at [gline-rs](https://github.com/fbilhaut/gline-rs) as a lightweight embedded NER. Gate this behind a new `redaction-ner` cargo feature (default off) so the base build stays small. When the feature is on *and* a model is available under `~/.aictl/models/ner/`, names / addresses / dates-of-birth are detected via NER. Missing model / feature off → personal-data detection falls back to Layer A regex heuristics only, with a clear log note. Under `mlx` or `gguf`, we could reuse the loaded backend in theory, but a separate NER model is simpler and doesn't borrow the chat model's memory.

All detectors run in a single pass over each message so we don't re-tokenize three times. Results are merged and overlapping matches are resolved by preferring the more specific kind (`AwsAccessKey` beats generic `HighEntropy`, `Jwt` beats `HighEntropy`).

### 3. `redact()` and `block()` modes

```rust
pub enum RedactionResult {
    Clean(String),                         // mode=off, no matches, or skip_local
    Redacted { text: String, matches: Vec<Match> },
    Blocked { matches: Vec<Match> },
}
```

- **Off**: pass-through — returns `Clean(text)`.
- **Redact**: replace each match range with `[REDACTED:<KIND>]`. Typed placeholders keep the LLM able to reason about structure ("The API key is [REDACTED:API_KEY]" is still understandable). Ranges are processed back-to-front to keep offsets valid. The original text is **never** sent.
- **Block**: on any match, abort the turn. `run_agent_turn` surfaces a clear error to the user naming each kind and a short context snippet (first 40 chars, ellipsised, with the match itself replaced by the placeholder so the error message itself stays clean).

A single `redact(text, &RedactionPolicy) -> RedactionResult` does all three — the mode lives on the policy, not the call site.

### 4. Integration points

The redactor runs at exactly two seams:

**Seam 1: outbound to a remote provider.** In `run_agent_turn` (`src/run.rs`), just above the provider dispatch (around the big `match provider {}` at line ~465). For each message in `llm_messages`, run `redact(msg.content, &pol)`:
- If `Clean` → pass through unchanged.
- If `Redacted` → substitute the message content with the redacted version **for this provider call only** (don't mutate the persisted history — see §6).
- If `Blocked` → abort the turn with an `Err` that lists the detected kinds; `run_agent_turn` already returns `Result<TurnResult, _>`, so this plugs in cleanly.

Skip the whole seam when `provider` is `Ollama | Gguf | Mlx` and `skip_local` is true.

**Seam 2: inbound tool result joining the history.** In `handle_tool_call` (`src/run.rs`), after the tool's `ToolOutput` has passed `sanitize_output` but before it is appended to `messages` as a tool-role turn. Same three-way switch. A `Blocked` result is surfaced to the user, recorded in the audit log, and the tool result is replaced with a short explanation that the tool result was blocked — the LLM still gets *something* to continue the turn but nothing sensitive.

The single-shot `--message` path flows through `run_agent_turn`, so covering those two seams covers every mode.

### 5. Local-provider bypass

The default is `skip_local=true`: data to Ollama / GGUF / MLX never leaves the machine, so redaction adds latency for no privacy gain. `AICTL_SECURITY_REDACTION_LOCAL=true` opts back in — useful for users who want to smoke-test their config against a local model before switching to a remote one, or who are running Ollama on a shared LAN host.

The check is a simple `matches!(provider, Provider::Ollama | Provider::Gguf | Provider::Mlx)` in `run_agent_turn`. The `Mock` provider (test-only) is treated as non-local so integration tests can exercise the redactor.

### 6. History and session persistence — keep the original

A hard design choice: **the user's own session file keeps the original text**. We redact *at the network boundary*, not in the stored history. Rationale:
- The user wrote the message or ran the tool; they already have the data locally.
- Session resumption (`--session <uuid>`) with redacted history would look wrong — the user would see `[REDACTED:API_KEY]` in their own messages on replay.
- Audit forensics need the original: "what did the model almost see?" is the question we're answering.

Implementation: the redactor produces a *transient* `Vec<Message>` for the provider call only; the persisted `messages: &mut Vec<Message>` is untouched. Memory-wise this doubles the message slice on each outbound call. For `LongTerm` memory that is potentially large — mitigated by only cloning when at least one match exists (common case: zero matches → zero extra allocations; we can `Cow<'_, str>` the message content to make this explicit).

When the user *does* want the stored session file scrubbed too, a follow-on `AICTL_SECURITY_REDACT_SESSIONS=true` config can be added — deferred to v2 to keep the first ship focused.

### 7. User-defined patterns and allowlists

Two config keys load on startup:

```
AICTL_REDACTION_EXTRA_PATTERNS=CUSTOMER_ID=CUST-\d{8};INTERNAL_TOKEN=tok_[A-Za-z0-9]{32}
AICTL_REDACTION_ALLOW=example\.com;AKIATEST0000000000
```

- Extra patterns are `NAME=REGEX` pairs, semicolon-separated. `NAME` becomes the placeholder suffix (`[REDACTED:CUSTOMER_ID]`).
- Allowlist entries are bare regexes; any match completely covered by an allowlist hit is dropped from the results before redaction or blocking.
- Bad regex at load time prints a single warning and is skipped — never aborts startup.

This mirrors the existing `AICTL_SECURITY_BLOCKED_PATHS` / `AICTL_SECURITY_ALLOWED_PATHS` ergonomics.

### 8. Audit log integration

`src/audit.rs` already writes JSONL per-session. Add a sibling `Outcome::RedactionEvent` or, less invasively, a new top-level logger `audit::log_redaction`:

```json
{
  "timestamp": "2026-04-19T10:22:15Z",
  "event": "redaction",
  "mode": "redact",
  "direction": "outbound",
  "source": "user_message",
  "matches": [
    {"kind": "API_KEY", "range": [143, 187], "snippet": "sk-proj-…REDACTED…", "confidence": "high"}
  ]
}
```

The snippet never contains the original secret — only the placeholder plus a few characters of surrounding context. Range is byte offsets in the outbound message. `direction` is `outbound` (message → provider) or `inbound` (tool result → history). `source` is one of `user_message`, `assistant_message`, `tool_result`, `system_prompt`. Block events reuse the same shape with `mode=block`.

Audit writes respect the existing `enabled()` check and the incognito gate — the redactor itself runs even in incognito (it's a privacy control, not an audit one), but the log line is skipped, consistent with the rest of `audit.rs`.

### 9. Configuration summary (in `~/.aictl/config`)

```
AICTL_SECURITY_REDACTION=off          # off | redact | block (default: off — opt-in for v1)
AICTL_SECURITY_REDACTION_LOCAL=false  # also redact outbound to Ollama/GGUF/MLX (default: false)
AICTL_REDACTION_EXTRA_PATTERNS=       # semicolon-separated NAME=REGEX pairs
AICTL_REDACTION_ALLOW=                # semicolon-separated regexes covering known-good matches
AICTL_REDACTION_DETECTORS=            # comma-separated subset — empty = all (api_key, aws, gcp, azure, jwt, private_key, credit_card, iban, email, phone, connection_string, person_name, address)
AICTL_REDACTION_NER=false             # enable NER pass when the `redaction-ner` cargo feature is built in
```

Default is `off` for v1 — redaction is a privacy *feature*, but a silent behavior change at the network boundary. Users opt in deliberately. `/security` prints the current mode; `/config` wizard grows a redaction page.

### 10. Cargo feature gating

Two features:
- `redaction` (default **on**) — regex packs, Luhn/IBAN validators, entropy scanner. No heavy deps; `regex` is already pulled in indirectly via `clap`. `sha2` already ships. So this is effectively free code, always compiled.
- `redaction-ner` (default **off**) — pulls in `gline-rs` and a tokenizer. Gates Layer C. Install instructions are the same shape as `mlx` / `gguf`: document the feature in the README, ship it in release binaries only where it makes sense, fail gracefully with a clear error if the user sets `AICTL_REDACTION_NER=true` in a build without the feature.

The release workflow (`.github/workflows/release.yml`) grows `--features redaction-ner` on the runner targets where the NER crate builds cleanly. Source builds stay lightweight.

### 11. Integration points summary

| File | Change |
|------|--------|
| `src/security.rs` | Add `RedactionPolicy`, `RedactionMode`, `Detector`, `Match`, `RedactionResult`, `redact()`, `redaction_policy()`, `load_redaction_policy()`, detector implementations |
| `src/security/redaction/` | **New** submodule tree — one file per detector family (`credentials.rs`, `cards_and_ibans.rs`, `contacts.rs`, `entropy.rs`, `ner.rs`), plus `detectors.rs` for the `Detector` type and registry |
| `src/run.rs` | Wire Seam 1 (before provider dispatch) and Seam 2 (in `handle_tool_call` after `sanitize_output`) |
| `src/audit.rs` | Add `log_redaction(direction, source, mode, matches)` |
| `src/commands/security.rs` | Show redaction mode + active detectors in the `/security` output |
| `src/commands/config_wizard.rs` | Add a redaction section — mode, local toggle, NER toggle |
| `src/config.rs` | Constants for default mode, default detector list |
| `Cargo.toml` | `redaction` feature (on by default), `redaction-ner` feature (off) |
| `CLAUDE.md` | Document the feature alongside the existing security section |
| `README.md` | Short section under Security — what is redacted, modes, how to allowlist |
| `.github/workflows/release.yml` | Add `redaction-ner` on targets that support it |

### 12. Phased rollout

1. **Phase 1 — scaffolding + regex detectors.** `RedactionPolicy`, mode enum, `redact()`, Layer A detectors (credentials + structured PII only — no names/addresses), config keys, audit logging, `/security` display. Ship with default `off`. No behavioral change for any user who doesn't opt in. Unit tests for every detector with positive and negative cases.
2. **Phase 2 — entropy heuristics + allowlists.** Layer B scanner, user-defined patterns, allowlist. At this point `redact` mode is genuinely useful for the "don't leak API keys" use case.
3. **Phase 3 — NER (optional feature).** Layer C behind `redaction-ner`. Ship the feature in release binaries where the build is clean. Document known false-positive/negative shape.
4. **Phase 4 — flip default to `redact` for remote providers** — only after Phase 1–3 have been in the wild long enough to gauge false-positive rates. This is a user-facing behavior change that deserves a version bump and a release note.
5. **Phase 5 (follow-on, not in this plan)** — on-disk session scrubbing (`AICTL_SECURITY_REDACT_SESSIONS`), exfiltration-direction detection on outbound `fetch_url` bodies, per-workspace policy files (`.aictl-redaction.toml`).

### 13. Testing

- **Unit tests, detector-level**: each detector has a table-driven test with 8–15 positive examples (real-shaped but fake values) and 4–8 negative examples (UUIDs, commit SHAs, prose that *looks* like a secret). Luhn and IBAN validators get their own direct tests. Entropy scanner gets a test that high-entropy hex of length ≥ 32 is flagged and `4f9c1a7b…` of length 8 is not.
- **Integration tests**: extend `src/integration_tests.rs` to exercise the `Mock` provider with redaction on. Verify (a) outbound messages have placeholders, (b) persisted history does not, (c) block mode surfaces the error, (d) local-provider skip is honored, (e) audit log contains the expected entries.
- **False-positive corpus**: keep a fixture file of real-world prose (commit messages, stack traces, code snippets) in `tests/fixtures/redaction/clean.txt` and assert `redact()` on each paragraph reports zero matches. This is the long-term regression guard against over-eager detectors.
- **Manual verification**: run `aictl --message` with a prompt embedding a fake `sk-proj-…` key against the Mock provider with `AICTL_SECURITY_REDACTION=redact`; verify the outbound message in the audit log shows the placeholder; verify the session file shows the original.

### 14. Known limitations (to be documented)

- Regex-based detection is fundamentally a floor, not a ceiling. Novel secret shapes, rotated key prefixes, or provider-specific formats we haven't patterned will slip through until added. The entropy scanner catches generic high-entropy strings but can't distinguish a customer token from a commit hash.
- NER for person names is locale- and model-dependent. A small embedded model will miss non-Latin names, honorifics, and contextual references. Users with strict PII requirements should lean on custom patterns for their known data shapes rather than trust the NER.
- Placeholder substitution changes the byte length of the message. Providers that meter on input tokens will show a small reduction; that's a feature, not a bug, but worth calling out so users don't chase phantom token-count regressions.
- Redaction runs on *string* content; base64-encoded images attached via `ImageData` are not scanned. An image of a whiteboard full of keys will still reach the provider. Same for audio/video when/if those arrive.

## Verification

1. `cargo build` — builds with and without `redaction-ner`.
2. `cargo clippy` — no new warnings.
3. `cargo test` — all detector unit tests pass, integration tests cover both seams, false-positive corpus is clean.
4. Manual runs against the Mock provider with each mode (`off`, `redact`, `block`) and against at least one remote provider and one local provider to confirm the skip-local logic.
5. `~/.aictl/audit/<session>` contains `redaction` events with the expected shape and no leaked secret material.

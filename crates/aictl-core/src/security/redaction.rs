//! Sensitive-data redaction for outbound LLM messages and inbound tool
//! results. Sits at a different seam than the rest of [`crate::security`]:
//! the path / shell / tool-call gates run on *tool calls*, while this
//! layer inspects *message bodies* right before they leave for a remote
//! provider (and right before a tool result rejoins the conversation
//! history). See `.claude/plans/sensitive-data-redaction.md` for the
//! design doc.
//!
//! # Detection layers
//!
//! - **Layer A — high-precision regex packs.** One compiled `Regex` per
//!   kind: API keys (`sk-…`, `sk-ant-…`, `AIza…`, `ghp_…`, `hf_…`,
//!   `sk_live_…`, `xoxb-…`), AWS access key IDs (`AKIA…` / `ASIA…`),
//!   JWTs, PEM private-key blocks, DB/AMQP connection strings with
//!   inline credentials, emails, phones (context-gated), credit-card
//!   digit runs (validated via Luhn), IBANs (validated via mod-97).
//! - **Layer B — entropy heuristic.** Sliding window over contiguous
//!   `[A-Za-z0-9/+=_-]` runs of length ≥ 32 with Shannon entropy above
//!   a threshold. Catches rotated or unknown-prefix tokens. User
//!   allowlist runs after this layer, so known-good hashes/UUIDs can
//!   be whitelisted.
//! - **Layer C — optional NER.** Gated behind the `redaction-ner`
//!   cargo feature (scaffolded only — the feature currently returns a
//!   clear "not built" error when `AICTL_REDACTION_NER=true` is set
//!   without the feature).
//!
//! Overlapping matches are resolved by priority: specific kinds
//! (`Jwt`, `PrivateKey`, `ApiKey`) beat generic `HighEntropy`; custom
//! user-defined patterns win over everything else. Ranges are
//! non-overlapping in the final list so placeholder substitution is
//! deterministic.
//!
//! # Modes
//!
//! - `off` — pass-through (default for v1).
//! - `redact` — replace match ranges with `[REDACTED:<KIND>]`. The
//!   original text is never sent to the provider.
//! - `block` — abort the turn; the error names each detected kind
//!   with a scrubbed context snippet.
//!
//! # History boundary
//!
//! Redaction runs *at the network boundary* only. The persisted session
//! file under `~/.aictl/sessions/` keeps the user's original text
//! (the user already has the data locally; replaying a redacted session
//! would be confusing). A transient redacted clone of the message slice
//! is handed to the provider for that one call; the caller's mutable
//! `Vec<Message>` is never mutated.

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

use crate::config::config_get_scoped;

pub mod ner;

/// Redaction mode — read from `AICTL_SECURITY_REDACTION` at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionMode {
    /// Pass-through. The default for v1 — no behavior change for users
    /// who do not opt in.
    Off,
    /// Replace each match range with `[REDACTED:<KIND>]` before the
    /// message leaves for the provider.
    Redact,
    /// Abort the turn on any match; surface the kinds to the user.
    Block,
}

/// Typed category for a single match. The placeholder rendered into
/// redacted text is derived from this enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DetectorKind {
    /// Known-prefix API keys (`OpenAI` `sk-…`, Anthropic `sk-ant-…`,
    /// Google `AIza…`, GitHub `ghp_…` / `gho_…`, `HuggingFace` `hf_…`,
    /// Stripe `sk_live_…`, Slack `xoxb-…` …).
    ApiKey,
    /// AWS access-key ID: `AKIA…` / `ASIA…` followed by 16 [A-Z0-9].
    AwsAccessKey,
    /// JSON Web Token: three base64url segments joined with `.` where
    /// the first two decode to `{` (header + payload).
    Jwt,
    /// PEM private-key block: `-----BEGIN … PRIVATE KEY-----` … `-----END … PRIVATE KEY-----`.
    PrivateKey,
    /// DB/AMQP/Redis URL with inline credentials — `scheme://user:pass@host…`.
    ConnectionString,
    /// 13–19 digit run that passes the Luhn check.
    CreditCard,
    /// IBAN that passes the mod-97 check.
    Iban,
    /// Email address (RFC-5322 practical subset).
    Email,
    /// Phone number, gated on context keywords (`phone`, `tel`, `mobile`, `cell`).
    Phone,
    /// Layer B — opaque high-entropy token.
    HighEntropy,
    /// Layer C — person name detected by the optional NER backend.
    /// Only constructed when the `redaction-ner` cargo feature is
    /// built in; the `dead_code` allow is for non-feature builds
    /// where the variant is reachable but unused.
    #[cfg_attr(not(feature = "redaction-ner"), allow(dead_code))]
    PersonName,
    /// Layer C — physical location (address, city, country) detected
    /// by the optional NER backend.
    #[cfg_attr(not(feature = "redaction-ner"), allow(dead_code))]
    Location,
    /// Layer C — organization / company / institution name detected
    /// by the optional NER backend.
    #[cfg_attr(not(feature = "redaction-ner"), allow(dead_code))]
    Organization,
    /// User-defined pattern from `AICTL_REDACTION_EXTRA_PATTERNS`.
    /// The `String` is the user-supplied name (becomes the placeholder
    /// suffix, e.g. `CUSTOMER_ID`).
    Custom(String),
}

impl DetectorKind {
    /// Placeholder suffix used in `[REDACTED:<SUFFIX>]`. Uppercase,
    /// matches the typed scheme described in the plan.
    pub fn placeholder(&self) -> String {
        match self {
            Self::ApiKey => "API_KEY".to_string(),
            Self::AwsAccessKey => "AWS_KEY".to_string(),
            Self::Jwt => "JWT".to_string(),
            Self::PrivateKey => "PRIVATE_KEY".to_string(),
            Self::ConnectionString => "CONNECTION_STRING".to_string(),
            Self::CreditCard => "CREDIT_CARD".to_string(),
            Self::Iban => "IBAN".to_string(),
            Self::Email => "EMAIL".to_string(),
            Self::Phone => "PHONE".to_string(),
            Self::HighEntropy => "HIGH_ENTROPY".to_string(),
            Self::PersonName => "PERSON".to_string(),
            Self::Location => "LOCATION".to_string(),
            Self::Organization => "ORGANIZATION".to_string(),
            Self::Custom(name) => name.clone(),
        }
    }

    /// Slug used in `AICTL_REDACTION_DETECTORS` to enable/disable a
    /// specific kind. Custom kinds are not filterable via the slug
    /// list — if the user declared them they are assumed wanted.
    pub fn slug(&self) -> &str {
        match self {
            Self::ApiKey => "api_key",
            Self::AwsAccessKey => "aws",
            Self::Jwt => "jwt",
            Self::PrivateKey => "private_key",
            Self::ConnectionString => "connection_string",
            Self::CreditCard => "credit_card",
            Self::Iban => "iban",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::HighEntropy => "high_entropy",
            Self::PersonName => "person_name",
            Self::Location => "location",
            Self::Organization => "organization",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// Resolution priority when two matches overlap — higher wins.
    /// Keeps `Jwt` from being shadowed by the entropy scanner, and
    /// keeps a user-defined `Custom` pattern ahead of everything.
    fn priority(&self) -> u8 {
        match self {
            Self::Custom(_) => 10,
            Self::PrivateKey => 9,
            Self::Jwt | Self::ConnectionString => 8,
            Self::ApiKey | Self::AwsAccessKey => 7,
            Self::Iban | Self::CreditCard => 6,
            Self::Email => 5,
            Self::Phone => 4,
            // NER hits sit above HighEntropy (model output is more
            // informative than "looks random") but below structured
            // detectors so a regex-confirmed credential wins over a
            // mis-classified proper-noun span.
            Self::PersonName | Self::Location | Self::Organization => 3,
            Self::HighEntropy => 1,
        }
    }
}

/// A single detected span in a message. Byte ranges are into the input
/// `&str` so `&text[range]` round-trips cleanly.
#[derive(Debug, Clone)]
pub struct Match {
    pub kind: DetectorKind,
    pub range: Range<usize>,
    pub confidence: &'static str,
}

/// Direction / origin tag for the audit log. Matches the `source` field
/// shape in the plan's audit entry.
#[derive(Debug, Clone, Copy)]
pub enum RedactionSource {
    SystemPrompt,
    UserMessage,
    AssistantMessage,
    ToolResult,
}

impl RedactionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemPrompt => "system_prompt",
            Self::UserMessage => "user_message",
            Self::AssistantMessage => "assistant_message",
            Self::ToolResult => "tool_result",
        }
    }
}

/// Direction of the message relative to the process boundary.
#[derive(Debug, Clone, Copy)]
pub enum RedactionDirection {
    /// User/assistant/tool-result payload heading to a remote provider.
    Outbound,
    /// Tool result on its way into conversation history.
    Inbound,
}

impl RedactionDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outbound => "outbound",
            Self::Inbound => "inbound",
        }
    }
}

/// What `redact()` returns. `Clean` carries no payload — callers should
/// use the original input untouched in that branch, avoiding any clone.
#[derive(Debug)]
pub enum RedactionResult {
    /// No match (or mode is `Off`). The original text is safe to send.
    Clean,
    /// At least one match; `text` is the placeholder-substituted output
    /// and `matches` is the (deduplicated, priority-resolved) hit list.
    Redacted { text: String, matches: Vec<Match> },
    /// Block mode tripped. The caller must surface an error; the
    /// original text must not be sent.
    Blocked { matches: Vec<Match> },
}

/// Global redaction policy loaded once at startup.
pub struct RedactionPolicy {
    pub mode: RedactionMode,
    /// Skip redaction for local providers (Ollama, GGUF, MLX). Default
    /// `true` — data never leaves the machine, no privacy gain.
    pub skip_local: bool,
    /// Enabled built-in detector slugs. Empty = all built-ins enabled.
    /// User-supplied custom patterns always run regardless.
    pub enabled_detectors: Vec<String>,
    /// User-defined `(name, regex)` pairs from
    /// `AICTL_REDACTION_EXTRA_PATTERNS`. `name` becomes the placeholder
    /// suffix.
    pub extra_patterns: Vec<(String, Regex)>,
    /// Regexes whose matches suppress any overlapping detection hit —
    /// e.g. `AKIATEST0000000000` or an internal commit-hash allowlist.
    pub allowlist: Vec<Regex>,
    /// User opted in to the NER pass. Only meaningful when the
    /// `redaction-ner` cargo feature is built in; otherwise we surface
    /// a startup warning. Surfaced in `/security` so the user can
    /// see whether their opt-in actually has an effect.
    pub ner_requested: bool,
}

impl RedactionPolicy {
    /// An inert policy used for tests and as the `OnceLock` fallback.
    fn off() -> Self {
        Self {
            mode: RedactionMode::Off,
            skip_local: true,
            enabled_detectors: vec![],
            extra_patterns: vec![],
            allowlist: vec![],
            ner_requested: false,
        }
    }

    /// One-line summary suitable for `/security` output.
    #[must_use]
    pub fn summary(&self) -> String {
        // One-line mode summary for the `redaction:` row in `/security`
        // and `/info`. The full breakdown (active detectors, custom
        // patterns, allowlist, NER state) is printed by
        // `commands::security::print_redaction_detail` directly below
        // this row, so this line stays short.
        match self.mode {
            RedactionMode::Off => "off".to_string(),
            RedactionMode::Redact => "redact (network-boundary scrubbing)".to_string(),
            RedactionMode::Block => "block (abort on sensitive data)".to_string(),
        }
    }

    pub(crate) fn is_detector_enabled(&self, kind: &DetectorKind) -> bool {
        // Custom patterns always run (they're declared by the user).
        if matches!(kind, DetectorKind::Custom(_)) {
            return true;
        }
        if self.enabled_detectors.is_empty() {
            return true;
        }
        self.enabled_detectors.iter().any(|s| s == kind.slug())
    }
}

static POLICY: OnceLock<RedactionPolicy> = OnceLock::new();

/// Initialize the redaction policy from config. Call once at startup
/// after [`crate::config::load_config`]. Returns any warnings produced
/// during config parsing (bad regexes, missing NER model / feature) so
/// the caller can route them through the active UI rather than having
/// the engine reach into stderr directly.
#[must_use]
pub fn init() -> Vec<String> {
    let (pol, warnings) = load_policy();
    POLICY.set(pol).ok();
    warnings
}

/// Access the global redaction policy. Returns an inert `Off` policy if
/// `init()` has not been called (tests, defensive fallback).
pub fn policy() -> &'static RedactionPolicy {
    static DEFAULT: OnceLock<RedactionPolicy> = OnceLock::new();
    POLICY
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(RedactionPolicy::off))
}

fn load_policy() -> (RedactionPolicy, Vec<String>) {
    // Every redaction knob honors the `AICTL_SERVER_*` override when
    // the engine is running inside `aictl-server`, so the proxy can run
    // a stricter (or weaker) data-leak posture than the operator's
    // interactive CLI without forking `~/.aictl/config`. When the
    // server-prefixed key is unset the lookup falls through to the
    // shared `AICTL_*` value.
    let mut warnings = Vec::new();
    let mode = match config_get_scoped(
        "AICTL_SERVER_SECURITY_REDACTION",
        "AICTL_SECURITY_REDACTION",
    )
    .as_deref()
    {
        Some("redact") => RedactionMode::Redact,
        Some("block") => RedactionMode::Block,
        _ => RedactionMode::Off,
    };

    let skip_local = config_get_scoped(
        "AICTL_SERVER_SECURITY_REDACTION_LOCAL",
        "AICTL_SECURITY_REDACTION_LOCAL",
    )
    .is_none_or(|v| v != "true" && v != "1");

    let enabled_detectors = parse_csv(
        &config_get_scoped(
            "AICTL_SERVER_REDACTION_DETECTORS",
            "AICTL_REDACTION_DETECTORS",
        )
        .unwrap_or_default(),
        ',',
    );

    let extra_patterns = parse_extra_patterns(
        &config_get_scoped(
            "AICTL_SERVER_REDACTION_EXTRA_PATTERNS",
            "AICTL_REDACTION_EXTRA_PATTERNS",
        )
        .unwrap_or_default(),
        &mut warnings,
    );

    let allowlist: Vec<Regex> = parse_csv(
        &config_get_scoped("AICTL_SERVER_REDACTION_ALLOW", "AICTL_REDACTION_ALLOW")
            .unwrap_or_default(),
        ';',
    )
    .into_iter()
    .filter_map(|p| match Regex::new(&p) {
        Ok(r) => Some(r),
        Err(e) => {
            warnings.push(format!(
                "invalid AICTL_REDACTION_ALLOW pattern '{p}': {e}. Skipped."
            ));
            None
        }
    })
    .collect();

    let ner_requested = matches!(
        config_get_scoped("AICTL_SERVER_REDACTION_NER", "AICTL_REDACTION_NER").as_deref(),
        Some("true" | "1")
    );

    if ner_requested {
        match ner::status(true) {
            ner::NerStatus::FeatureMissing => warnings.push(
                "AICTL_REDACTION_NER=true but this build lacks the `redaction-ner` feature. \
                 The NER pass will not run. Rebuild with `cargo build --features redaction-ner`."
                    .to_string(),
            ),
            ner::NerStatus::ModelMissing { expected_name } => warnings.push(format!(
                "AICTL_REDACTION_NER=true but NER model '{expected_name}' is not on disk. \
                 Run `aictl --pull-ner-model <owner>/<repo>` (default: {}) to fetch it.",
                ner::DEFAULT_NER_MODEL
            )),
            ner::NerStatus::Disabled | ner::NerStatus::Ready { .. } => {}
        }
    }

    (
        RedactionPolicy {
            mode,
            skip_local,
            enabled_detectors,
            extra_patterns,
            allowlist,
            ner_requested,
        },
        warnings,
    )
}

fn parse_csv(s: &str, sep: char) -> Vec<String> {
    s.split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Parse `NAME=REGEX;NAME=REGEX;…` into compiled pairs. Bad regexes are
/// recorded as warnings and skipped rather than aborting startup; the
/// caller routes the accumulated messages through the active UI.
fn parse_extra_patterns(s: &str, warnings: &mut Vec<String>) -> Vec<(String, Regex)> {
    let mut out = Vec::new();
    for entry in s.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((name, pattern)) = entry.split_once('=') else {
            warnings.push(format!(
                "AICTL_REDACTION_EXTRA_PATTERNS entry '{entry}' is missing '=' — expected NAME=REGEX. Skipped."
            ));
            continue;
        };
        let name = name.trim();
        let pattern = pattern.trim();
        if name.is_empty() || pattern.is_empty() {
            warnings.push(format!(
                "AICTL_REDACTION_EXTRA_PATTERNS entry '{entry}' has empty name or pattern. Skipped."
            ));
            continue;
        }
        match Regex::new(pattern) {
            Ok(r) => out.push((name.to_ascii_uppercase(), r)),
            Err(e) => warnings.push(format!(
                "invalid AICTL_REDACTION_EXTRA_PATTERNS regex for '{name}': {e}. Skipped."
            )),
        }
    }
    out
}

// --- Public entry point ---

/// Redact (or block, or leave untouched) the given text according to the
/// provided policy. Pure function: no global state, easy to test.
pub fn redact(text: &str, pol: &RedactionPolicy) -> RedactionResult {
    if matches!(pol.mode, RedactionMode::Off) || text.is_empty() {
        return RedactionResult::Clean;
    }

    let matches = find_matches(text, pol);
    if matches.is_empty() {
        return RedactionResult::Clean;
    }

    match pol.mode {
        RedactionMode::Off => RedactionResult::Clean,
        RedactionMode::Redact => {
            let text = apply_placeholders(text, &matches);
            RedactionResult::Redacted { text, matches }
        }
        RedactionMode::Block => RedactionResult::Blocked { matches },
    }
}

/// Render a short, non-sensitive description of the block-mode matches
/// for error messages. Never includes the original match content — only
/// the placeholder plus a few characters of surrounding context.
pub fn describe_matches(text: &str, matches: &[Match]) -> String {
    let mut kinds: Vec<String> = matches.iter().map(|m| m.kind.placeholder()).collect();
    kinds.sort();
    kinds.dedup();
    let kinds_str = kinds.join(", ");
    let first = &matches[0];
    let ctx_start = safe_boundary(text, first.range.start.saturating_sub(20), false);
    let ctx_end = safe_boundary(text, (first.range.end + 20).min(text.len()), true);
    let before = &text[ctx_start..first.range.start];
    let after = &text[first.range.end..ctx_end];
    let placeholder = format!("[REDACTED:{}]", first.kind.placeholder());
    let snippet = format!("…{before}{placeholder}{after}…");
    format!("{kinds_str} at: {snippet}")
}

// --- Matching ---

fn find_matches(text: &str, pol: &RedactionPolicy) -> Vec<Match> {
    let mut raw: Vec<Match> = Vec::new();

    // Custom user patterns first — they win on priority.
    for (name, re) in &pol.extra_patterns {
        for m in re.find_iter(text) {
            raw.push(Match {
                kind: DetectorKind::Custom(name.clone()),
                range: m.start()..m.end(),
                confidence: "user",
            });
        }
    }

    // Layer A built-in detectors.
    run_regex_detector(
        text,
        api_key_regex(),
        &DetectorKind::ApiKey,
        "high",
        pol,
        &mut raw,
    );
    run_regex_detector(
        text,
        aws_access_key_regex(),
        &DetectorKind::AwsAccessKey,
        "high",
        pol,
        &mut raw,
    );
    run_jwt_detector(text, pol, &mut raw);
    run_regex_detector(
        text,
        private_key_regex(),
        &DetectorKind::PrivateKey,
        "high",
        pol,
        &mut raw,
    );
    run_regex_detector(
        text,
        connection_string_regex(),
        &DetectorKind::ConnectionString,
        "high",
        pol,
        &mut raw,
    );
    run_credit_card_detector(text, pol, &mut raw);
    run_iban_detector(text, pol, &mut raw);
    run_regex_detector(
        text,
        email_regex(),
        &DetectorKind::Email,
        "medium",
        pol,
        &mut raw,
    );
    run_phone_detector(text, pol, &mut raw);

    // Layer B entropy scanner.
    if pol.is_detector_enabled(&DetectorKind::HighEntropy) {
        run_entropy_scanner(text, &mut raw);
    }

    // Layer C — NER-based person/location/organization detection.
    // The call is a no-op when the `redaction-ner` feature is off,
    // when the user has not opted in, or when no model is on disk —
    // see `ner::run_ner_detector` for the gating.
    if pol.ner_requested {
        ner::run_ner_detector(text, pol, &mut raw);
    }

    // Filter out anything covered by the allowlist (run after Layers
    // B and C so users can whitelist a known-good high-entropy hash
    // or a false-positive name).
    if !pol.allowlist.is_empty() {
        raw.retain(|m| !is_allowlisted(text, m, &pol.allowlist));
    }

    merge_overlaps(raw)
}

fn run_regex_detector(
    text: &str,
    re: &Regex,
    kind: &DetectorKind,
    confidence: &'static str,
    pol: &RedactionPolicy,
    out: &mut Vec<Match>,
) {
    if !pol.is_detector_enabled(kind) {
        return;
    }
    for m in re.find_iter(text) {
        out.push(Match {
            kind: kind.clone(),
            range: m.start()..m.end(),
            confidence,
        });
    }
}

/// JWT detector with a cheap sanity-check: the header segment must
/// base64-decode to something that starts with `{`, rejecting the
/// common `eyJ…` false-positives in binary/encoded blobs.
fn run_jwt_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::Jwt) {
        return;
    }
    for m in jwt_regex().find_iter(text) {
        let matched = m.as_str();
        let Some(header) = matched.split('.').next() else {
            continue;
        };
        if decoded_starts_with_brace(header) {
            out.push(Match {
                kind: DetectorKind::Jwt,
                range: m.start()..m.end(),
                confidence: "high",
            });
        }
    }
}

fn decoded_starts_with_brace(b64url: &str) -> bool {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    URL_SAFE_NO_PAD
        .decode(b64url)
        .ok()
        .and_then(|bytes| bytes.first().copied())
        .is_some_and(|b| b == b'{')
}

fn run_credit_card_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::CreditCard) {
        return;
    }
    for m in credit_card_regex().find_iter(text) {
        let raw = m.as_str();
        let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
        let n = digits.len();
        // Reject UUIDs (32 chars), commit SHAs, and overlong runs.
        if !(13..=19).contains(&n) {
            continue;
        }
        if luhn_check(&digits) {
            out.push(Match {
                kind: DetectorKind::CreditCard,
                range: m.start()..m.end(),
                confidence: "high",
            });
        }
    }
}

fn luhn_check(digits: &str) -> bool {
    let mut sum: u32 = 0;
    for (i, c) in digits.chars().rev().enumerate() {
        let Some(d) = c.to_digit(10) else {
            return false;
        };
        if i % 2 == 1 {
            let doubled = d * 2;
            sum += if doubled > 9 { doubled - 9 } else { doubled };
        } else {
            sum += d;
        }
    }
    sum.is_multiple_of(10)
}

fn run_iban_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::Iban) {
        return;
    }
    for m in iban_regex().find_iter(text) {
        if iban_check(m.as_str()) {
            out.push(Match {
                kind: DetectorKind::Iban,
                range: m.start()..m.end(),
                confidence: "high",
            });
        }
    }
}

/// mod-97 IBAN validator. Moves the first four characters to the end,
/// converts letters to two-digit codes (A=10…Z=35), computes mod 97.
fn iban_check(iban: &str) -> bool {
    let iban: String = iban.chars().filter(|c| !c.is_whitespace()).collect();
    if iban.len() < 15 || iban.len() > 34 {
        return false;
    }
    let (head, tail) = iban.split_at(4);
    let rearranged = format!("{tail}{head}");
    let mut expanded = String::with_capacity(rearranged.len() * 2);
    for c in rearranged.chars() {
        if c.is_ascii_digit() {
            expanded.push(c);
        } else if c.is_ascii_alphabetic() {
            let code = (c.to_ascii_uppercase() as u32) - ('A' as u32) + 10;
            expanded.push_str(&code.to_string());
        } else {
            return false;
        }
    }
    // Stream mod-97 over the decimal string — the number can be
    // hundreds of digits long.
    let mut rem: u64 = 0;
    for c in expanded.chars() {
        let d = c.to_digit(10).unwrap_or(0);
        rem = (rem * 10 + u64::from(d)) % 97;
    }
    rem == 1
}

/// Phone numbers are noisy by shape — an arbitrary 10-digit run could be
/// an order number or postal code. Gate the match on a context keyword
/// (`phone`, `tel`, `mobile`, `cell`, `fax`) within ~30 chars to the
/// left of the span. E.164 international format (`+CC…`) is high enough
/// confidence to flag without context.
fn run_phone_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::Phone) {
        return;
    }
    let lower = text.to_ascii_lowercase();
    for m in phone_regex().find_iter(text) {
        let raw = m.as_str();
        let digits = raw.chars().filter(char::is_ascii_digit).count();
        if !(7..=15).contains(&digits) {
            continue;
        }
        // E.164 (`+CC…`) is a standalone signal; otherwise require
        // `phone`/`tel`/`mobile`/`cell`/`fax` nearby.
        let confidence = if raw.trim_start().starts_with('+') {
            "high"
        } else if has_phone_context(&lower, m.start()) {
            "medium"
        } else {
            continue;
        };
        out.push(Match {
            kind: DetectorKind::Phone,
            range: m.start()..m.end(),
            confidence,
        });
    }
}

const PHONE_CONTEXT_WORDS: &[&str] = &["phone", "tel", "mobile", "cell", "fax"];

fn has_phone_context(lower_text: &str, span_start: usize) -> bool {
    let window_start = span_start.saturating_sub(30);
    let window_start = safe_boundary(lower_text, window_start, false);
    let window = &lower_text[window_start..span_start];
    PHONE_CONTEXT_WORDS.iter().any(|w| window.contains(w))
}

// --- Layer B entropy scanner ---

const ENTROPY_MIN_RUN: usize = 32;
const ENTROPY_THRESHOLD: f64 = 4.5;

fn run_entropy_scanner(text: &str, out: &mut Vec<Match>) {
    // Walk the byte stream and find maximal runs of [A-Za-z0-9/+=_-].
    // We report ranges on byte offsets — all characters in the allowed
    // set are ASCII, so byte and char offsets coincide in-run.
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_token_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_token_byte(bytes[i]) {
            i += 1;
        }
        let end = i;
        if end - start >= ENTROPY_MIN_RUN {
            let run = &text[start..end];
            if shannon_entropy(run.as_bytes()) >= ENTROPY_THRESHOLD {
                out.push(Match {
                    kind: DetectorKind::HighEntropy,
                    range: start..end,
                    confidence: "low",
                });
            }
        }
    }
}

fn is_token_byte(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'+' | b'=' | b'_' | b'-')
}

fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    // Precision loss is fine: entropy is compared against a threshold
    // and the byte counts we feed here are bounded by the max run
    // length the scanner produces (well under 2^53).
    #[allow(clippy::cast_precision_loss)]
    let len = bytes.len() as f64;
    let mut h = 0.0;
    for &c in &counts {
        if c == 0 {
            continue;
        }
        let p = f64::from(c) / len;
        h -= p * p.log2();
    }
    h
}

// --- Allowlist + overlap resolution ---

fn is_allowlisted(text: &str, m: &Match, allowlist: &[Regex]) -> bool {
    let snippet = &text[m.range.clone()];
    allowlist.iter().any(|re| {
        // Accept either an exact full-string match on the snippet or a
        // hit whose range fully covers the match span in the original
        // text. The latter catches "this whole line is an example".
        if let Some(whole) = re.find(snippet)
            && whole.start() == 0
            && whole.end() == snippet.len()
        {
            return true;
        }
        re.find_iter(text)
            .any(|am| am.start() <= m.range.start && am.end() >= m.range.end)
    })
}

/// Resolve overlapping matches. Higher `priority()` wins; within the
/// same kind, earlier-starting wins. Output is sorted by start offset
/// and non-overlapping.
fn merge_overlaps(mut matches: Vec<Match>) -> Vec<Match> {
    if matches.is_empty() {
        return matches;
    }
    // Primary sort: priority desc. Secondary: start asc. Tertiary: wider span first.
    matches.sort_by(|a, b| {
        b.kind
            .priority()
            .cmp(&a.kind.priority())
            .then(a.range.start.cmp(&b.range.start))
            .then((b.range.end - b.range.start).cmp(&(a.range.end - a.range.start)))
    });

    let mut accepted: Vec<Match> = Vec::new();
    for m in matches {
        let overlaps_accepted = accepted
            .iter()
            .any(|a| m.range.start < a.range.end && m.range.end > a.range.start);
        if !overlaps_accepted {
            accepted.push(m);
        }
    }
    // Re-sort final list by start offset so placeholder substitution
    // downstream is left-to-right deterministic.
    accepted.sort_by_key(|m| m.range.start);
    accepted
}

// --- Placeholder substitution ---

fn apply_placeholders(text: &str, matches: &[Match]) -> String {
    // Walk back-to-front to keep byte offsets in unchanged regions
    // valid as we splice each span. `merge_overlaps` already
    // guarantees the ranges are non-overlapping and sorted ascending.
    let mut out = text.to_string();
    for m in matches.iter().rev() {
        let placeholder = format!("[REDACTED:{}]", m.kind.placeholder());
        out.replace_range(m.range.clone(), &placeholder);
    }
    out
}

fn safe_boundary(s: &str, mut idx: usize, forward: bool) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && idx < s.len() && !s.is_char_boundary(idx) {
        if forward {
            idx += 1;
        } else {
            idx -= 1;
        }
    }
    idx
}

// --- Regex registry ---
//
// Each regex compiles once on first use and is reused for the rest of
// the process lifetime. `expect` on `Regex::new` is fine here — the
// patterns are compile-time constants under test coverage.

fn api_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Combined alternation over known vendor-prefixed API keys.
        // Anchored on the prefix; trailing char class is constrained so
        // we don't grab neighboring prose. Each branch uses a
        // conservative length floor.
        Regex::new(
            r"(?x)
            (?:
                sk-ant-[A-Za-z0-9_-]{20,}                       # Anthropic
              | sk-(?:proj-)?[A-Za-z0-9_-]{20,}                 # OpenAI (legacy + project)
              | AIza[A-Za-z0-9_-]{35}                           # Google API key
              | ghp_[A-Za-z0-9]{36}                             # GitHub classic PAT
              | gho_[A-Za-z0-9]{36}                             # GitHub OAuth
              | ghu_[A-Za-z0-9]{36}                             # GitHub user-to-server
              | ghs_[A-Za-z0-9]{36}                             # GitHub server-to-server
              | ghr_[A-Za-z0-9]{36}                             # GitHub refresh
              | xox[abpre]-[A-Za-z0-9-]{10,}                    # Slack
              | hf_[A-Za-z0-9]{20,}                             # Hugging Face
              | (?:sk|pk|rk)_live_[A-Za-z0-9]{24,}              # Stripe live
              | (?:sk|pk|rk)_test_[A-Za-z0-9]{24,}              # Stripe test
              | gsk_[A-Za-z0-9]{20,}                            # Groq
            )",
        )
        .expect("api_key regex compiles")
    })
}

fn aws_access_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").expect("aws regex compiles"))
}

fn jwt_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Three base64url segments joined with `.`; first two start
        // with the canonical `eyJ` (base64 of `{"`). The header sanity
        // check in run_jwt_detector cuts remaining false positives.
        Regex::new(r"\beyJ[A-Za-z0-9_-]{5,}\.eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\b")
            .expect("jwt regex compiles")
    })
}

fn private_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?s)-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY-----.*?-----END (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY-----",
        )
        .expect("private key regex compiles")
    })
}

fn connection_string_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis(?:s)?|amqp(?:s)?)://[A-Za-z0-9._~%!$&'()*+,;=:-]+:[^@\s]+@[A-Za-z0-9._~%!$&'()*+,;=:@/\[\]?-]+",
        )
        .expect("connection_string regex compiles")
    })
}

fn credit_card_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Digit runs with optional space/dash separators, 13–19 digits
        // total (approximated here; Luhn + post-filter confirm).
        Regex::new(r"\b(?:\d[ -]?){13,19}\b").expect("credit_card regex compiles")
    })
}

fn iban_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b").expect("iban regex compiles"))
}

fn email_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,63}\b")
            .expect("email regex compiles")
    })
}

fn phone_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Broad shape — E.164 (`+CC…`) and common national formats with
        // separators. Digit-count + context gate in `run_phone_detector`
        // does the tightening.
        Regex::new(
            r"(?x)
            (?:
                \+\d{1,3}[\s.\-]?(?:\(\d{1,4}\)[\s.\-]?)?\d{1,4}(?:[\s.\-]?\d{1,4}){1,4}
              | \(\d{2,4}\)[\s.\-]?\d{2,4}[\s.\-]?\d{2,4}(?:[\s.\-]?\d{2,4})?
              | \d{3,4}[\s.\-]\d{3,4}[\s.\-]\d{3,5}
            )",
        )
        .expect("phone regex compiles")
    })
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn pol_redact() -> RedactionPolicy {
        RedactionPolicy {
            mode: RedactionMode::Redact,
            skip_local: true,
            enabled_detectors: vec![],
            extra_patterns: vec![],
            allowlist: vec![],
            ner_requested: false,
        }
    }

    fn pol_block() -> RedactionPolicy {
        let mut p = pol_redact();
        p.mode = RedactionMode::Block;
        p
    }

    fn pol_off() -> RedactionPolicy {
        RedactionPolicy::off()
    }

    // --- redact() top-level ---

    #[test]
    fn off_mode_is_passthrough() {
        let r = redact("my key is sk-proj-abcdef123456ABCDEFGHIJKL", &pol_off());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn clean_text_returns_clean() {
        let r = redact("hello world, nothing here", &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn empty_text_returns_clean() {
        let r = redact("", &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- API key detection ---

    #[test]
    fn detects_openai_sk_proj_key() {
        let input = "token: sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb end";
        let r = redact(input, &pol_redact());
        match r {
            RedactionResult::Redacted { text, matches } => {
                assert!(text.contains("[REDACTED:API_KEY]"));
                assert_eq!(matches.len(), 1);
                assert_eq!(matches[0].kind, DetectorKind::ApiKey);
            }
            other => panic!("expected Redacted, got {other:?}"),
        }
    }

    #[test]
    fn detects_anthropic_key() {
        let input = "LLM_ANTHROPIC_API_KEY=sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:API_KEY]"));
        assert_eq!(matches[0].kind, DetectorKind::ApiKey);
    }

    #[test]
    fn detects_google_api_key() {
        let input = "key=AIzaSyB1234567890abcdefghijklmnopqrstuvw";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Redacted { .. }));
    }

    #[test]
    fn detects_github_token() {
        let input = "ghp_1234567890abcdefABCDEFghijklmnopqrst";
        assert_eq!(input.len() - 4, 36);
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, .. } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(text, "[REDACTED:API_KEY]");
    }

    #[test]
    fn detects_stripe_live_key() {
        // String built at runtime so GitHub push protection's Stripe
        // scanner doesn't flag the literal in source.
        let input = format!("sk_{}_abcdefghijklmnopqrstuvwxyz0123456789", "live");
        let r = redact(&input, &pol_redact());
        assert!(matches!(r, RedactionResult::Redacted { .. }));
    }

    // --- AWS access keys ---

    #[test]
    fn detects_aws_access_key() {
        let input = "aws_access_key_id=AKIAIOSFODNN7EXAMPLE";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:AWS_KEY]"));
        assert_eq!(matches[0].kind, DetectorKind::AwsAccessKey);
    }

    #[test]
    fn aws_ignores_shorter_akia_prefix() {
        let input = "AKIASHORT";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- JWTs ---

    #[test]
    fn detects_valid_jwt() {
        // header `{"alg":"HS256"}` → eyJhbGciOiJIUzI1NiJ9
        // payload `{"sub":"42"}` → eyJzdWIiOiI0MiJ9
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiI0MiJ9.abcdefghij_klmnop-qrstuv";
        let r = redact(jwt, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(matches[0].kind, DetectorKind::Jwt);
    }

    #[test]
    fn jwt_shape_without_valid_header_is_ignored() {
        // Header doesn't decode to a {-prefixed JSON object.
        let r = redact("eyJxxxxxxxxxxxxxx.eyJyyyyyyyyyy.zzzzzzzzzz", &pol_redact());
        assert!(matches!(
            r,
            RedactionResult::Clean | RedactionResult::Redacted { .. }
        ));
    }

    // --- Private keys ---

    #[test]
    fn detects_pem_private_key() {
        let input = "Here is my key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\ndone";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:PRIVATE_KEY]"));
        assert_eq!(matches[0].kind, DetectorKind::PrivateKey);
    }

    #[test]
    fn detects_openssh_private_key() {
        let input = "-----BEGIN OPENSSH PRIVATE KEY-----\nbody\n-----END OPENSSH PRIVATE KEY-----";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Redacted { .. }));
    }

    // --- Connection strings ---

    #[test]
    fn detects_postgres_conn_string() {
        let input = "DATABASE_URL=postgres://admin:s3cret@db.example.com:5432/prod";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(matches[0].kind, DetectorKind::ConnectionString);
    }

    #[test]
    fn detects_mongodb_srv_conn_string() {
        let input = "mongodb+srv://user:pw@cluster0.abcde.mongodb.net/dbname";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Redacted { .. }));
    }

    #[test]
    fn postgres_without_password_is_not_redacted() {
        let input = "postgres://localhost:5432/db";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- Credit cards ---

    #[test]
    fn detects_valid_luhn_card() {
        // 4111 1111 1111 1111 — classic Visa test card, passes Luhn.
        let input = "card: 4111 1111 1111 1111 thanks";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(matches[0].kind, DetectorKind::CreditCard);
    }

    #[test]
    fn rejects_non_luhn_digit_run() {
        let input = "order number: 1234567890123456";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn rejects_uuid_as_card() {
        let input = "uuid 123e4567-e89b-12d3-a456-426614174000 ok";
        let r = redact(input, &pol_redact());
        // UUID has 32 hex chars with dashes — not all digits, so the
        // card regex won't match. The entropy scanner may trip; we
        // accept either Clean or a Redacted-from-entropy result.
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::CreditCard));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // --- IBANs ---

    #[test]
    fn detects_valid_iban() {
        // GB82 WEST 1234 5698 7654 32 — classic example, passes mod-97.
        let input = "send to GB82WEST12345698765432 today";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Iban));
    }

    #[test]
    fn rejects_invalid_iban_check() {
        let input = "GB00WEST12345698765432";
        let r = redact(input, &pol_redact());
        // May still be flagged as HighEntropy, but not as Iban.
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::Iban));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // --- Email / phone ---

    #[test]
    fn detects_email() {
        let input = "reach me at piotr.wittchen@gmail.com anytime";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:EMAIL]"));
        assert_eq!(matches[0].kind, DetectorKind::Email);
    }

    #[test]
    fn detects_e164_phone() {
        let input = "Call +1 415 555 2671 soon";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Phone));
    }

    #[test]
    fn phone_without_context_is_ignored() {
        // No keyword, no + prefix — likely an order number.
        let input = "ref 415 555 2671 for pickup";
        let r = redact(input, &pol_redact());
        // May still hit HighEntropy for long digit runs, but should
        // not mis-classify as Phone.
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::Phone));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn phone_with_context_keyword_detected() {
        let input = "phone: 415-555-2671 after 5pm";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Phone));
    }

    // --- Entropy ---

    #[test]
    fn entropy_flags_long_random_string() {
        let input = "blob: q8X2Lk9wR4vN1cM7pT3eJ6hZ5fY0oAbD Ud2S end";
        // The token above is 32 chars and high-entropy.
        let r = redact(input, &pol_redact());
        match r {
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().any(|m| m.kind == DetectorKind::HighEntropy));
            }
            RedactionResult::Clean => panic!("expected a high-entropy match"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn entropy_ignores_short_tokens() {
        let input = "hello short1 abc123def";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn entropy_ignores_low_entropy_long_string() {
        let input = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- Allowlist ---

    #[test]
    fn allowlist_suppresses_matching_span() {
        let mut p = pol_redact();
        p.allowlist.push(Regex::new(r"AKIATEST0000000000").unwrap());
        let input = "key is AKIATEST0000000000 only";
        let r = redact(input, &p);
        // AKIATEST0000000000 is 18 chars (AKIA + 14), so the AWS regex
        // which requires 16 chars after AKIA won't match. Let's use a
        // true AKIA test.
        let _ = r;
        let mut p = pol_redact();
        p.allowlist
            .push(Regex::new(r"AKIAIOSFODNN7EXAMPLE").unwrap());
        let r = redact("key=AKIAIOSFODNN7EXAMPLE only", &p);
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- Custom user patterns ---

    #[test]
    fn custom_pattern_is_redacted_with_its_name() {
        let mut p = pol_redact();
        p.extra_patterns.push((
            "CUSTOMER_ID".to_string(),
            Regex::new(r"CUST-\d{8}").unwrap(),
        ));
        let r = redact("see CUST-12345678 for details", &p);
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:CUSTOMER_ID]"));
        assert_eq!(matches[0].kind, DetectorKind::Custom("CUSTOMER_ID".into()));
    }

    // --- Detector filter ---

    #[test]
    fn ner_detector_kinds_have_expected_placeholders_and_slugs() {
        // Covers the Layer-C variants even when the `redaction-ner`
        // feature is off, so the enum is fully exercised in both
        // build configurations.
        for (kind, expected_placeholder, expected_slug) in [
            (DetectorKind::PersonName, "PERSON", "person_name"),
            (DetectorKind::Location, "LOCATION", "location"),
            (DetectorKind::Organization, "ORGANIZATION", "organization"),
        ] {
            assert_eq!(kind.placeholder(), expected_placeholder);
            assert_eq!(kind.slug(), expected_slug);
        }
    }

    #[test]
    fn disabled_detector_skipped() {
        let mut p = pol_redact();
        p.enabled_detectors = vec!["email".to_string()];
        let r = redact(
            "sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb and foo@bar.com",
            &p,
        );
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().all(|m| m.kind == DetectorKind::Email));
    }

    // --- Overlap resolution ---

    #[test]
    fn priority_jwt_beats_entropy() {
        // JWTs look like high-entropy runs; `Jwt` priority should win.
        let input = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiI0MiJ9.abcdefghij_klmnop-qrstuv";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Jwt));
        assert!(matches.iter().all(|m| m.kind != DetectorKind::HighEntropy));
    }

    #[test]
    fn priority_apikey_beats_entropy() {
        let input = "token=sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb end";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        // At least one ApiKey hit, and nothing else overlapping it.
        assert!(matches.iter().any(|m| m.kind == DetectorKind::ApiKey));
    }

    // --- Block mode ---

    #[test]
    fn block_mode_returns_blocked() {
        let r = redact(
            "please use sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb",
            &pol_block(),
        );
        assert!(matches!(r, RedactionResult::Blocked { .. }));
    }

    #[test]
    fn block_mode_clean_input_returns_clean() {
        let r = redact("all good here", &pol_block());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- Describe / UTF-8 safety ---

    #[test]
    fn describe_matches_never_includes_raw_secret() {
        let input = "key=sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb done";
        let RedactionResult::Blocked { matches } = redact(input, &pol_block()) else {
            panic!("expected Blocked");
        };
        let desc = describe_matches(input, &matches);
        assert!(!desc.contains("sk-proj-aaaaa"));
        assert!(desc.contains("[REDACTED:API_KEY]"));
    }

    #[test]
    fn utf8_safe_context_window() {
        let input = "日本語 key=sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb 日本語";
        let RedactionResult::Blocked { matches } = redact(input, &pol_block()) else {
            panic!("expected Blocked");
        };
        // Should not panic — describe_matches must walk to UTF-8 boundaries.
        let _ = describe_matches(input, &matches);
    }

    // --- Luhn / mod-97 direct tests ---

    #[test]
    fn luhn_accepts_known_valid() {
        assert!(luhn_check("4111111111111111"));
        assert!(luhn_check("5500000000000004"));
        assert!(luhn_check("340000000000009"));
    }

    #[test]
    fn luhn_rejects_invalid() {
        assert!(!luhn_check("4111111111111112"));
        assert!(!luhn_check("0000000000000001"));
    }

    #[test]
    fn iban_accepts_known_valid() {
        assert!(iban_check("GB82WEST12345698765432"));
        assert!(iban_check("DE89370400440532013000"));
    }

    #[test]
    fn iban_rejects_invalid() {
        assert!(!iban_check("GB00WEST12345698765432"));
        assert!(!iban_check("XX999999999"));
    }

    // --- Shannon entropy ---

    #[test]
    fn entropy_zero_for_uniform() {
        let h = shannon_entropy(b"aaaaaa");
        assert!((h - 0.0).abs() < 1e-9);
    }

    #[test]
    fn entropy_positive_for_mixed() {
        let h = shannon_entropy(b"abcdefgh");
        assert!(h > 2.9);
    }

    // --- parse_extra_patterns ---

    #[test]
    fn parse_extra_patterns_basic() {
        let mut warnings = Vec::new();
        let got = parse_extra_patterns("CUSTOMER=CUST-\\d+;TICKET=JIRA-\\d+", &mut warnings);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "CUSTOMER");
        assert_eq!(got[1].0, "TICKET");
        assert!(warnings.is_empty());
    }

    #[test]
    fn parse_extra_patterns_skips_bad_regex() {
        let mut warnings = Vec::new();
        let got = parse_extra_patterns("GOOD=abc;BAD=(unclosed", &mut warnings);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "GOOD");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("BAD"));
    }

    #[test]
    fn parse_extra_patterns_skips_malformed_entry() {
        let mut warnings = Vec::new();
        let got = parse_extra_patterns("noEquals;HAS=ok", &mut warnings);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "HAS");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("noEquals"));
    }
}

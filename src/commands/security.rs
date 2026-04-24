use crossterm::style::{Color, Stylize};

use crate::security::redaction::ner::{self, NerStatus};
use crate::security::redaction::{self, RedactionMode, RedactionPolicy};

pub fn print_security() {
    use crate::keys;

    let summary = crate::security::policy_summary();
    let max_label = summary
        .iter()
        .map(|(k, _)| k.len())
        .chain(std::iter::once("key storage".len()))
        .chain(keys::KEY_NAMES.iter().map(|n| n.len()))
        .chain(std::iter::once("redaction detectors".len()))
        .max()
        .unwrap_or(0);
    println!();
    for (key, value) in &summary {
        let pad = max_label - key.len() + 2;
        println!("  {}:{:pad$}{}", key.as_str().with(Color::Cyan), "", value);
    }
    print_redaction_block(max_label);
    print_key_storage(max_label);
}

/// Render the redaction status. The `redaction:` row lives at the
/// bottom of the summary block (after `disabled tools`) and — when the
/// layer is turned on — is followed by an indented detail block listing
/// active detectors, custom patterns, allowlist size, and NER state.
fn print_redaction_block(max_label: usize) {
    let pol = redaction::policy();
    let is_off = matches!(pol.mode, RedactionMode::Off);

    let label = "redaction";
    let pad = max_label - label.len() + 2;
    println!("  {}:{:pad$}{}", label.with(Color::Cyan), "", pol.summary(),);

    if is_off {
        return;
    }

    print_subrow(max_label, "local providers", local_providers_label(pol));
    print_subrow(max_label, "redaction detectors", detectors_label(pol));
    print_subrow_styled(
        max_label,
        "custom patterns",
        custom_patterns_label(pol),
        if pol.extra_patterns.is_empty() {
            Color::DarkGrey
        } else {
            Color::Yellow
        },
    );
    print_subrow_styled(
        max_label,
        "redaction allowlist",
        allowlist_label(pol),
        if pol.allowlist.is_empty() {
            Color::DarkGrey
        } else {
            Color::Yellow
        },
    );
    print_subrow_styled(max_label, "redaction ner", ner_label(pol), ner_color(pol));
}

fn local_providers_label(pol: &RedactionPolicy) -> (String, Color) {
    if pol.skip_local {
        (
            "skip (ollama, gguf, mlx bypass redaction)".to_string(),
            Color::DarkGrey,
        )
    } else {
        (
            "include (redaction applies to local too)".to_string(),
            Color::Yellow,
        )
    }
}

fn detectors_label(pol: &RedactionPolicy) -> (String, Color) {
    if pol.enabled_detectors.is_empty() {
        (
            "all (api_key, aws, jwt, private_key, connection_string, credit_card, iban, email, phone, high_entropy)"
                .to_string(),
            Color::Green,
        )
    } else {
        (pol.enabled_detectors.join(", "), Color::Green)
    }
}

fn custom_patterns_label(pol: &RedactionPolicy) -> String {
    if pol.extra_patterns.is_empty() {
        "none".to_string()
    } else {
        let names: Vec<&str> = pol.extra_patterns.iter().map(|(n, _)| n.as_str()).collect();
        format!("{} ({})", names.len(), names.join(", "))
    }
}

fn allowlist_label(pol: &RedactionPolicy) -> String {
    if pol.allowlist.is_empty() {
        "none".to_string()
    } else if pol.allowlist.len() == 1 {
        "1 pattern".to_string()
    } else {
        format!("{} patterns", pol.allowlist.len())
    }
}

fn ner_label(pol: &RedactionPolicy) -> String {
    match ner::status(pol.ner_requested) {
        NerStatus::Disabled => "off".to_string(),
        NerStatus::FeatureMissing => {
            "requested but feature not built in (rebuild with --features redaction-ner)".to_string()
        }
        NerStatus::ModelMissing { expected_name } => format!(
            "on — model '{expected_name}' not pulled yet (run `aictl --pull-ner-model <owner>/<repo>`)"
        ),
        NerStatus::Ready { model_name } => format!("on — model '{model_name}' ready"),
    }
}

fn ner_color(pol: &RedactionPolicy) -> Color {
    match ner::status(pol.ner_requested) {
        NerStatus::Disabled => Color::DarkGrey,
        NerStatus::FeatureMissing | NerStatus::ModelMissing { .. } => Color::Red,
        NerStatus::Ready { .. } => Color::Green,
    }
}

fn print_subrow(max_label: usize, label: &str, value: (String, Color)) {
    let (text, color) = value;
    let pad = max_label - label.len() + 3;
    println!(
        "  {}{:pad$}{}",
        label.with(Color::DarkGrey),
        "",
        text.with(color),
    );
}

fn print_subrow_styled(max_label: usize, label: &str, text: String, color: Color) {
    let pad = max_label - label.len() + 3;
    println!(
        "  {}{:pad$}{}",
        label.with(Color::DarkGrey),
        "",
        text.with(color),
    );
}

/// Print the current API key storage backend and per-key status.
fn print_key_storage(max_label: usize) {
    use crate::keys::{self, KeyLocation};

    let backend = keys::backend_name();
    let (locked, plain, both, _unset) = keys::counts();
    let key = "key storage";
    let pad = max_label - key.len() + 2;
    println!(
        "  {}:{:pad$}{} {}",
        key.with(Color::Cyan),
        "",
        backend.with(Color::Green),
        format!("({locked} locked · {plain} plain · {both} both)").with(Color::DarkGrey),
    );
    for (name, loc) in keys::all_locations() {
        let label = loc.label();
        let color = match loc {
            KeyLocation::Keyring => Color::Green,
            KeyLocation::Config => Color::Yellow,
            KeyLocation::Both => Color::Red,
            KeyLocation::None => Color::DarkGrey,
        };
        let pad = max_label - name.len() + 3;
        println!(
            "  {}{:pad$}{}",
            name.with(Color::DarkGrey),
            "",
            label.with(color),
        );
    }
    println!();
}

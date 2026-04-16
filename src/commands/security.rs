use crossterm::style::{Color, Stylize};

pub fn print_security() {
    use crate::keys;

    let summary = crate::security::policy_summary();
    let max_label = summary
        .iter()
        .map(|(k, _)| k.len())
        .chain(std::iter::once("key storage".len()))
        .chain(keys::KEY_NAMES.iter().map(|n| n.len()))
        .max()
        .unwrap_or(0);
    println!();
    for (key, value) in &summary {
        let pad = max_label - key.len() + 2;
        println!("  {}:{:pad$}{}", key.as_str().with(Color::Cyan), "", value);
    }
    print_key_storage(max_label);
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

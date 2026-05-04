use aictl_core::llm::gguf::CATALOG as LMSTUDIO_CATALOG;
use crossterm::style::{Color, Stylize};

use crate::ui::AgentUI;

use super::menu::{
    confirm_yn, format_size, prompt_line_cancellable, select_from_menu, show_cancelled,
};

const GGUF_MENU_ITEMS: &[(&str, &str)] = &[
    ("view downloaded", "list models in ~/.aictl/models/gguf/"),
    (
        "pull model",
        "download a GGUF model from Hugging Face or URL",
    ),
    ("remove model", "delete a downloaded model"),
    ("clear all", "remove every downloaded model"),
];

fn build_gguf_menu_lines(selected: usize) -> Vec<String> {
    let max = GGUF_MENU_ITEMS
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    GGUF_MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let is_selected = i == selected;
            let padded = format!("{name:<max$}");
            let name_styled = if is_selected {
                format!(
                    "{}",
                    padded
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("{}", padded.with(Color::DarkGrey))
            };
            let desc_styled = format!("{}", desc.with(Color::DarkGrey));
            if is_selected {
                format!("  {} {name_styled}  {desc_styled}", "›".with(Color::Cyan))
            } else {
                format!("    {name_styled}  {desc_styled}")
            }
        })
        .collect()
}

fn print_gguf_models() {
    let models = crate::llm::gguf::list_models();
    println!();
    if !crate::llm::gguf::is_available() {
        println!(
            "  {}",
            "native inference is not compiled in — rebuild with `cargo build --features gguf` to use downloaded models".with(Color::Yellow)
        );
    }
    if models.is_empty() {
        println!("  {}", "no local models downloaded".with(Color::DarkGrey));
        println!();
        return;
    }
    let dir = crate::llm::gguf::models_dir();
    for m in &models {
        let path = dir.join(format!("{m}.gguf"));
        let size = std::fs::metadata(&path)
            .ok()
            .map_or_else(|| "?".to_string(), |meta| format_size(meta.len()));
        println!(
            "  {} {}  {}",
            "●".with(Color::Green),
            m.as_str().with(Color::White),
            size.with(Color::DarkGrey),
        );
    }
    println!();
}

fn build_lmstudio_catalog_menu_lines(selected: usize) -> Vec<String> {
    let max_label = LMSTUDIO_CATALOG
        .iter()
        .map(|e| e.label.len())
        .max()
        .unwrap_or(0);
    let total = LMSTUDIO_CATALOG.len() + 1; // +1 for "custom spec"
    (0..total)
        .map(|i| {
            let is_selected = i == selected;
            let (label, size) = if i < LMSTUDIO_CATALOG.len() {
                let e = &LMSTUDIO_CATALOG[i];
                (e.label.to_string(), e.size_label.to_string())
            } else {
                (
                    "custom spec (hf:/owner/repo:/https://...)".to_string(),
                    String::new(),
                )
            };
            let padded = format!("{label:<max_label$}");
            let name_styled = if is_selected {
                padded
                    .with(Color::White)
                    .attribute(crossterm::style::Attribute::Bold)
                    .to_string()
            } else {
                padded.with(Color::DarkGrey).to_string()
            };
            let size_styled = format!("{}", size.with(Color::DarkGrey));
            if is_selected {
                format!("  {} {name_styled}  {size_styled}", "›".with(Color::Cyan))
            } else {
                format!("    {name_styled}  {size_styled}")
            }
        })
        .collect()
}

async fn pull_gguf_model(ui: &dyn AgentUI, show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {}",
        "curated community GGUFs on Hugging Face (mostly LM Studio's lmstudio-community mirror)"
            .with(Color::DarkGrey)
    );
    let total = LMSTUDIO_CATALOG.len() + 1;
    let Some(sel) = select_from_menu(total, 0, build_lmstudio_catalog_menu_lines) else {
        return;
    };

    let spec = if sel < LMSTUDIO_CATALOG.len() {
        LMSTUDIO_CATALOG[sel].spec.to_string()
    } else {
        println!();
        println!("  {}", "spec examples:".with(Color::DarkGrey));
        println!(
            "    {}",
            "hf:TheBloke/Llama-2-7B-Chat-GGUF/llama-2-7b-chat.Q4_K_M.gguf".with(Color::DarkGrey)
        );
        println!(
            "    {}",
            "bartowski/Llama-3.2-3B-Instruct-GGUF:Llama-3.2-3B-Instruct-Q4_K_M.gguf"
                .with(Color::DarkGrey)
        );
        println!(
            "    {}",
            "https://host/path/model.gguf".with(Color::DarkGrey)
        );
        match prompt_line_cancellable("spec:") {
            Ok(s) if s.trim().is_empty() => {
                show_cancelled();
                return;
            }
            Ok(s) => s.trim().to_string(),
            Err(()) => {
                show_cancelled();
                return;
            }
        }
    };

    let name_override = if let Ok(s) =
        prompt_line_cancellable("local name (optional, press enter to use default):")
    {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    } else {
        show_cancelled();
        return;
    };

    let download = crate::llm::gguf::download_model(ui, &spec, name_override.as_deref());
    match crate::with_esc_cancel(ui, download).await {
        Ok(Ok(name)) => {
            println!();
            println!(
                "  {} downloaded {}",
                "✓".with(Color::Green),
                name.with(Color::White)
            );
            println!();
        }
        Ok(Err(e)) => show_error(&format!("download failed: {e}")),
        Err(_) => {
            println!();
            println!(
                "  {} download cancelled (partial file removed)",
                "✗".with(Color::Yellow)
            );
            println!();
            // Clean up the leaked .part file so the next attempt starts fresh.
            let _ = cleanup_partial_download(&spec, name_override.as_deref());
        }
    }
}

/// Best-effort cleanup of a `<name>.gguf.part` file left behind when a
/// download is cancelled via Esc. Silently ignores failures.
fn cleanup_partial_download(spec: &str, override_name: Option<&str>) -> std::io::Result<()> {
    // Resolve the same name the downloader would have used.
    let name = override_name.map_or_else(
        || {
            spec.rsplit('/')
                .next()
                .and_then(|f| f.split('?').next())
                .map(|f| {
                    std::path::Path::new(f)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(f)
                        .to_string()
                })
                .unwrap_or_default()
        },
        String::from,
    );
    if name.is_empty() {
        return Ok(());
    }
    let path = crate::llm::gguf::models_dir().join(format!("{name}.gguf.part"));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn remove_gguf_model_interactive(show_error: &dyn Fn(&str)) {
    let models = crate::llm::gguf::list_models();
    if models.is_empty() {
        println!();
        println!("  {}", "no local models to remove".with(Color::DarkGrey));
        println!();
        return;
    }
    let build = |sel: usize| -> Vec<String> {
        models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let is_selected = i == sel;
                if is_selected {
                    format!(
                        "  {} {}",
                        "›".with(Color::Cyan),
                        m.as_str()
                            .with(Color::White)
                            .attribute(crossterm::style::Attribute::Bold)
                    )
                } else {
                    format!("    {}", m.as_str().with(Color::DarkGrey))
                }
            })
            .collect()
    };
    let Some(sel) = select_from_menu(models.len(), 0, build) else {
        return;
    };
    let name = &models[sel];
    println!();
    if !confirm_yn(&format!("remove local model '{name}'?")) {
        return;
    }
    match crate::llm::gguf::remove_model(name) {
        Ok(()) => {
            println!();
            println!(
                "  {} removed {}",
                "✓".with(Color::Green),
                name.as_str().with(Color::White)
            );
            println!();
        }
        Err(e) => show_error(&format!("remove failed: {e}")),
    }
}

fn clear_all_gguf_models_confirm() {
    println!();
    if !confirm_yn("remove ALL downloaded local models?") {
        return;
    }
    match crate::llm::gguf::clear_models() {
        Ok(n) => {
            println!();
            println!("  {} removed {n} local model(s)", "✓".with(Color::Green));
            println!();
        }
        Err(e) => {
            println!();
            println!(
                "  {} {}",
                "✗".with(Color::Red),
                e.to_string().with(Color::Red)
            );
            println!();
        }
    }
}

/// Interactive `/gguf` menu: list / pull / remove / clear.
pub async fn run_gguf_menu(ui: &dyn AgentUI, show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {} {}",
        "⚠".with(Color::Yellow),
        "native GGUF model support is experimental — expect rough edges".with(Color::Yellow)
    );
    println!();
    let Some(sel) = select_from_menu(GGUF_MENU_ITEMS.len(), 0, build_gguf_menu_lines) else {
        return;
    };
    match sel {
        0 => print_gguf_models(),
        1 => pull_gguf_model(ui, show_error).await,
        2 => remove_gguf_model_interactive(show_error),
        3 => clear_all_gguf_models_confirm(),
        _ => {}
    }
}

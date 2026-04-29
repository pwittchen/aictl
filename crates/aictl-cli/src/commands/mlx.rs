use crossterm::style::{Color, Stylize};

use crate::ui::AgentUI;

use super::menu::{
    confirm_yn, format_size, prompt_line_cancellable, select_from_menu, show_cancelled,
};

const MLX_MENU_ITEMS: &[(&str, &str)] = &[
    ("view downloaded", "list models in ~/.aictl/models/mlx/"),
    (
        "pull model",
        "download an MLX model from Hugging Face (mlx-community)",
    ),
    ("remove model", "delete a downloaded model"),
    ("clear all", "remove every downloaded model"),
];

fn build_mlx_menu_lines(selected: usize) -> Vec<String> {
    let max = MLX_MENU_ITEMS
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0);
    MLX_MENU_ITEMS
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

fn print_mlx_models() {
    let models = crate::llm::mlx::list_models();
    println!();
    if !crate::llm::mlx::host_supports_mlx() {
        println!(
            "  {}",
            "MLX inference requires macOS + Apple Silicon — downloaded models on this host can't run"
                .with(Color::Yellow)
        );
    } else if !crate::llm::mlx::is_available() {
        println!(
            "  {}",
            "native MLX inference is not compiled in — rebuild with `cargo build --features mlx` to use downloaded models".with(Color::Yellow)
        );
    }
    if models.is_empty() {
        println!("  {}", "no MLX models downloaded".with(Color::DarkGrey));
        println!();
        return;
    }
    for m in &models {
        let size = format_size(crate::llm::mlx::model_size(m));
        println!(
            "  {} {}  {}",
            "●".with(Color::Green),
            m.as_str().with(Color::White),
            size.with(Color::DarkGrey),
        );
    }
    println!();
}

/// Curated starter list of popular MLX-community repos on Hugging Face.
/// Sizes are approximate on-disk footprints for the 4-bit variants; the
/// actual download size will depend on what's in the repo tree.
const MLX_CATALOG: &[(&str, &str, &str)] = &[
    (
        "Llama 3.2 3B Instruct (4-bit)",
        "mlx-community/Llama-3.2-3B-Instruct-4bit",
        "~1.8 GB",
    ),
    (
        "Llama 3.1 8B Instruct (4-bit)",
        "mlx-community/Meta-Llama-3.1-8B-Instruct-4bit",
        "~4.5 GB",
    ),
    (
        "Qwen2.5 7B Instruct (4-bit)",
        "mlx-community/Qwen2.5-7B-Instruct-4bit",
        "~4.3 GB",
    ),
    (
        "Qwen2.5 14B Instruct (4-bit)",
        "mlx-community/Qwen2.5-14B-Instruct-4bit",
        "~8.0 GB",
    ),
    (
        "Qwen2.5 Coder 7B Instruct (4-bit)",
        "mlx-community/Qwen2.5-Coder-7B-Instruct-4bit",
        "~4.3 GB",
    ),
    (
        "Mistral 7B Instruct v0.3 (4-bit)",
        "mlx-community/Mistral-7B-Instruct-v0.3-4bit",
        "~4.1 GB",
    ),
    (
        "Gemma 2 9B Instruct (4-bit)",
        "mlx-community/gemma-2-9b-it-4bit",
        "~5.3 GB",
    ),
    (
        "Phi-3.5 Mini Instruct (4-bit)",
        "mlx-community/Phi-3.5-mini-instruct-4bit",
        "~2.2 GB",
    ),
    (
        "DeepSeek R1 Distill Qwen 7B (4-bit)",
        "mlx-community/DeepSeek-R1-Distill-Qwen-7B-4bit",
        "~4.3 GB",
    ),
    (
        "DeepSeek R1 Distill Qwen 14B (4-bit)",
        "mlx-community/DeepSeek-R1-Distill-Qwen-14B-4bit",
        "~8.0 GB",
    ),
];

fn build_mlx_catalog_menu_lines(selected: usize) -> Vec<String> {
    let max_label = MLX_CATALOG
        .iter()
        .map(|(l, _, _)| l.len())
        .max()
        .unwrap_or(0);
    let max_size = MLX_CATALOG
        .iter()
        .map(|(_, _, s)| s.len())
        .max()
        .unwrap_or(0);
    MLX_CATALOG
        .iter()
        .enumerate()
        .map(|(i, (label, spec, size))| {
            let is_selected = i == selected;
            let padded_label = format!("{label:<max_label$}");
            let padded_size = format!("{size:<max_size$}");
            let label_styled = if is_selected {
                format!(
                    "{}",
                    padded_label
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("{}", padded_label.with(Color::DarkGrey))
            };
            let size_styled = format!("{}", padded_size.with(Color::DarkGrey));
            let spec_styled = format!("{}", spec.with(Color::DarkGrey));
            if is_selected {
                format!(
                    "  {} {label_styled}  {size_styled}  {spec_styled}",
                    "›".with(Color::Cyan)
                )
            } else {
                format!("    {label_styled}  {size_styled}  {spec_styled}")
            }
        })
        .chain(std::iter::once({
            let label = "other (enter a custom spec)";
            let is_selected = selected == MLX_CATALOG.len();
            let padded_label = format!("{label:<max_label$}");
            if is_selected {
                format!(
                    "  {} {}",
                    "›".with(Color::Cyan),
                    padded_label
                        .with(Color::White)
                        .attribute(crossterm::style::Attribute::Bold)
                )
            } else {
                format!("    {}", padded_label.with(Color::DarkGrey))
            }
        }))
        .collect()
}

async fn pull_mlx_model(ui: &dyn AgentUI, show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {}",
        "curated from mlx-community on Hugging Face (huggingface.co/mlx-community)"
            .with(Color::DarkGrey)
    );
    let total = MLX_CATALOG.len() + 1;
    let Some(sel) = select_from_menu(total, 0, build_mlx_catalog_menu_lines) else {
        return;
    };

    let spec = if sel < MLX_CATALOG.len() {
        MLX_CATALOG[sel].1.to_string()
    } else {
        println!();
        println!("  {}", "spec examples:".with(Color::DarkGrey));
        println!(
            "    {}",
            "mlx:mlx-community/Llama-3.2-3B-Instruct-4bit".with(Color::DarkGrey)
        );
        println!(
            "    {}",
            "mlx-community/Qwen2.5-7B-Instruct-4bit".with(Color::DarkGrey)
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

    let download = crate::llm::mlx::download_model(ui, &spec, name_override.as_deref());
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
                "  {} download cancelled (partial directory left in place)",
                "✗".with(Color::Yellow)
            );
            println!();
        }
    }
}

fn remove_mlx_model_interactive(show_error: &dyn Fn(&str)) {
    let models = crate::llm::mlx::list_models();
    if models.is_empty() {
        println!();
        println!("  {}", "no MLX models to remove".with(Color::DarkGrey));
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
    if !confirm_yn(&format!("remove MLX model '{name}'?")) {
        return;
    }
    match crate::llm::mlx::remove_model(name) {
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

fn clear_all_mlx_models_confirm() {
    println!();
    if !confirm_yn("remove ALL downloaded MLX models?") {
        return;
    }
    match crate::llm::mlx::clear_models() {
        Ok(n) => {
            println!();
            println!("  {} removed {n} MLX model(s)", "✓".with(Color::Green));
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

/// Interactive `/mlx` menu: list / pull / remove / clear.
pub async fn run_mlx_menu(ui: &dyn AgentUI, show_error: &dyn Fn(&str)) {
    println!();
    println!(
        "  {} {}",
        "⚠".with(Color::Yellow),
        "native MLX model support is experimental — expect rough edges".with(Color::Yellow)
    );
    if !crate::llm::mlx::host_supports_mlx() {
        println!(
            "  {} {}",
            "⚠".with(Color::Yellow),
            "this host is not Apple Silicon — models can be downloaded but not run here"
                .with(Color::Yellow)
        );
    }
    println!();
    let Some(sel) = select_from_menu(MLX_MENU_ITEMS.len(), 0, build_mlx_menu_lines) else {
        return;
    };
    match sel {
        0 => print_mlx_models(),
        1 => pull_mlx_model(ui, show_error).await,
        2 => remove_mlx_model_interactive(show_error),
        3 => clear_all_mlx_models_confirm(),
        _ => {}
    }
}

use crossterm::style::{Color, Stylize};

use crate::agents;

use super::memory::MemoryMode;

#[allow(clippy::too_many_lines)]
pub fn print_info(
    provider: &str,
    model: &str,
    auto: bool,
    memory: MemoryMode,
    version_info: &str,
    ollama_models: &[String],
) {
    let version = crate::VERSION;
    let behavior = if auto { "auto" } else { "human-in-the-loop" };
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let current_exe = std::env::current_exe().ok();
    let binary_path = current_exe
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |p| p.display().to_string());
    let binary_size = current_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or_else(
            || "unknown".to_string(),
            #[allow(clippy::cast_precision_loss)]
            |m| {
                let bytes = m.len();
                if bytes >= 1_048_576 {
                    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
                } else {
                    format!("{:.1} KB", bytes as f64 / 1_024.0)
                }
            },
        );

    let version_display = if version_info.is_empty() {
        version.to_string()
    } else {
        let version_color = if version_info.contains("latest") {
            Color::Green
        } else {
            Color::Yellow
        };
        format!("{version} {}", version_info.with(version_color))
    };

    println!();
    println!("  {} {version_display}", "version:  ".with(Color::Cyan));
    println!(
        "  {} {}",
        "build:    ".with(Color::Cyan),
        env!("AICTL_BUILD_DATETIME")
    );
    println!("  {} {provider}", "provider: ".with(Color::Cyan));
    println!("  {} {model}", "model:    ".with(Color::Cyan));
    println!("  {} {behavior}", "behavior: ".with(Color::Cyan));
    println!("  {} {memory}", "memory:   ".with(Color::Cyan));
    let timeout_secs = crate::config::llm_timeout().as_secs();
    let timeout_source = crate::config::config_get("AICTL_LLM_TIMEOUT")
        .and_then(|v| v.parse::<u64>().ok())
        .is_some();
    let timeout_display = if timeout_secs >= u64::MAX / 4 {
        "disabled (0, AICTL_LLM_TIMEOUT)".to_string()
    } else if timeout_source {
        format!("{timeout_secs}s (AICTL_LLM_TIMEOUT)")
    } else {
        format!("{timeout_secs}s (default)")
    };
    println!("  {} {timeout_display}", "timeout:  ".with(Color::Cyan));
    let max_iter = crate::config::max_iterations();
    let max_iter_source = crate::config::config_get("AICTL_MAX_ITERATIONS")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v >= 1)
        .is_some();
    let max_iter_display = if max_iter_source {
        format!("{max_iter} (AICTL_MAX_ITERATIONS)")
    } else {
        format!("{max_iter} (default)")
    };
    println!("  {} {max_iter_display}", "max iter: ".with(Color::Cyan));
    let primary_prompt_name = crate::config::config_get("AICTL_PROMPT_FILE")
        .unwrap_or_else(|| crate::config::DEFAULT_PROMPT_FILE.to_string());
    let prompt_info = match crate::config::load_prompt_file() {
        Some((name, _)) if name == primary_prompt_name => format!("{name} (loaded)"),
        Some((name, _)) => format!("{name} (loaded as fallback for {primary_prompt_name})"),
        None => {
            let fallback_status = if crate::config::prompt_fallback_enabled() {
                "fallback enabled"
            } else {
                "fallback disabled"
            };
            format!("{primary_prompt_name} (not found, {fallback_status})")
        }
    };

    println!("  {} {os}/{arch}", "os:       ".with(Color::Cyan));
    println!("  {} {binary_size}", "binary:   ".with(Color::Cyan));
    println!("  {} {binary_path}", "path:     ".with(Color::Cyan));
    println!("  {} {prompt_info}", "prompt:   ".with(Color::Cyan));
    let agent_info = agents::loaded_agent_name()
        .map_or_else(|| "(none)".to_string(), |n| format!("{n} (loaded)"));
    println!("  {} {agent_info}", "agent:    ".with(Color::Cyan));

    // Collect unique cloud providers from the static catalog. Anything in
    // MODELS counts as a cloud provider; ollama / native GGUF / native MLX
    // are listed separately under "local:".
    let mut cloud_providers: Vec<&str> = Vec::new();
    for &(prov, _, _) in crate::llm::MODELS {
        if !cloud_providers.contains(&prov) {
            cloud_providers.push(prov);
        }
    }
    let cloud_count = cloud_providers.len();
    let local_count = 3; // ollama + native GGUF + native MLX
    let model_count = crate::llm::MODELS.len();
    println!(
        "  {} {cloud_count} ({})",
        "cloud:    ".with(Color::Cyan),
        cloud_providers.join(", ")
    );
    let ollama_label = if ollama_models.is_empty() {
        "ollama [not running]".to_string()
    } else {
        format!("ollama [{} model(s)]", ollama_models.len())
    };
    println!(
        "  {} {local_count} ({ollama_label}, gguf, mlx)",
        "local:    ".with(Color::Cyan),
    );

    let experimental = "[experimental]".with(Color::Yellow).to_string();

    let gguf_models = crate::llm::gguf::list_models();
    let gguf_available = crate::llm::gguf::is_available();
    let gguf_feature_label = if gguf_available {
        "enabled".with(Color::Green).to_string()
    } else {
        "disabled (rebuild with --features gguf)"
            .with(Color::Yellow)
            .to_string()
    };
    let gguf_info = format!(
        "{} downloaded · inference {gguf_feature_label} {experimental}",
        gguf_models.len()
    );
    println!("  {} {gguf_info}", "gguf:     ".with(Color::Cyan));

    let mlx_models = crate::llm::mlx::list_models();
    let mlx_available = crate::llm::mlx::is_available();
    let mlx_host_ok = crate::llm::mlx::host_supports_mlx();
    let mlx_feature_label = if mlx_available {
        "enabled".with(Color::Green).to_string()
    } else if !mlx_host_ok {
        "disabled (requires macOS + Apple Silicon)"
            .with(Color::Yellow)
            .to_string()
    } else {
        "disabled (rebuild with --features mlx)"
            .with(Color::Yellow)
            .to_string()
    };
    let mlx_info = format!(
        "{} downloaded · inference {mlx_feature_label} {experimental}",
        mlx_models.len()
    );
    println!("  {} {mlx_info}", "mlx:      ".with(Color::Cyan));

    if mlx_host_ok {
        let metallib = current_exe
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.join("mlx.metallib"));
        let metallib_info = match metallib {
            Some(p) if p.exists() => p.display().to_string(),
            Some(p) => format!("(not found at {})", p.display()),
            None => "(unknown)".to_string(),
        };
        println!("  {} {metallib_info}", "metallib: ".with(Color::Cyan));
    }

    let total_models = model_count + ollama_models.len() + gguf_models.len() + mlx_models.len();
    println!(
        "  {} {total_models} ({model_count} cataloged, {} ollama, {} gguf, {} mlx)",
        "models:   ".with(Color::Cyan),
        ollama_models.len(),
        gguf_models.len(),
        mlx_models.len()
    );
    let tool_count = crate::tools::TOOL_COUNT;
    let disabled = crate::security::policy().disabled_tools.len();
    let tools_info = if crate::tools::tools_enabled() {
        if disabled > 0 {
            format!("{tool_count} ({disabled} disabled)")
        } else {
            format!("{tool_count}")
        }
    } else {
        "disabled — pure chat mode (AICTL_TOOLS_ENABLED=false)"
            .with(Color::Yellow)
            .to_string()
    };
    println!("  {} {tools_info}", "tools:    ".with(Color::Cyan));

    let plugins_explicitly_disabled = matches!(
        crate::config::config_get("AICTL_PLUGINS_ENABLED").as_deref(),
        Some("false" | "0")
    );
    let plugins_info = if plugins_explicitly_disabled {
        "disabled (AICTL_PLUGINS_ENABLED=false)"
            .with(Color::Yellow)
            .to_string()
    } else {
        format!("{}", crate::plugins::list().len())
    };
    println!("  {} {plugins_info}", "plugins:  ".with(Color::Cyan));

    let hooks = crate::hooks::list_all();
    let hooks_total = hooks.len();
    let hooks_disabled = hooks.iter().filter(|h| !h.enabled).count();
    let hooks_info = if hooks_disabled > 0 {
        format!("{hooks_total} ({hooks_disabled} disabled)")
    } else {
        format!("{hooks_total}")
    };
    println!("  {} {hooks_info}", "hooks:    ".with(Color::Cyan));
    println!();
}

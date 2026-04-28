use super::util::truncate_output;

pub(super) async fn tool_exec_shell(input: &str) -> String {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(input);

    // Environment scrubbing
    cmd.env_clear();
    for (key, value) in crate::security::scrubbed_env() {
        cmd.env(key, value);
    }

    let future = cmd.output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        match tokio::time::timeout(timeout, future).await {
            Ok(result) => result,
            Err(_) => return format!("Error: command timed out after {}s", timeout.as_secs()),
        }
    } else {
        future.await
    };

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("[stderr]\n");
                result.push_str(&stderr);
            }
            if result.is_empty() {
                result.push_str("(no output)");
            }
            truncate_output(&mut result);
            result
        }
        Err(e) => format!("Error executing command: {e}"),
    }
}

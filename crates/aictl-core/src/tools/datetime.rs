pub(super) async fn tool_fetch_datetime() -> String {
    match tokio::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M:%S %Z (%A)")
        .output()
        .await
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if stdout.is_empty() {
                "(could not determine date/time)".to_string()
            } else {
                stdout
            }
        }
        Err(e) => format!("Error fetching date/time: {e}"),
    }
}

pub(super) async fn tool_fetch_geolocation(input: &str) -> String {
    let ip = input.trim();
    let url = if ip.is_empty() {
        "http://ip-api.com/json/?fields=status,message,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as".to_string()
    } else {
        format!(
            "http://ip-api.com/json/{ip}?fields=status,message,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as"
        )
    };
    let client = crate::config::http_client();
    match client.get(&url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(json) => {
                if json["status"].as_str() == Some("fail") {
                    let msg = json["message"].as_str().unwrap_or("unknown error");
                    format!("Geolocation lookup failed: {msg}")
                } else {
                    serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string())
                }
            }
            Err(e) => format!("Error parsing geolocation response: {e}"),
        },
        Err(e) => format!("Error fetching geolocation data: {e}"),
    }
}

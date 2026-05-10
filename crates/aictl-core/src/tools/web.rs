use std::fmt::Write as _;

use super::util::truncate_output;

pub(super) async fn tool_search_web_fc(input: &str) -> String {
    let Some(api_key) = crate::keys::get_secret("FIRECRAWL_API_KEY") else {
        return "Error: FIRECRAWL_API_KEY not set in ~/.aictl/config or system keyring".to_string();
    };
    let query = input.trim();
    let client = crate::config::http_client();
    let body = serde_json::json!({
        "query": query,
        "limit": 5
    });
    match client
        .post("https://api.firecrawl.dev/v2/search")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                return format!("Error: Firecrawl API returned status {}", resp.status());
            }
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let results = json["data"]
                        .as_array()
                        .or_else(|| json["data"]["web"].as_array());
                    match results {
                        Some(items) if !items.is_empty() => {
                            let mut output = String::new();
                            for (i, item) in items.iter().enumerate() {
                                let title = item["title"].as_str().unwrap_or("(no title)");
                                let url = item["url"].as_str().unwrap_or("(no url)");
                                let desc = item["description"]
                                    .as_str()
                                    .or_else(|| item["snippet"].as_str())
                                    .unwrap_or("(no description)");
                                if i > 0 {
                                    output.push('\n');
                                }
                                let _ = write!(
                                    output,
                                    "[{}] {}\nURL: {}\n{}\n",
                                    i + 1,
                                    title,
                                    url,
                                    desc
                                );
                            }
                            output
                        }
                        _ => "No results found.".to_string(),
                    }
                }
                Err(e) => format!("Error parsing Firecrawl response: {e}"),
            }
        }
        Err(e) => format!("Error calling Firecrawl API: {e}"),
    }
}

/// `DuckDuckGo` fallback search. Two-stage: first hits the public
/// `api.duckduckgo.com` Instant Answer endpoint (the URL spec'd in the
/// project requirements — abstract + curated topics in JSON, no key);
/// when that returns nothing — which is the common case, since IA only
/// covers Wikipedia-grade entities — falls through to scraping the
/// `html.duckduckgo.com/html/` SERP page so the tool actually returns
/// real search results for arbitrary queries. Capped at 5 entries.
pub(super) async fn tool_search_web_ddg(input: &str) -> String {
    let query = input.trim();
    if query.is_empty() {
        return "Error: empty query".to_string();
    }
    let client = crate::config::http_client();

    let mut entries = ddg_instant_answer(query, client).await;
    if entries.is_empty() {
        match ddg_html_serp(query, client).await {
            Ok(serp) => entries = serp,
            Err(e) => return format!("Error calling DuckDuckGo: {e}"),
        }
    }

    if entries.is_empty() {
        return "No results found.".to_string();
    }

    let mut output = String::new();
    for (i, (title, url, desc)) in entries.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        let _ = write!(output, "[{}] {}\nURL: {}\n{}\n", i + 1, title, url, desc);
    }
    output
}

/// Stage 1: hit the Instant Answer JSON endpoint per the spec. Returns
/// the abstract (when one exists) plus flat related-topics — typically
/// empty for non-Wikipedia queries.
async fn ddg_instant_answer(
    query: &str,
    client: &reqwest::Client,
) -> Vec<(String, String, String)> {
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&pretty=1&no_html=1&skip_disambig=1&atb=v467-1",
        percent_encode(query),
    );
    let Ok(resp) = client.get(&url).send().await else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(json) = resp.json::<serde_json::Value>().await else {
        return Vec::new();
    };

    let mut entries: Vec<(String, String, String)> = Vec::new();

    let abstract_text = json["AbstractText"].as_str().unwrap_or("").trim();
    let abstract_url = json["AbstractURL"].as_str().unwrap_or("").trim();
    if !abstract_text.is_empty() && !abstract_url.is_empty() {
        let heading_field = json["Heading"].as_str().unwrap_or("").trim();
        let heading = if heading_field.is_empty() {
            query
        } else {
            heading_field
        };
        entries.push((
            heading.to_string(),
            abstract_url.to_string(),
            abstract_text.to_string(),
        ));
    }

    if let Some(topics) = json["RelatedTopics"].as_array() {
        for topic in topics {
            if entries.len() >= 5 {
                break;
            }
            // Some entries are grouped under a `Topics` array (category
            // sections); flatten those one level deep.
            if let Some(sub) = topic["Topics"].as_array() {
                for t in sub {
                    if entries.len() >= 5 {
                        break;
                    }
                    if let Some(e) = ddg_topic_entry(t) {
                        entries.push(e);
                    }
                }
            } else if let Some(e) = ddg_topic_entry(topic) {
                entries.push(e);
            }
        }
    }

    entries
}

/// Stage 2: scrape the `html.duckduckgo.com/html/` SERP page. This is
/// the same endpoint `DuckDuckGo` serves to JS-disabled clients, so it
/// works without an API key and returns full SERP results. Each result
/// row contains `<a class="result__a" href="...">title</a>` and
/// `<a class="result__snippet">description</a>`; we pair them in DOM
/// order. Sets a generic browser UA — the endpoint rejects requests
/// with reqwest's default UA.
async fn ddg_html_serp(
    query: &str,
    client: &reqwest::Client,
) -> Result<Vec<(String, String, String)>, String> {
    let body = format!("q={}&kl=wt-wt", percent_encode(query));
    let resp = client
        .post("https://html.duckduckgo.com/html/")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
        )
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP status {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("error reading body: {e}"))?;

    let document = scraper::Html::parse_document(&body);
    let title_sel = scraper::Selector::parse("a.result__a").map_err(|e| format!("{e:?}"))?;
    let snippet_sel =
        scraper::Selector::parse("a.result__snippet").map_err(|e| format!("{e:?}"))?;

    let titles: Vec<_> = document.select(&title_sel).collect();
    let snippets: Vec<_> = document.select(&snippet_sel).collect();

    let mut entries: Vec<(String, String, String)> = Vec::new();
    for (i, t) in titles.iter().enumerate() {
        if entries.len() >= 5 {
            break;
        }
        let href = t.value().attr("href").unwrap_or("").trim();
        let url = unwrap_ddg_redirect(href);
        if url.is_empty() {
            continue;
        }
        let title: String = t.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let desc = snippets
            .get(i)
            .map(|s| s.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let desc = if desc.is_empty() {
            "(no description)".to_string()
        } else {
            desc
        };
        entries.push((title, url, desc));
    }
    Ok(entries)
}

/// `DuckDuckGo` sometimes wraps SERP hrefs in a redirector
/// (`//duckduckgo.com/l/?uddg=<percent-encoded-url>&...`). Unwrap to the
/// real destination so the model gets a usable URL.
fn unwrap_ddg_redirect(href: &str) -> String {
    let normalized = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        href.to_string()
    };
    if let Some(idx) = normalized.find("uddg=") {
        let tail = &normalized[idx + "uddg=".len()..];
        let raw = tail.split('&').next().unwrap_or("");
        if let Some(decoded) = percent_decode(raw) {
            return decoded;
        }
    }
    normalized
}

/// Inverse of [`percent_encode`] — decodes `%XX` triplets back to the
/// original byte sequence and returns it as UTF-8. Returns `None` on
/// invalid escapes or non-UTF-8 byte sequences (we'd rather skip the
/// entry than feed the model a corrupt URL).
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            // hi and lo are 0..=15 (each from a single hex digit), so
            // (hi * 16 + lo) is 0..=255 — fits in a u8 without truncation.
            #[allow(clippy::cast_possible_truncation)]
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Minimal percent-encoder for the `DuckDuckGo` query parameter — keeps
/// unreserved chars (RFC 3986: ALPHA / DIGIT / `-` `.` `_` `~`) verbatim
/// and percent-encodes everything else byte-by-byte. Avoids pulling in
/// `urlencoding` / `percent-encoding` for one call site.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn ddg_topic_entry(t: &serde_json::Value) -> Option<(String, String, String)> {
    let url = t["FirstURL"].as_str()?.trim();
    let text = t["Text"].as_str().unwrap_or("").trim();
    if url.is_empty() || text.is_empty() {
        return None;
    }
    // DuckDuckGo's Text field is `Title - description`; split on the
    // first `" - "` so we can render the same shape as the Firecrawl
    // result.
    let (title, desc) = match text.split_once(" - ") {
        Some((t, d)) => (t.to_string(), d.to_string()),
        None => (text.to_string(), text.to_string()),
    };
    Some((title, url.to_string(), desc))
}

pub(super) async fn tool_fetch_url(input: &str) -> String {
    let url = input.trim();
    let client = crate::config::http_client();
    match client.get(url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return format!("Error: HTTP status {}", resp.status());
            }
            match resp.text().await {
                Ok(body) => {
                    // Strip HTML tags
                    let mut result = String::with_capacity(body.len());
                    let mut in_tag = false;
                    for ch in body.chars() {
                        if ch == '<' {
                            in_tag = true;
                        } else if ch == '>' {
                            in_tag = false;
                        } else if !in_tag {
                            result.push(ch);
                        }
                    }
                    // Collapse whitespace runs
                    let mut collapsed = String::with_capacity(result.len());
                    let mut prev_ws = false;
                    for ch in result.chars() {
                        if ch.is_whitespace() {
                            if !prev_ws {
                                collapsed.push(if ch == '\n' { '\n' } else { ' ' });
                            }
                            prev_ws = true;
                        } else {
                            collapsed.push(ch);
                            prev_ws = false;
                        }
                    }
                    let mut result = collapsed.trim().to_string();
                    if result.is_empty() {
                        result = "(empty page)".to_string();
                    }
                    truncate_output(&mut result);
                    result
                }
                Err(e) => format!("Error reading response body: {e}"),
            }
        }
        Err(e) => format!("Error fetching URL: {e}"),
    }
}

pub(super) async fn tool_extract_website(input: &str) -> String {
    let url = input.trim();
    let client = crate::config::http_client();
    match client.get(url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return format!("Error: HTTP status {}", resp.status());
            }
            match resp.text().await {
                Ok(body) => {
                    let document = scraper::Html::parse_document(&body);
                    let noise_selectors = [
                        "script", "style", "nav", "header", "footer", "noscript", "svg", "form",
                        "iframe",
                    ];
                    let mut remove_ids = std::collections::HashSet::new();
                    for sel_str in &noise_selectors {
                        if let Ok(sel) = scraper::Selector::parse(sel_str) {
                            for el in document.select(&sel) {
                                remove_ids.insert(el.id());
                            }
                        }
                    }
                    let mut text = String::new();
                    for node_ref in document.tree.root().descendants() {
                        if let scraper::node::Node::Text(t) = node_ref.value() {
                            let skip = node_ref.ancestors().any(|a| remove_ids.contains(&a.id()));
                            if !skip {
                                text.push_str(&t.text);
                            }
                        }
                    }
                    // Collapse whitespace
                    let mut result = String::with_capacity(text.len());
                    let mut prev_ws = false;
                    for ch in text.chars() {
                        if ch.is_whitespace() {
                            if !prev_ws {
                                result.push(if ch == '\n' { '\n' } else { ' ' });
                            }
                            prev_ws = true;
                        } else {
                            result.push(ch);
                            prev_ws = false;
                        }
                    }
                    let mut result = result.trim().to_string();
                    if result.is_empty() {
                        result = "(no content extracted)".to_string();
                    }
                    truncate_output(&mut result);
                    result
                }
                Err(e) => format!("Error reading response body: {e}"),
            }
        }
        Err(e) => format!("Error fetching URL: {e}"),
    }
}

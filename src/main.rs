use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, ValueEnum)]
enum Provider {
    Openai,
    Anthropic,
}

#[derive(Parser)]
#[command(name = "aictl", about = "CLI tool for interacting with LLM APIs")]
struct Cli {
    /// LLM provider to use
    #[arg(short, long)]
    provider: Provider,

    /// API key for the provider (falls back to OPENAI_API_KEY or ANTHROPIC_API_KEY)
    #[arg(short = 'k', long)]
    api_key: Option<String>,

    /// Model to use (e.g. gpt-4o, claude-sonnet-4-20250514)
    #[arg(short, long)]
    model: String,

    /// Message to send to the LLM
    #[arg(short = 'M', long)]
    message: String,
}

// --- OpenAI types ---

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
}

#[derive(Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

// --- Anthropic types ---

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    text: String,
}

async fn call_openai(api_key: &str, model: &str, message: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let body = OpenAiRequest {
        model: model.to_string(),
        messages: vec![OpenAiMessage {
            role: "user".to_string(),
            content: message.to_string(),
        }],
    };

    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(format!("OpenAI API error ({status}): {text}").into());
    }

    let parsed: OpenAiResponse = serde_json::from_str(&text)?;
    parsed
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| "No response from OpenAI".into())
}

async fn call_anthropic(api_key: &str, model: &str, message: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let body = AnthropicRequest {
        model: model.to_string(),
        max_tokens: 4096,
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: message.to_string(),
        }],
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if !status.is_success() {
        return Err(format!("Anthropic API error ({status}): {text}").into());
    }

    let parsed: AnthropicResponse = serde_json::from_str(&text)?;
    parsed
        .content
        .first()
        .map(|c| c.text.clone())
        .ok_or_else(|| "No response from Anthropic".into())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let env_var = match cli.provider {
        Provider::Openai => "OPENAI_API_KEY",
        Provider::Anthropic => "ANTHROPIC_API_KEY",
    };

    let api_key = cli.api_key.unwrap_or_else(|| {
        std::env::var(env_var).unwrap_or_else(|_| {
            eprintln!("Error: API key not provided. Pass --api-key or set {env_var}");
            std::process::exit(1);
        })
    });

    let result = match cli.provider {
        Provider::Openai => call_openai(&api_key, &cli.model, &cli.message).await,
        Provider::Anthropic => call_anthropic(&api_key, &cli.model, &cli.message).await,
    };

    match result {
        Ok(response) => println!("{response}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

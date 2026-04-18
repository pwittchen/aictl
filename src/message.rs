//! Provider-agnostic conversation message types.
//!
//! These structs are the lingua franca between the agent loop and the
//! per-provider request encoders in [`crate::llm`]. Each provider converts
//! `&[Message]` into its native request payload (Anthropic top-level system
//! field, `OpenAI` inline messages, Gemini `systemInstruction`, etc.) so the
//! agent loop never has to special-case provider wire formats.

#[derive(Debug, Clone)]
pub(crate) enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub(crate) struct ImageData {
    pub base64_data: String,
    pub media_type: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Message {
    pub role: Role,
    pub content: String,
    pub images: Vec<ImageData>,
}

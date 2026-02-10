//! Anthropic API providers.
//!
//! Three provider variants for the Anthropic API:
//! - [`AnthropicKey`] — standard API key auth, minimal beta headers
//! - [`AnthropicOAuth`] — OAuth (Claude Max), full beta headers
//! - [`AnthropicFastMode`] — OAuth + research preview, adaptive thinking,
//!   billing header injection, and cch body hash
//!
//! Sensitive constants (seeds, salts) are passed as parameters from config
//! so nothing is hardcoded in source.

use genai::chat::{BodySerializer, ChatOptions, ReasoningEffort};
use genai::Headers;
use serde_json::Value;
use sha2::{Digest, Sha256};
use xxhash_rust::xxh64::xxh64;

use super::RequestCustomizer;

// ─── Constants ──────────────────────────────────────────────────────────────

/// Beta features for OAuth sessions (non-Opus 4.6 models).
/// Opus 4.6 uses different betas — see [`AnthropicFastMode`].
const OAUTH_BETA: &str = concat!(
    "claude-code-20250219,",
    "oauth-2025-04-20,",
    "context-management-2025-06-27,",
    "prompt-caching-scope-2026-01-05,",
    "effort-2025-11-24,",
    "interleaved-thinking-2025-05-14",
);

/// Beta features for Opus 4.6 (adaptive thinking replaces interleaved thinking).
const OPUS_46_BETA: &str = concat!(
    "claude-code-20250219,",
    "oauth-2025-04-20,",
    "context-management-2025-06-27,",
    "prompt-caching-scope-2026-01-05,",
    "effort-2025-11-24,",
    "adaptive-thinking-2026-01-28",
);

/// Beta features for API key sessions (no OAuth/claude-code betas).
const API_KEY_BETA: &str = "interleaved-thinking-2025-05-14";

/// Additional beta flag that activates fast mode (research preview).
const FAST_MODE_BETA: &str = "research-preview-2026-02-01";

/// Claude Code version string (used in User-Agent and billing header).
const CC_VERSION: &str = "2.1.37";

/// User-Agent string matching the Claude Code client.
const USER_AGENT: &str = concat!("claude-cli/", "2.1.37", " (external, cli)");

/// Anthropic API version header.
const ANTHROPIC_VERSION: &str = "2023-06-01";

// ─── Shared header builder ──────────────────────────────────────────────────

/// Build the common OAuth header set with a given beta string and access token.
fn oauth_headers(access_token: &str, beta: &str) -> Headers {
    Headers::from([
        (
            "authorization".to_string(),
            format!("Bearer {}", access_token),
        ),
        ("anthropic-beta".to_string(), beta.to_string()),
        (
            "anthropic-version".to_string(),
            ANTHROPIC_VERSION.to_string(),
        ),
        (
            "anthropic-dangerous-direct-browser-access".to_string(),
            "true".to_string(),
        ),
        ("user-agent".to_string(), USER_AGENT.to_string()),
        ("x-app".to_string(), "cli".to_string()),
    ])
}

// ─── CchSerializer ──────────────────────────────────────────────────────────

/// Body serializer that prepends the billing header as the first system block
/// and computes/injects the `cch` hash after JSON serialization.
///
/// 1. Builds the billing header from the first user message in the payload
/// 2. Inserts it at `system[0]` (no `cache_control`)
/// 3. Serializes the modified payload to JSON (with `cch=00000` placeholder)
/// 4. Replaces the placeholder with the real xxHash64-derived value
struct CchSerializer {
    seed: u64,
    cc_version_salt: Option<String>,
}

impl CchSerializer {
    /// Extract first user message text from the JSON payload.
    fn first_user_message(payload: &Value) -> String {
        payload
            .get("messages")
            .and_then(|m| m.as_array())
            .and_then(|msgs| {
                msgs.iter()
                    .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            })
            .and_then(|m| {
                // Handle both string content and array content
                match m.get("content") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Array(parts)) => parts
                        .iter()
                        .find_map(|p| p.get("text").and_then(|t| t.as_str()).map(String::from)),
                    _ => None,
                }
            })
            .unwrap_or_default()
    }
}

impl BodySerializer for CchSerializer {
    fn serialize(&self, payload: &Value) -> Result<String, String> {
        let mut payload = payload.clone();

        // Build and prepend billing header as first system block
        if let Some(ref salt) = self.cc_version_salt {
            let first_msg = Self::first_user_message(&payload);
            let header = build_billing_header(&first_msg, salt, CC_VERSION);
            let block = serde_json::json!({
                "type": "text",
                "text": header
            });
            match payload.get_mut("system") {
                Some(Value::Array(arr)) => arr.insert(0, block),
                Some(Value::String(s)) => {
                    let existing = serde_json::json!({
                        "type": "text",
                        "text": s.clone(),
                        "cache_control": {"type": "ephemeral"}
                    });
                    payload["system"] = serde_json::json!([block, existing]);
                },
                _ => {
                    payload["system"] = serde_json::json!([block]);
                },
            }
        }

        let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        Ok(apply_cch(&body, self.seed))
    }
}

// ─── Shared OAuth request builders ─────────────────────────────────────────

/// Extract the text of the first user message from a chat request.
/// Build extra body params for OAuth Opus 4.6 requests.
///
/// Both `AnthropicOAuth` and `AnthropicFastMode` include:
/// - `metadata: { "user_id": "user_{...}_account_{...}_session_{...}" }`
///
/// Fast mode additionally includes:
/// - `research_preview_2026_02: "active"`
/// - `context_management: { "edits": [{ "type": "clear_thinking_20251015", "keep": "all" }] }`
fn build_extra_body(
    user_id: Option<&str>,
    account_uuid: Option<&str>,
    session_id: &str,
    fast_mode: bool,
) -> Value {
    let mut body = serde_json::json!({});

    // Build metadata.user_id if we have the identity components
    if let (Some(uid), Some(acct)) = (user_id, account_uuid) {
        let metadata_user_id = build_user_id(uid, acct, session_id);
        body["metadata"] = serde_json::json!({
            "user_id": metadata_user_id
        });
    }

    body["context_management"] = serde_json::json!({
        "edits": [
            { "type": "clear_thinking_20251015", "keep": "all" }
        ]
    });

    if fast_mode {
        body["research_preview_2026_02"] = serde_json::json!("active");
    }

    body
}

// ─── AnthropicKey ───────────────────────────────────────────────────────────

/// Anthropic API key provider.
///
/// Standard API key auth via `ANTHROPIC_API_KEY` env var.
/// Minimal beta headers, supports extended thinking.
pub struct AnthropicKey;

impl RequestCustomizer for AnthropicKey {
    fn label(&self) -> &str {
        "anthropic-key"
    }

    fn apply_chat_options(&self, options: ChatOptions, thinking_budget: u32) -> ChatOptions {
        options
            .with_extra_headers(Headers::from([
                ("anthropic-beta".to_string(), API_KEY_BETA.to_string()),
                (
                    "anthropic-version".to_string(),
                    ANTHROPIC_VERSION.to_string(),
                ),
            ]))
            .with_capture_reasoning_content(true)
            .with_reasoning_effort(ReasoningEffort::Budget(thinking_budget))
    }
}

// ─── AnthropicOAuth ─────────────────────────────────────────────────────────

/// Anthropic OAuth provider (Claude Max).
///
/// Full beta headers including `oauth-2025-04-20` and `claude-code-20250219`.
/// Injects billing header, metadata, context_management, and cch body hash.
pub struct AnthropicOAuth {
    pub access_token: String,
    /// xxHash64 seed for cch body hash (from config).
    pub cch_seed: Option<u64>,
    /// Salt for cc_version suffix (from config).
    pub cc_version_salt: Option<String>,
    /// Persistent user identity hex string (from config).
    pub user_id: Option<String>,
    /// Account UUID from OAuth profile (from config).
    pub account_uuid: Option<String>,
    /// Per-session UUID (from Agent).
    pub session_id: String,
}

impl RequestCustomizer for AnthropicOAuth {
    fn label(&self) -> &str {
        "anthropic-oauth"
    }

    fn apply_chat_options(&self, options: ChatOptions, thinking_budget: u32) -> ChatOptions {
        let extra_body = build_extra_body(
            self.user_id.as_deref(),
            self.account_uuid.as_deref(),
            &self.session_id,
            false,
        );
        let options = options
            .with_extra_headers(oauth_headers(&self.access_token, OAUTH_BETA))
            .with_extra_body(extra_body)
            .with_capture_reasoning_content(true)
            .with_reasoning_effort(ReasoningEffort::Budget(thinking_budget));

        match self.cch_seed {
            Some(seed) => options.with_body_serializer(CchSerializer {
                seed,
                cc_version_salt: self.cc_version_salt.clone(),
            }),
            None => options,
        }
    }
}

// ─── AnthropicFastMode ──────────────────────────────────────────────────────

/// Anthropic fast mode provider (research preview).
///
/// OAuth + `research-preview-2026-02-01` beta flag. Uses adaptive thinking
/// instead of budgeted thinking. Injects billing header, metadata,
/// context_management, research_preview_2026_02, and cch body hash.
pub struct AnthropicFastMode {
    pub access_token: String,
    /// xxHash64 seed for cch body hash (from config).
    pub cch_seed: Option<u64>,
    /// Salt for cc_version suffix (from config).
    pub cc_version_salt: Option<String>,
    /// Persistent user identity hex string (from config).
    pub user_id: Option<String>,
    /// Account UUID from OAuth profile (from config).
    pub account_uuid: Option<String>,
    /// Per-session UUID (from Agent).
    pub session_id: String,
}

impl RequestCustomizer for AnthropicFastMode {
    fn label(&self) -> &str {
        "anthropic-fast"
    }

    fn apply_chat_options(&self, options: ChatOptions, _thinking_budget: u32) -> ChatOptions {
        let beta = format!("{},{}", OPUS_46_BETA, FAST_MODE_BETA);
        let extra_body = build_extra_body(
            self.user_id.as_deref(),
            self.account_uuid.as_deref(),
            &self.session_id,
            true,
        );
        let options = options
            .with_extra_headers(oauth_headers(&self.access_token, &beta))
            .with_extra_body(extra_body)
            .with_capture_reasoning_content(true)
            .with_reasoning_effort(ReasoningEffort::Adaptive);

        match self.cch_seed {
            Some(seed) => options.with_body_serializer(CchSerializer {
                seed,
                cc_version_salt: self.cc_version_salt.clone(),
            }),
            None => options,
        }
    }
}

// ─── Fast mode hash functions ───────────────────────────────────────────────

/// Compute the `cch` hash for a serialized request body.
///
/// The body should contain the `cch=00000` placeholder. Returns a
/// 5-character lowercase hex string (20-bit hash).
///
/// # Algorithm
///
/// `xxHash64(body_bytes, seed) & 0xFFFFF` → formatted as lowercase hex.
pub fn compute_cch(body: &[u8], seed: u64) -> String {
    let hash = xxh64(body, seed);
    format!("{:x}", hash & 0xFFFFF)
}

/// Compute the `cc_version` suffix (3-character hex string).
///
/// Picks characters at indices 4, 7, and 20 from the first user message,
/// concatenates with a salt and version string, then takes the first 3
/// characters of the SHA-256 hex digest.
///
/// # Arguments
///
/// * `first_user_message` - Text of the first user message in the conversation
/// * `salt` - Salt constant (from config)
/// * `version` - Version string (e.g. "2.1.37")
pub fn compute_cc_version_suffix(first_user_message: &str, salt: &str, version: &str) -> String {
    let chars: Vec<char> = first_user_message.chars().collect();
    let picked: String = [4, 7, 20]
        .iter()
        .map(|&i| chars.get(i).copied().unwrap_or('0'))
        .collect();
    let input = format!("{}{}{}", salt, picked, version);
    let hash = Sha256::digest(input.as_bytes());
    let hex = hex::encode(hash);
    hex[..3].to_string()
}

/// Build the billing header string with `cch=00000` placeholder.
///
/// This string is embedded as the first system prompt block in the
/// request body. The placeholder is later replaced by [`apply_cch`].
///
/// # Returns
///
/// A string like:
/// `x-anthropic-billing-header: cc_version=2.1.37.fbe; cc_entrypoint=cli; cch=00000;`
pub fn build_billing_header(first_user_message: &str, salt: &str, version: &str) -> String {
    let suffix = compute_cc_version_suffix(first_user_message, salt, version);
    format!(
        "x-anthropic-billing-header: cc_version={}.{}; cc_entrypoint=cli; cch=00000;",
        version, suffix
    )
}

/// Replace the `cch=00000` placeholder in a serialized request body
/// with the computed hash.
///
/// # Arguments
///
/// * `body` - The JSON-serialized request body containing `cch=00000`
/// * `seed` - The xxHash64 seed
///
/// # Returns
///
/// The body with `cch=00000` replaced by the computed hash value,
/// or the body unchanged if the placeholder isn't found.
pub fn apply_cch(body: &str, seed: u64) -> String {
    let hash = compute_cch(body.as_bytes(), seed);
    body.replacen("cch=00000", &format!("cch={}", hash), 1)
}

/// Build the `metadata.user_id` string.
///
/// Format: `user_{user_id}_account_{account_uuid}_session_{session_id}`
pub fn build_user_id(user_id: &str, account_uuid: &str, session_id: &str) -> String {
    format!(
        "user_{}_account_{}_session_{}",
        user_id, account_uuid, session_id
    )
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use genai::chat::ChatOptions;

    use super::*;

    // ─── AnthropicKey tests ───────────────────────────────────────────

    #[test]
    fn test_anthropic_key_label() {
        let provider = AnthropicKey;
        assert_eq!(provider.label(), "anthropic-key");
    }

    #[test]
    fn test_anthropic_key_options() {
        let provider = AnthropicKey;
        let options = provider.apply_chat_options(ChatOptions::default(), 2000);
        let _ = options; // Compiles and doesn't panic
    }

    // ─── AnthropicOAuth tests ─────────────────────────────────────────

    fn test_oauth_provider() -> AnthropicOAuth {
        AnthropicOAuth {
            access_token: "test-token".to_string(),
            cch_seed: Some(0x1234),
            cc_version_salt: Some("salt".to_string()),
            user_id: Some("user123".to_string()),
            account_uuid: Some("acct-456".to_string()),
            session_id: "sess-789".to_string(),
        }
    }

    #[test]
    fn test_anthropic_oauth_label() {
        let provider = test_oauth_provider();
        assert_eq!(provider.label(), "anthropic-oauth");
    }

    #[test]
    fn test_anthropic_oauth_options() {
        let provider = test_oauth_provider();
        let options = provider.apply_chat_options(ChatOptions::default(), 2000);
        let _ = options;
    }

    // ─── AnthropicFastMode tests ──────────────────────────────────────

    fn test_fast_provider() -> AnthropicFastMode {
        AnthropicFastMode {
            access_token: "fast-token".to_string(),
            cch_seed: Some(0x5678),
            cc_version_salt: Some("salt".to_string()),
            user_id: Some("user123".to_string()),
            account_uuid: Some("acct-456".to_string()),
            session_id: "sess-789".to_string(),
        }
    }

    #[test]
    fn test_anthropic_fast_mode_label() {
        let provider = test_fast_provider();
        assert_eq!(provider.label(), "anthropic-fast");
    }

    #[test]
    fn test_anthropic_fast_mode_options() {
        let provider = test_fast_provider();
        let options = provider.apply_chat_options(ChatOptions::default(), 2000);
        let _ = options;
    }

    // ─── Hash function tests ─────────────────────────────────────────

    #[test]
    fn test_compute_cch_deterministic() {
        let body = b"test body with cch=00000 placeholder";
        let seed = 0x1234567890abcdef_u64;
        let hash1 = compute_cch(body, seed);
        let hash2 = compute_cch(body, seed);
        assert_eq!(hash1, hash2);
        // Should be 1-5 hex chars (20-bit max = fffff)
        assert!(hash1.len() <= 5);
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_compute_cch_different_seeds() {
        let body = b"same body";
        let hash1 = compute_cch(body, 0);
        let hash2 = compute_cch(body, 1);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_compute_cch_different_bodies() {
        let seed = 42u64;
        let hash1 = compute_cch(b"body one", seed);
        let hash2 = compute_cch(b"body two", seed);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_cc_version_suffix_basic() {
        // "hello world" → chars at 4,7,20 → 'o','o','0' (index 20 missing)
        let suffix = compute_cc_version_suffix("hello world", "59cf53e54c78", "2.1.37");
        assert_eq!(suffix.len(), 3);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_cc_version_suffix_short_message() {
        // Very short message — indices 4,7,20 all missing → "000"
        let suffix = compute_cc_version_suffix("hi", "59cf53e54c78", "2.1.37");
        assert_eq!(suffix.len(), 3);
    }

    #[test]
    fn test_cc_version_suffix_deterministic() {
        let s1 = compute_cc_version_suffix("test message here", "salt", "1.0");
        let s2 = compute_cc_version_suffix("test message here", "salt", "1.0");
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_build_billing_header() {
        let header = build_billing_header("hello world", "59cf53e54c78", "2.1.37");
        assert!(header.starts_with("x-anthropic-billing-header:"));
        assert!(header.contains("cc_version=2.1.37."));
        assert!(header.contains("cc_entrypoint=cli"));
        assert!(header.contains("cch=00000"));
    }

    #[test]
    fn test_apply_cch() {
        let body = r#"{"system":"x-anthropic-billing-header: cch=00000;","messages":[]}"#;
        let result = apply_cch(body, 42);
        assert!(!result.contains("cch=00000"));
        assert!(result.contains("cch="));
    }

    #[test]
    fn test_apply_cch_no_placeholder() {
        let body = r#"{"messages":[]}"#;
        let result = apply_cch(body, 42);
        assert_eq!(result, body);
    }

    #[test]
    fn test_build_user_id() {
        let uid = build_user_id("abc123", "uuid-456", "session-789");
        assert_eq!(uid, "user_abc123_account_uuid-456_session_session-789");
    }

    /// Cross-check: verify cc_version suffix matches the Python PoC output.
    #[test]
    fn test_cc_version_suffix_oracle() {
        // Python: sha256("59cf53e54c78" + "oo0" + "2.1.37").hexdigest()[:3] == "1ad"
        let suffix = compute_cc_version_suffix("hello world", "59cf53e54c78", "2.1.37");
        assert_eq!(suffix, "1ad");
    }
}

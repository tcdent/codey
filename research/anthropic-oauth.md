# Anthropic OAuth for Claude Max

This document describes the OAuth flow required to use Claude Max subscriptions with the Anthropic API, reverse-engineered from OpenCode's implementation.

## Overview

Claude Max subscribers can use their subscription for API access via OAuth, avoiding separate API billing. This requires specific headers and request formatting that identify the client as "Claude Code".

## OAuth Flow

### 1. Authorization URL

Generate a PKCE challenge and redirect user to:

```
https://claude.ai/oauth/authorize?
  code=true&
  client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&
  response_type=code&
  redirect_uri=https://console.anthropic.com/oauth/code/callback&
  scope=org:create_api_key user:profile user:inference&
  code_challenge={PKCE_CHALLENGE}&
  code_challenge_method=S256&
  state={PKCE_VERIFIER}
```

The `client_id` is OpenCode's registered OAuth client ID.

### 2. Token Exchange

After user authorizes, they receive a code in format `{code}#{state}`. Exchange it:

```
POST https://console.anthropic.com/v1/oauth/token
Content-Type: application/json

{
  "code": "{code_part}",
  "state": "{state_part}",
  "grant_type": "authorization_code",
  "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
  "redirect_uri": "https://console.anthropic.com/oauth/code/callback",
  "code_verifier": "{pkce_verifier}"
}
```

Response contains `access_token`, `refresh_token`, and `expires_in`.

### 3. Token Refresh

```
POST https://console.anthropic.com/v1/oauth/token
Content-Type: application/json

{
  "grant_type": "refresh_token",
  "refresh_token": "{refresh_token}",
  "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
}
```

## API Request Requirements

### Headers

When using OAuth tokens, requests must include:

```
authorization: Bearer {access_token}
anthropic-version: 2023-06-01
anthropic-beta: oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14
user-agent: ai-sdk/anthropic/2.0.50 ai-sdk/provider-utils/3.0.18 runtime/bun/1.3.4
content-type: application/json
```

**Critical**: Do NOT include `x-api-key` header. The API rejects requests that have both `authorization` and `x-api-key`.

### Required Beta Headers

| Header | Purpose |
|--------|---------|
| `oauth-2025-04-20` | Enables OAuth authentication |
| `claude-code-20250219` | Identifies as Claude Code client |
| `interleaved-thinking-2025-05-14` | Enables extended thinking with tool use |
| `fine-grained-tool-streaming-2025-05-14` | Enables streaming tool results |

### System Prompt Structure

The API validates that requests come from Claude Code by checking the system prompt. The system prompt must be an array with the Claude Code identifier as the first block:

```json
{
  "system": [
    {
      "type": "text",
      "text": "You are Claude Code, Anthropic's official CLI for Claude.",
      "cache_control": {"type": "ephemeral"}
    },
    {
      "type": "text",
      "text": "Your actual system prompt here...",
      "cache_control": {"type": "ephemeral"}
    }
  ]
}
```

Without this identifier block, the API returns:
```json
{
  "type": "error",
  "error": {
    "type": "invalid_request_error",
    "message": "This credential is only authorized for use with Claude Code and cannot be used for other API requests."
  }
}
```

## Implementation Notes

### Token Storage

Tokens should be stored securely with restricted permissions:
- Access token (short-lived, ~1 hour)
- Refresh token (long-lived)
- Expiration timestamp

### PKCE Generation

```
verifier = base64url(random(32 bytes))
challenge = base64url(sha256(verifier))
```

### Error Handling

Common errors:
- `400` with "credit balance too low" - Account billing issue
- `400` with "only authorized for Claude Code" - Missing system prompt identifier or wrong headers
- `401` - Token expired, needs refresh

## References

- OpenCode anthropic-auth plugin: `opencode-anthropic-auth@0.0.5`
- OpenCode source: https://github.com/sst/opencode

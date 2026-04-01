#!/usr/bin/env python3
"""PoC: Claude Code fast mode with native cch hash — no Bun binary required.
Usage: uv run --with xxhash python3 poc_fast_mode.py
"""
import hashlib, json, os, subprocess, uuid, urllib.request, xxhash

CCH_SEED    = 0x6E52736AC806831E
VERSION     = "2.1.37"
SALT        = "59cf53e54c78"
API_URL     = "https://api.anthropic.com/v1/messages?beta=true"
PROMPT      = "Say 'hello' and nothing else."

# OAuth token from macOS keychain (same store Claude Code uses)
creds = json.loads(subprocess.check_output([
    "security", "find-generic-password",
    "-a", os.environ["USER"], "-s", "Claude Code-credentials", "-w"
], text=True).strip())
TOKEN = creds["claudeAiOauth"]["accessToken"]

# cc_version suffix: sha256(salt + 3 chars from user message + version)[:3]
chars = "".join(PROMPT[i] if i < len(PROMPT) else "0" for i in (4, 7, 20))
suffix = hashlib.sha256(f"{SALT}{chars}{VERSION}".encode()).hexdigest()[:3]

# Build body with cch=00000 placeholder
body = json.dumps({
    "model": "claude-opus-4-6",
    "max_tokens": 32000,
    "stream": False,
    "thinking": {"type": "adaptive"},
    "research_preview_2026_02": "active",
    "metadata": {"user_id": f"user_poc_session_{uuid.uuid4()}"},
    "system": [{
        "type": "text",
        "text": f"x-anthropic-billing-header: cc_version={VERSION}.{suffix}; cc_entrypoint=cli; cch=00000;"
    }],
    "messages": [{"role": "user", "content": PROMPT}],
}, separators=(",", ":"))

# Compute cch: xxHash64(body_with_placeholder, seed) & 0xFFFFF
cch = format(xxhash.xxh64(body.encode(), seed=CCH_SEED).intdigest() & 0xFFFFF, "05x")
body = body.replace("cch=00000", f"cch={cch}")

print(f"cch={cch}  cc_version={VERSION}.{suffix}")

# Send request
req = urllib.request.Request(API_URL, data=body.encode(), method="POST", headers={
    "Content-Type": "application/json",
    "Authorization": f"Bearer {TOKEN}",
    "User-Agent": "claude-cli/2.1.37 (external, cli)",
    "anthropic-version": "2023-06-01",
    "anthropic-beta": "claude-code-20250219,oauth-2025-04-20,adaptive-thinking-2026-01-28,research-preview-2026-02-01",
    "x-app": "cli",
})

try:
    resp = urllib.request.urlopen(req)
    data = json.loads(resp.read())
    print(f"HTTP {resp.status} — model={data['model']}, tokens={data['usage']}")
    for block in data.get("content", []):
        if block["type"] == "text":
            print(f"\n{block['text']}")
except urllib.error.HTTPError as e:
    print(f"HTTP {e.code}: {e.read().decode()[:500]}")

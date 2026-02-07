# Browser Sessions PR #64 — Review Findings

## Must Fix

- [x] **No close_all() on app exit** — Chrome processes orphaned on quit → added `browser_sessions.close_all().await` before `restore_terminal()`
- [x] **Temp dir collision** — all sessions use same `codey-browser-session-{pid}`, second session overwrites first profile copy → added atomic counter: `codey-browser-{pid}-{n}`
- [x] **Field naming** — `_handler_task` and `_temp_dir` have leading underscore (Rust "unused" convention) but are used in close() → renamed to `handler_task` and `temp_dir`

## Should Fix

- [x] **page_load_wait_ms unused** — config option still exists but sessions used hardcoded NETWORK_SETTLE_MS → now uses config value via `settle_ms()` helper
- [ ] **Lock held during cleanup_expired()** — calls browser.close().await while holding mutex; hung Chrome blocks all sessions

## Nice to Have

- [ ] **Read-only tools could auto-approve** — browser_list_sessions and browser_snapshot are read-only
- [ ] **Move docs/browser-sessions.md to research/** — consistency with other research docs

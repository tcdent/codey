-- Atomically check if preview slot is available and claim it
-- Returns true if successfully claimed, false if another instance owns it
-- Args: channel_id (number) - the claiming instance's channel ID

local channel_id = ...

-- Check if preview is currently in use
local preview_tab = vim.g.codey_preview_tab
local preview_owner = vim.g.codey_preview_owner

-- If there's an active preview tab that's still valid, slot is taken
if preview_tab and vim.api.nvim_tabpage_is_valid(preview_tab) then
    return false
end

-- If another instance has claimed but not yet shown preview, slot is taken
-- (unless it's us re-claiming, which is fine)
if preview_owner and preview_owner ~= channel_id then
    return false
end

-- Slot is available - claim it atomically
vim.g.codey_preview_owner = channel_id
return true

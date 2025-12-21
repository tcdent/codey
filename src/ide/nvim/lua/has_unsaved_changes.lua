-- Check if a file has unsaved changes
-- Args: target_path (string)

local target_path = ...
for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    local name = vim.api.nvim_buf_get_name(buf)
    if name == target_path and vim.bo[buf].modified then
        return true
    end
end
return false

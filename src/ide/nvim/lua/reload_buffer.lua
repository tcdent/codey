-- Reload buffer by file path
-- Args: target_path (string)

local target_path = ...
for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    local name = vim.api.nvim_buf_get_name(buf)
    if name == target_path then
        vim.api.nvim_buf_call(buf, function()
            vim.cmd('checktime')
        end)
    end
end

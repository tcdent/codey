-- Navigate to a file and position
-- Args: path (string), line (number), col (number)

local path, line, col = ...

-- Expand to absolute path if relative
if not path:match('^/') then
    local cwd = vim.fn.getcwd()
    path = cwd .. '/' .. path
end

-- Normalize the path (resolve . and ..)
path = vim.fn.fnamemodify(path, ':p')

vim.cmd('edit ' .. vim.fn.fnameescape(path))

if line and line > 0 then
    local line_count = vim.api.nvim_buf_line_count(0)
    line = math.min(line, line_count)
    col = col or 1
    vim.api.nvim_win_set_cursor(0, {line, math.max(0, col - 1)})
end

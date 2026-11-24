-- A to-do list that survives restarts, with the current git branch of a folder.
-- Copy this file into Ripple's plugins folder to try it. PLUGINS.md explains the API.
local REPO = "~/code"

return {
  name = "To-do",
  persist = true,
  state = { draft = "", items = {} },
  view = function(s)
    local rows = {
      gap = 8,
      ui.row {
        ui.icon("terminal", { size = 14 }),
        ui.text(s.branch or "branch?", { size = 12, dim = true }),
        ui.spacer(),
        ui.button("Refresh", function()
          ripple.run({ "git", "-C", REPO:gsub("^~", os.getenv("HOME") or os.getenv("USERPROFILE") or ""), "branch", "--show-current" },
            function(s, out) s.branch = out end)
        end),
      },
      ui.input("draft", {
        value = s.draft,
        placeholder = "Add a task",
        on_change = function(s, text) s.draft = text end,
        on_submit = function(s, text)
          if text ~= "" then table.insert(s.items, { title = text, done = false }) end
          s.draft = ""
        end,
      }),
    }
    for i, item in ipairs(s.items) do
      table.insert(rows, ui.toggle(item.title, item.done, function(s) s.items[i].done = not s.items[i].done end, { key = "item-" .. i }))
    end
    local done = 0
    for _, item in ipairs(s.items) do if item.done then done = done + 1 end end
    if done > 0 then
      table.insert(rows, ui.button("Clear done", function(s)
        local left = {}
        for _, item in ipairs(s.items) do if not item.done then table.insert(left, item) end end
        s.items = left
        ripple.notify(done == 1 and "1 task done" or done .. " tasks done")
      end))
    end
    return ui.col(rows)
  end,
}

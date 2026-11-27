-- A focus timer: 25 minutes of work, then a 5 minute break.
-- Copy this file into Ripple's plugins folder to try it. PLUGINS.md explains the API.
local WORK, BREAK = 25 * 60, 5 * 60

local function left(s)
  if s.ends then return math.max(0, s.ends - os.time()) end
  return s.paused or s.length
end

return {
  name = "Focus Timer",
  state = { length = WORK },
  -- Runs once a second; it catches up on phases that ended while the Island slept.
  tick = function(s)
    while s.ends and os.time() >= s.ends do
      s.length = s.length == WORK and BREAK or WORK
      s.ends = s.ends + s.length
    end
  end,
  -- While the timer runs, it shows on the compact Island beside the time.
  live = function(s)
    if not s.ends then return nil end
    local l = left(s)
    return {
      icon = "timer",
      label = string.format("%02d:%02d", l // 60, l % 60),
      color = s.length == WORK and "orange" or "green",
      progress = 1 - l / s.length,
      spoken = string.format("%s, %d minutes left", s.length == WORK and "Focus" or "Break", math.ceil(l / 60)),
    }
  end,
  view = function(s)
    local l = left(s)
    return ui.col {
      gap = 10,
      ui.text(s.length == WORK and "Focus" or "Break", { dim = true }),
      ui.text(string.format("%02d:%02d", l // 60, l % 60), { size = 40, weight = 700 }),
      ui.bar(1 - l / s.length),
      ui.row {
        s.ends and ui.button("Pause", function(s) s.paused, s.ends = left(s), nil end, { key = "start" })
          or ui.button("Start", function(s) s.ends, s.paused = os.time() + left(s), nil end, { key = "start" }),
        ui.button("Reset", function(s) s.length, s.ends, s.paused = WORK, nil, nil end),
      },
    }
  end,
}

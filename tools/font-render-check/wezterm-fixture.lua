local wezterm = require 'wezterm'

return {
  font = wezterm.font('JetBrainsMono Nerd Font'),
  font_size = 12.0,
  initial_cols = 72,
  initial_rows = 12,
  enable_tab_bar = false,
  window_decorations = 'RESIZE',
  colors = {
    foreground = '#f2f2f2',
    background = '#101010',
    cursor_bg = '#f2f2f2',
    cursor_fg = '#101010',
  },
}

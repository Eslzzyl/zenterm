# 2026-07-17/18 修改记录

## 背景

用户需要让一个 TUI Coding Agent 工具 tidev (~/WorkSpace/Rust/tidev) 在 zenterm 上正确工作。

### tidev 如何检测图片协议

`ratatui-image` 的 `Picker::from_query_stdio()` 向 PTY 写查询序列，从 stdin 读回复：

1. Kitty APC query — 检测 Kitty 图形协议支持
2. CSI 16 t — 获取字体像素尺寸
3. DSR — 检测终端在线（确保不 hang）

Picker 的协议选择优先级：

```
IO 检测结果（Kitty/Sixel）
  .or(环境变量 tmux 检测)
  .or(环境变量 Iterm2 检测)    ← iterm2_from_env()
  .unwrap_or(Halfblocks)         ← 兜底
```

### 关键环境变量

| 变量                   | 来源           | 对 Picker 的影响                                               |
| ---------------------- | -------------- | -------------------------------------------------------------- |
| `WEZTERM_EXECUTABLE`   | 继承自 WezTerm | `picker.rs:122-129`: **黑名单 Kitty + Sixel**，不发送 APC 查询 |
| `TERM_PROGRAM=WezTerm` | 继承自 WezTerm | `picker.rs:327-339` (`iterm2_from_env()`): 回退到 Iterm2 协议  |

用户在外层 WezTerm 中运行 zenterm，这两个变量被子进程继承。

---

## Kitty 图片协议实现

### Unicode 占位符（U=1）

zenterm 实现了 Kitty 协议的 Unicode 占位符模式（`U=1`），这是 `ratatui-image` 默认使用的图片放置方式。

**实现要点**：

- DIACRITICS 数组包含 Kitty 规范定义的 297 个组合变音符号，用于编码占位符的行列位置
- 图片 ID 编码在前景色（TrueColor `38;2;R;G;B`）和第3个组合变音符号中，重建公式为 `(id_extra << 24) | (r << 16) | (g << 8) | b`
- 同一行内后续占位符字符继承首字符的 row/id_extra，列号自动递增
- 占位符渲染分两遍：第一遍扫描 grid 收集位置信息，第二遍创建 ImageCell

**渲染流程**：

1. ratatui-image 发送 `a=T,U=1,f=32` 的 APC 分块传输图片数据
2. zenterm 解码并存储到 image cache
3. ratatui-image 写入 U+10EEEE 占位符字符到终端 grid
4. `visible_cells()` 扫描 grid 中的 U+10EEEE，从 fg 颜色和变音符号还原 image_id + 行列
5. 为每个占位符计算 UV 坐标，创建 ImageCell 存入 grid_cache
6. GPU 渲染 ImageCell 为纹理四边形

### 性能优化

- APC/DCS 扫描器使用 `memchr`（SIMD）搜索 ST 终止符，替代 `windows(2).position()`
- 累加器采用流式解码：每个 chunk 到达时立即 base64 解码追加到 buffer，不再存储中间 String
- `scan_oscs` 主循环使用 `memchr` 搜索 ESC 字节

### 环境配置

在 zenterm 中启动 tidev，需要清理继承的 `WEZTERM_EXECUTABLE`：

```sh
WEZTERM_EXECUTABLE= tidev
```

或写一个别名/包装脚本。`TERM_PROGRAM=zenterm` 由 zenterm 自动设置。

---

## 已知问题

### 1. 可折叠工具卡片点击无效

**现象**: Session 内 AI 返回的工具结果卡片（可折叠/展开）点击无反应。

**已确认**:

- 图片 badge 点击有效 → 左键 SGR 事件已转发到 tidev
- 工具卡片与图片 badge 走不同的点击命中逻辑（`selectable_region` vs `image_badge_bounds`）
- 两种点击最终都经过 `chat/mod.rs:769` 的 `handle_mouse_click()`

未调查根因。

### 2. Hover 无效

**已确认**:

- `tab_viewer.rs:99`: `Sense::click_and_drag()` 不含 hover sense
- `handle_mouse`（`mouse.rs:66`）仅在 egui 调用此回调时执行，纯移动事件不触发
- `compute_hover` 每帧执行但不发送 SGR 事件
- motion 转发代码在 `handle_mouse` 末尾，若 `handle_mouse` 不被调用则不执行

未修复。

# Refactoring Plan & Lessons Learned

## 现状

| Crate | 文件数 | 总行数 | 最大文件 | 备注 |
|-------|--------|--------|---------|------|
| zenterm | 1 | 110 | 110 | 二进制入口，不动 |
| zenterm-core | 8 | 668 | 235 | 已拆分良好 |
| zenterm-config | 11 | 1,357 | 335 | 已拆分良好 |
| zenterm-pty | 1 | 221 | 221 | 单文件，不动 |
| zenterm-input | 1 | 176 | 176 | 单文件，不动 |
| **zenterm-term** | 2 | 854 | **842** | ⚠️ term.rs 需拆分 |
| **zenterm-glyph** | 2 | 1,755 | **1,320** | ⚠️ lib.rs 需拆分 |
| **zenterm-render** | 3 | 911 | **510** | ⚠️ lib.rs 需拆分 |
| **zenterm-ui** | 12 | 5,729 | **1,431** | ⚠️ app.rs 需拆分 |
| **已拆分** session/ | 13 | 2,199 | 416 | ✅ 完成 |

---

## 待拆分文件

每个拆分前必须做：`git show HEAD:path/to/file > /tmp/original.rs`，逐行对照，禁止重写。

### 1. zenterm-glyph/src/lib.rs (1,320 行)

**问题：** `impl GlyphAtlas` 占 1,031 行，全部在 lib.rs 里。

**方案：**
```
src/
├── lib.rs           类型定义 (GlyphContentType, GlyphEntry, RunCacheKey, ShapedGlyph, GlyphAtlas struct)
├── atlas_impl.rs    GlyphAtlas 核心方法 (new, get_or_shape, cache management)
├── rasterize.rs     swash 光栅化 (rasterize_swash, layout_and_rasterize)
├── allocate.rs      etagere 纹理分配
└── builtin.rs       (保留，435 行)
```

### 2. zenterm-term/src/term.rs (842 行)

**问题：** 5 个逻辑单元 (TermDimensions, EventListener, ColorScheme, GridView, Terminal) 加 OSC 7 扫描混在一个文件。

**方案：**
```
src/term/
├── mod.rs           pub use
├── terminal.rs      Terminal struct + 核心方法 (new, feed, resize, scroll, selection)
├── color_scheme.rs  ColorScheme
├── grid_view.rs     GridView + CursorInfo
├── listener.rs      EventListener
├── effects.rs       take_title, take_bell, take_exit, take_clipboard, etc.
├── resolve.rs       resolve_cell, resolve_color
└── osc7.rs          scan_osc7
```

### 3. zenterm-render/src/lib.rs (510 行)

**问题：** WGSL 着色器字符串内联在 Rust 代码中 (~140 行)。

**方案：**
```
src/
├── lib.rs           CellInstance + TerminalRenderPass (保留 ~370 行)
└── shaders.rs       WGSL 着色器常量 (~140 行)
```
或把 WGSL 放 `.wgsl` 文件用 `include_str!` 嵌入。

### 4. zenterm-ui/src/app.rs (1,431 行)

**问题：** 巨型的 `impl ZentermApp` 块包含 22 个方法，`handle_shortcuts` 202 行，`render_tabs_with_dock` 298 行。

**方案：**
```
src/app/
├── mod.rs              ZentermApp struct + new_with_wgpu + eframe::App impl
├── session_lifecycle   spawn_session, close_session, focus_tab
├── keyboard.rs         forward_event, feed_keyboard, handle_shortcuts
├── config.rs           apply_new_config, reload_config, maybe_save_config
├── persistence.rs      maybe_persist_layout, persist_layout_now
├── settings.rs         render_settings_viewport
├── dock.rs             render_tabs_with_dock
└── theme.rs            sync_theme + color helpers
```

---

## 从 session.rs 拆分中学到的教训

### 教训 1：永远不要"重写"，只能"搬运"

**错的：**
```rust
// 我以为这段逻辑很简单，就自己写了一遍
fn handle_mouse(&mut self, ...) {
    let pos = match ui.ctx().pointer_interact_pos() { ... };
    let col = (rel_x / self.cell_width) as usize;
    // 缺少 ppp 乘法，缺少 pixel_to_cell_clamped，选取触发时机不对
}
```

**对的：**
```rust
// 从原文一字不差复制，只改模块路径和编译必需的机械替换
git show HEAD:./path/to/original.rs > /tmp/orig.rs
// 逐行对比
```

### 教训 2：任何一个"等价"的判断都必须验证，不能靠假设

我说 Copy 按钮没差异时没去查原文，说边缘拖拽应该工作时没去查 egui 的 `interact_pointer_pos()` 行为。两次都错了。

### 教训 3：函数提取和逻辑变更不能在同一次提交里做

我应该先做纯拆分（把代码原样移到新文件），编译通过后，再单独做逻辑修正。混在一起导致 bug 无法定位是"提取引入的"还是"原本就有的"。

### 教训 4：大函数拆分前先看懂每条分支

`update_cell_instances` 的连字分支有完全独立的 UV 计算逻辑（`origin_to_bitmap`、垂直/水平裁切、颜色处理），我没读懂就提取了，结果写了一套完全不同的算法。

### 教训 5：拆分后 diff 原文

拆分完应该做 `diff -u <提取后的文件> <原文对应段落>` 确认没有意外差异。这次 mouse.rs 的 ALT_SCREEN 缺失就是应该被 diff 发现的。

---

## 下次拆分的执行步骤

```
1. git show HEAD 取原文到 /tmp/
2. 创建目标目录结构
3. 复制原文到新文件，只改：
   a. 模块路径 (use super::xxx / use crate::xxx)
   b. 可见性 (pub(crate) if needed)
   c. 闭包参数改为直接传值 (如 px_to_clip 闭包 → x_scale/y_scale)
4. cargo check --workspace
5. diff -u 原文 新文件（排除 import 差异）
6. git commit（纯拆分，零逻辑变更）
7. 如需要逻辑修正，单独提交并说明原因
```

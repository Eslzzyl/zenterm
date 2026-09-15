# Glyph Quad Clipping

## 问题

swash 光栅化 glyph 时，位图的包围盒（`placement.top` + `placement.height`）
可能超出 cell 边界。原因：

1. **包围盒基于矢量轮廓**：swash 遍历所有轮廓点（包括 Bézier 控制点），
   控制点可以超出实际曲线，导致包围盒偏大
2. **像素对齐的 floor/ceil**：包围盒对齐到像素网格时，上下各可能多出 1px
3. **OS/2 度量与实际轮廓不一致**：cosmic-text 的 `max_descent` 来自字体的
   OS/2 表（排版建议值），而 swash 的位图高度来自实际轮廓像素范围。
   个别字符的轮廓可以超出排版度量

这导致 GLYPH quad 的 `clip_cell_size` 超出 cell，shader 在字形外填充的
`bg_color` 会溢出到相邻 cell 区域，产生可见的视觉伪影；如果直接把 quad
裁剪到 cell 边界，超出顶部的字形笔画也会被截断。

## 行高与 fallback

终端网格采用配置主字体的固定 cell 尺寸。`GlyphAtlas::measure_baseline` 只用
主字体的 `Mg` 测量 ascent/descent；CJK、日文、韩文等 fallback 字体不会把自己
的度量传播到所有行，否则某个脚本就会把整个终端的行高撑大。

`cosmic-text::LayoutGlyph.font_id` 会保留每个 glyph 实际选中的字体。swash 光栅化
后，如果该 fallback glyph 的 bitmap 包围盒超出主字体 cell，就先按安全比例降低
font size 并重新光栅化，以 baseline 为中心保持 bearing 关系；重新光栅化后的取整或
hinting 残差才交给渲染时的 scale 处理。缩放只作为异常 fallback 的安全适配，主字体
glyph 保持原始尺寸。fallback glyph
从光栅化阶段就使用灰度 mask，避免不同字体的物理子像素与采样位置错位；若后续
对异常字形执行几何缩放，也不会重新引入 LCD coverage。这样可以保留固定行高，
同时避免常见 fallback 字体的中文顶部被裁掉；渲染层的 cell 裁剪继续作为未预探测
异常字形的最后安全边界。

## 解决方案

在 CPU 端构建 instance 数据时，将 GLYPH quad 裁剪到 cell 边界内，
同时同步调整 UV 坐标以避免纹理拉伸。fallback glyph 会在进入渲染层前按固定 cell
约束，因此正常情况下不会触发顶部裁剪；该裁剪只处理未被适配的异常字形。

代码位于 `crates/zenterm-ui/src/session/render/mod.rs`，非连字 glyph 渲染路径中：

```rust
// 垂直裁剪
let glyph_bot_px = glyph_y_px + scaled_h;
let clipped_top = glyph_y_px.max(cell_top);
let clipped_bot = glyph_bot_px.min(cell_bottom);
let clipped_h = (clipped_bot - clipped_top).max(0.0);
if clipped_h < scaled_h && scaled_h > 0.0 {
    let r_top = (clipped_top - glyph_y_px) / scaled_h;
    let r_bot = (clipped_bot - glyph_y_px) / scaled_h;
    let v_range = v_max - v_min;
    v_min = v_min + v_range * r_top;
    v_max = v_min + (r_bot - r_top) * v_range;
    glyph_y_px = clipped_top;
    scaled_h = clipped_h;
}

// 水平裁剪（同理）
let clipped_left = glyph_x_px.max(cell_left);
let clipped_right = glyph_right_px.min(cell_right);
```

裁剪对所有 glyph 生效（不只是光标），确保任何字符的位图 padding
都不会溢出到相邻 cell。

## 宽字符（CJK / Emoji）处理

全角字符占据两个 cell。裁剪边界通过检查下一列的 `is_spacer` 标志
来确定字符宽度：

```rust
let num_cells: f32 = if col + 1 < cols {
    grid.cell(row, col + 1)
        .map_or(1.0, |c| if c.is_spacer { 2.0 } else { 1.0 })
} else {
    1.0
};

let cell_right = cell_left + cw * num_cells;  // 半角: +cw, 全角: +2*cw
```

背景 quad 同样使用 `num_cells` 确保全角字符的背景覆盖完整的两列。

## 与其他终端的对比

| | 裁剪 | 策略 |
|--|------|------|
| Alacritty | 无裁剪 | cell 足够大 + 画家算法覆盖溢出 |
| WezTerm | 无裁剪 | 同上 |
| zenterm | **CPU 端裁剪** | clip quad + 调整 UV |

Alacritty/WezTerm 依赖 cell 高度（来自字体真实行高）足够容纳 glyph，
且下一行的渲染自然覆盖溢出。zenterm 选择在 CPU 端显式裁剪，
确保 GLYPH quad 严格不超出 cell 边界。

## 性能影响

可忽略。裁剪逻辑是每个 glyph 几次浮点比较和加减法，发生在 CPU 端
instance 构建阶段（非热路径）。对于完全在 cell 内的 glyph，
`if` 分支不进入。GPU 端无变化。

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
`bg_color` 会溢出到相邻 cell 区域，产生可见的视觉伪影。

## 解决方案

在 CPU 端构建 instance 数据时，将 GLYPH quad 裁剪到 cell 边界内，
同时同步调整 UV 坐标以避免纹理拉伸。

代码位于 `crates/zenterm-ui/src/app.rs`，glyph 渲染路径中：

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
    v_max = v_min + v_range * (r_bot - r_top);
    glyph_y_px = clipped_top;
    scaled_h = clipped_h;
}

// 水平裁剪（同理）
```

裁剪对所有 glyph 生效（不只是光标），确保任何字符的位图 padding
都不会溢出到相邻 cell。

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

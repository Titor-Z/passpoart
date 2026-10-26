//! 设计 token：聿Canvas 项目 `tokens.css` 在代码里的唯一落地处。
//!
//! **唯一数值来源**——组件里不许出现裸 hex、裸字号，一律引这里，
//! 否则改了设计稿而界面不动，两边会慢慢漂成两套设计。
//! 亮色 = tokens.css 的 `:root`；暗色 = `.dark` 覆盖层（只换底/玻璃/文本/描边/光斑），
//! 品牌与分类的**色相家族两套一致**，暗色仅提升明度保对比。

use gpui::{BoxShadow, Rgba};

const fn c(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba::new(r, g, b, a)
}

/// 分量按 0..255 写 → 0..1（与设计稿整数一一对应，便于逐条核对）。
const fn rgb8(r: u8, g: u8, b: u8) -> Rgba {
    c(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

/// 八位十六进制 `0xRRGGBBAA`（与 CSS 里写的值直接对应）。
const fn argb(hex: u32) -> Rgba {
    let a = (hex & 0xFF) as f32 / 255.0;
    let r = ((hex >> 24) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let b = ((hex >> 8) & 0xFF) as f32 / 255.0;
    c(r, g, b, a)
}

/// gpui 阴影的 blur 换算：shader 把 `blur_radius` 直接当高斯 σ 用（macOS shaders.metal
/// `gaussian(x, sigma)`），而 CSS `box-shadow` 的 blur 半径 = 2σ（css-backgrounds 规范）。
/// 画板写的是 CSS 值，这里统一除 2，否则阴影弥散范围是设计稿的两倍（暗色下成一片黑晕）。
fn css_blur(blur: f32) -> gpui::Pixels {
    gpui::px(blur / 2.0)
}

/// 外阴影：CSS `x y blur color`。
fn sh(x: f32, y: f32, blur: f32, color: Rgba) -> BoxShadow {
    BoxShadow {
        color: gpui::rgb_to_hsla(color),
        offset: gpui::point(gpui::px(x), gpui::px(y)),
        blur_radius: css_blur(blur),
        spread_radius: gpui::px(0.0),
        inset: false,
    }
}

/// 内阴影：CSS `inset x y blur color`（凹）。
fn sh_in(x: f32, y: f32, blur: f32, color: Rgba) -> BoxShadow {
    BoxShadow {
        color: gpui::rgb_to_hsla(color),
        offset: gpui::point(gpui::px(x), gpui::px(y)),
        blur_radius: css_blur(blur),
        spread_radius: gpui::px(0.0),
        inset: true,
    }
}

/// 扩散环：CSS `0 0 0 spread color`（聚焦光环）。
fn sh_ring(spread: f32, color: Rgba) -> BoxShadow {
    BoxShadow {
        color: gpui::rgb_to_hsla(color),
        offset: gpui::point(gpui::px(0.0), gpui::px(0.0)),
        blur_radius: gpui::px(0.0),
        spread_radius: gpui::px(spread),
        inset: false,
    }
}

// gpui 的 `bg`/`text_color`/`border_color` 吃 `Hsla`，统一在这一个出口换算。

// ---------------------------------------------------------------- 尺寸 token
// 「小样」的窗口形态 = 画板「密码库 · 列表态」：420×640 固定小窗。

pub const WINDOW_W: f32 = 420.0;
pub const WINDOW_H: f32 = 640.0;
pub const TITLEBAR_H: f32 = 44.0;

/// 圆角：控件 10 / 卡内 14（对应 --radius-sm/md）。
/// 窗口本身圆角不在此：交由系统遮罩（macOS/Win11 圆角，Win10/Linux 直角），
/// 内容层直角铺满以免与系统圆角半径不一致产生四角透明缺口。
pub const RADIUS_SM: f32 = 10.0;
pub const RADIUS_MD: f32 = 14.0;

// 字阶（--font-size-*）：caption 12 / label 13 / body 14 / title 16。
pub const BRAND_NAME: &str = "Passpoart";

// ---------------------------------------------------------------- 亮色（:root）
mod light {
    use super::*;
    pub const WINDOW_BG: Rgba = argb(0xFAF4F8_F5); // --color-background 96%：画板本为实色 #FAF4F8，保留微玻璃感但不让暗桌布洗灰画面
    pub const CARD: Rgba = argb(0xFFFFFF_99); // --color-card 0.60
    pub const CARD_STRONG: Rgba = argb(0xFFFFFF_CC); // --color-card-strong 0.80
    pub const CARD_SUNKEN: Rgba = argb(0xFCF9FD_80); // --color-card-sunken 0.50
    pub const BORDER: Rgba = argb(0xFFFFFF_B3); // --color-border 0.70
    pub const BORDER_SOFT: Rgba = argb(0xFFFFFF_66); // 0.40
    pub const TEXT: Rgba = rgb8(0x42, 0x33, 0x50);
    pub const TEXT_2: Rgba = rgb8(0x6C, 0x5D, 0x7A);
    pub const MUTED: Rgba = rgb8(0x9B, 0x8C, 0xA6);
    pub const PRIMARY: Rgba = rgb8(0xE4, 0x73, 0x9D);
    pub const PRIMARY_STRONG: Rgba = rgb8(0xD4, 0x5E, 0x8C);
    pub const PRIMARY_SOFT: Rgba = argb(0xE4739D_29); // 0.16
    pub const MINT: Rgba = rgb8(0x58, 0xB9, 0x8C);
    pub const LAVENDER: Rgba = rgb8(0x9B, 0x7F, 0xDF);
    pub const PEACH: Rgba = rgb8(0xE9, 0x94, 0x5F);
    pub const BUTTER: Rgba = rgb8(0xE0, 0xB5, 0x4A);
    pub const SKY: Rgba = rgb8(0x6B, 0xA6, 0xDE);
    pub const MINT_SOFT: Rgba = argb(0x8CD8B4_38);
    pub const LAVENDER_SOFT: Rgba = argb(0xBAA6EE_3D);
    pub const PEACH_SOFT: Rgba = argb(0xF7BE8E_42);
    pub const BUTTER_SOFT: Rgba = argb(0xF6DE92_4D);
    pub const SKY_SOFT: Rgba = argb(0x96C8F0_42);
    pub const SUCCESS: Rgba = rgb8(0x3F, 0xA4, 0x7E);
    pub const WARNING: Rgba = rgb8(0xDE, 0x9A, 0x44);
    pub const DANGER: Rgba = rgb8(0xDB, 0x54, 0x61);
    pub const ROW_ALT: Rgba = argb(0xFFFFFF_66); // 0.40
    pub const HOVER: Rgba = argb(0xFFFFFF_80); // 0.50

    // shadow（tokens.css --shadow-* 色名同位；亮色画板原本为冷紫灰 #5E406E，
    // 与粉白底 #FAF4F8 色相冲突，gpui 渲染下出现「粉里杂灰」发浑观感；
    // 换为同饱和度暖调品暗色 #7A5E70，阴影融入背景同色相、不引入冷调灰色。
    // alpha 仍按 RZ0022+后续调低补偿 gpui 与 CSS 渲染偏重。）
    pub const SHADOW_RAISED_C: Rgba = argb(0x5E406E_1A); // 画板原值 rgba(94,64,110,0.10)——冷紫灰 + 暖白底是设计者刻意对比，配 inset-highlight 白光拓揠道；上一轮改暖色相 + 动 alpha 是误归因
    pub const SHADOW_SUNKEN_C: Rgba = argb(0x5E406E_1A); // 画板原值 0.10
    pub const SHADOW_HIGHLIGHT_C: Rgba = argb(0xFFFFFF_BF); // inset 顶部高光 rgba(255,255,255,0.75)——新拟物“阳阴”另一面，不可丢
    pub const SHADOW_KNOB_C: Rgba = argb(0x5E406E_4D); // 画板原值 0.30
}

// ---------------------------------------------------------------- 暗色（.dark）
mod dark {
    use super::*;
    pub const WINDOW_BG: Rgba = argb(0x1E1826_F2); // --color-background 95%：暗色半透明过低会被亮桌面洗浅（v0.3.3 修正）
    pub const CARD: Rgba = argb(0x2C243A_8C); // --color-card 0.55
    pub const CARD_STRONG: Rgba = argb(0x322A40_B8); // --color-card-strong 0.72
    pub const CARD_SUNKEN: Rgba = argb(0x1C1626_80); // 0.50
    pub const BORDER: Rgba = argb(0xFFFFFF_1F); // 0.12
    pub const BORDER_SOFT: Rgba = argb(0xFFFFFF_12); // 0.07
    pub const TEXT: Rgba = rgb8(0xF2, 0xEC, 0xF7);
    pub const TEXT_2: Rgba = rgb8(0xBF, 0xB2, 0xCE);
    pub const MUTED: Rgba = rgb8(0x8E, 0x80, 0xA0);
    pub const PRIMARY: Rgba = rgb8(0xDE, 0x6F, 0x9E);
    pub const PRIMARY_STRONG: Rgba = rgb8(0xC8, 0x5E, 0x8C);
    pub const PRIMARY_SOFT: Rgba = argb(0xF08BB0_29);
    pub const MINT: Rgba = rgb8(0x6F, 0xCB, 0xA0);
    pub const LAVENDER: Rgba = rgb8(0xB2, 0x9B, 0xEE);
    pub const PEACH: Rgba = rgb8(0xF2, 0xA8, 0x73);
    pub const BUTTER: Rgba = rgb8(0xE8, 0xC3, 0x66);
    pub const SKY: Rgba = rgb8(0x7F, 0xB6, 0xE6);
    pub const MINT_SOFT: Rgba = argb(0x6FCBA0_2E);
    pub const LAVENDER_SOFT: Rgba = argb(0xB29BEE_33);
    pub const PEACH_SOFT: Rgba = argb(0xF2A873_33);
    pub const BUTTER_SOFT: Rgba = argb(0xE8C366_38);
    pub const SKY_SOFT: Rgba = argb(0x7FB6E6_33);
    pub const SUCCESS: Rgba = rgb8(0x5B, 0xC4, 0x96);
    pub const WARNING: Rgba = rgb8(0xE8, 0xAC, 0x5E);
    pub const DANGER: Rgba = rgb8(0xE8, 0x70, 0x87);
    pub const ROW_ALT: Rgba = argb(0xFFFFFF_0A); // 0.04
    pub const HOVER: Rgba = argb(0xFFFFFF_0F); // 0.06

    // shadow：暗夜换纯黑影、顶部高光降到 10%（tokens.css .dark）
    pub const SHADOW_RAISED_C: Rgba = argb(0x000000_4D); // rgba(0,0,0,0.30)
    pub const SHADOW_SUNKEN_C: Rgba = argb(0x000000_40); // rgba(0,0,0,0.25)
    pub const SHADOW_HIGHLIGHT_C: Rgba = argb(0xFFFFFF_1A); // rgba(255,255,255,0.10)
}

/// 运行时主题：字段名与 tokens.css 令牌一一对应。
pub struct Theme {
    pub window_bg: Rgba,
    pub card: Rgba,
    pub card_strong: Rgba,
    pub card_sunken: Rgba,
    pub border: Rgba,
    pub border_soft: Rgba,
    pub text: Rgba,
    pub text_2: Rgba,
    pub muted: Rgba,
    pub primary: Rgba,
    pub primary_strong: Rgba,
    pub primary_soft: Rgba,
    pub mint: Rgba,
    pub lavender: Rgba,
    pub peach: Rgba,
    pub butter: Rgba,
    pub sky: Rgba,
    pub mint_soft: Rgba,
    pub lavender_soft: Rgba,
    pub peach_soft: Rgba,
    pub butter_soft: Rgba,
    pub sky_soft: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
    pub row_alt: Rgba,
    pub hover: Rgba,
    pub on_primary: Rgba,
    // 新拟物阴影（画板 --shadow-* 体系：卡片/井凹、控件凸、顶部高光）
    pub sh_raised: BoxShadow,    // 0 2px 8px 凸
    pub sh_sunken: BoxShadow,    // inset 0 2px 6px 凹
    pub sh_highlight: BoxShadow, // inset 0 1px 0 顶部高光
    pub sh_knob: BoxShadow,      // 0 1px 3px 开关旋钮
    pub sh_glow: BoxShadow,      // 0 6px 16px 主按钮粉晕（两主题同值）
    pub sh_ring: BoxShadow,      // 0 0 0 4px 聚焦光环（primary-soft）
}

impl Theme {
    pub fn light() -> Self {
        use light::*;
        Self {
            window_bg: WINDOW_BG,
            card: CARD,
            card_strong: CARD_STRONG,
            card_sunken: CARD_SUNKEN,
            border: BORDER,
            border_soft: BORDER_SOFT,
            text: TEXT,
            text_2: TEXT_2,
            muted: MUTED,
            primary: PRIMARY,
            primary_strong: PRIMARY_STRONG,
            primary_soft: PRIMARY_SOFT,
            mint: MINT,
            lavender: LAVENDER,
            peach: PEACH,
            butter: BUTTER,
            sky: SKY,
            mint_soft: MINT_SOFT,
            lavender_soft: LAVENDER_SOFT,
            peach_soft: PEACH_SOFT,
            butter_soft: BUTTER_SOFT,
            sky_soft: SKY_SOFT,
            success: SUCCESS,
            warning: WARNING,
            danger: DANGER,
            row_alt: ROW_ALT,
            hover: HOVER,
            on_primary: rgb8(0xFF, 0xFF, 0xFF),
            sh_raised: sh(0.0, 2.0, 8.0, SHADOW_RAISED_C),
            sh_sunken: sh_in(0.0, 2.0, 6.0, SHADOW_SUNKEN_C),
            sh_highlight: sh_in(0.0, 1.0, 0.0, SHADOW_HIGHLIGHT_C),
            sh_knob: sh(0.0, 1.0, 3.0, SHADOW_KNOB_C),
            sh_glow: sh(0.0, 6.0, 16.0, argb(0xE4739D_61)), // 画板原值 38%（之前为了不洗粉压到 24%，现在阴影视角走暖调不再需要压 alpha）
            sh_ring: sh_ring(4.0, PRIMARY_SOFT),
        }
    }

    pub fn dark() -> Self {
        use dark::*;
        Self {
            window_bg: WINDOW_BG,
            card: CARD,
            card_strong: CARD_STRONG,
            card_sunken: CARD_SUNKEN,
            border: BORDER,
            border_soft: BORDER_SOFT,
            text: TEXT,
            text_2: TEXT_2,
            muted: MUTED,
            primary: PRIMARY,
            primary_strong: PRIMARY_STRONG,
            primary_soft: PRIMARY_SOFT,
            mint: MINT,
            lavender: LAVENDER,
            peach: PEACH,
            butter: BUTTER,
            sky: SKY,
            mint_soft: MINT_SOFT,
            lavender_soft: LAVENDER_SOFT,
            peach_soft: PEACH_SOFT,
            butter_soft: BUTTER_SOFT,
            sky_soft: SKY_SOFT,
            success: SUCCESS,
            warning: WARNING,
            danger: DANGER,
            row_alt: ROW_ALT,
            hover: HOVER,
            on_primary: rgb8(0xFF, 0xFF, 0xFF),
            sh_raised: sh(0.0, 2.0, 8.0, SHADOW_RAISED_C),
            sh_sunken: sh_in(0.0, 2.0, 6.0, SHADOW_SUNKEN_C),
            sh_highlight: sh_in(0.0, 1.0, 0.0, SHADOW_HIGHLIGHT_C),
            sh_knob: sh(0.0, 1.0, 3.0, argb(0x5E406E_4D)), // 画板暗色未覆盖旋钮影；暗背景需足够重才可见，保持 30%
            sh_glow: sh(0.0, 6.0, 16.0, argb(0xE4739D_61)), // 画板暗色未覆盖粉晕，沿用
            sh_ring: sh_ring(4.0, PRIMARY_SOFT),
        }
    }
}

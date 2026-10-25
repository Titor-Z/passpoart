//! 输入井配色：由宿主应用注入（组件库不持全局主题，避免反向依赖）。

/// 单个输入井需要的外观参数（颜色 + 尺寸 + 阴影）。
#[derive(Clone)]
pub struct WellStyle {
    /// 井底色。
    pub bg: gpui::Rgba,
    /// 未聚焦描边。
    pub border: gpui::Rgba,
    /// 聚焦描边（主色高不透明度）。
    pub border_focused: gpui::Rgba,
    /// 正文字色。
    pub text: gpui::Rgba,
    /// 占位/提示字色。
    pub muted: gpui::Rgba,
    /// 光标色。
    pub caret: gpui::Rgba,
    /// 选区高亮底色。
    pub selection: gpui::Rgba,
    /// 井内小徽标底色（Ctrl F / 字符数等，随主题）。
    pub chip_bg: gpui::Rgba,
    /// 井内小徽标描边。
    pub chip_border: gpui::Rgba,
    /// 井阴影（画板 --shadow-sunken：凹）。
    pub shadow: gpui::BoxShadow,
    /// 聚焦时叠加的光环（画板 well.focus：0 0 0 4px primary-soft）。
    pub focus_ring: gpui::BoxShadow,
    /// 井高（px）。
    pub height: f32,
    /// 字号（px）。
    pub font_size: f32,
}

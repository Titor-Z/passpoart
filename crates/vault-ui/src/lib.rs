//! vault-ui：Passpoart 自维护的 gpui-ce 输入组件库。
//!
//! gpui-ce 不公开文本输入组件（Zed 的编辑器太重，不随库分发），
//! 本 crate 提供三个积木：
//! - [`Field`]：单行编辑缓冲（文本 + 光标 + 选区，char 语义）；
//! - [`TextInput`]：单行输入框组件（`masked()` 即密码框），键盘/点击/剪贴板/光标自持；
//! - [`TextArea`]：多行输入框（Enter 换行、上下行移动、按行点击定位）。
//!
//! 组件是独立 Entity：宿主用 `cx.new` 创建、`.child(entity)` 嵌入，
//! Enter/Tab/Esc 通过回调交宿主编排；配色经 [`WellStyle`] 注入，不持全局主题。

mod field;
mod text_input;
mod text_area;
mod well_style;

pub use field::{field_key, ClipOp, Field, FieldResult};
pub use text_area::TextArea;
pub use text_input::TextInput;
pub use well_style::WellStyle;

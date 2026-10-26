//! 主窗口：AppRoot 状态机（解锁 / 列表 / 详情）+ 三屏渲染。
//!
//! 布局逐条对应聿Canvas 画板 `unlock.html` / `vault-home.html` / `vault-full.html`：
//! 420×640 固定小窗、自绘标题栏、推入态导航（详情盖过列表，返回箭头退回）。

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, rgba, size, App, Bounds, Context, Div, Entity, ElementId,
    FontWeight, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    SharedString, Stateful, Styled, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowOptions,
};

use vault_core::keymgr::{
    derive_local_verifier, derive_master_key, generate_salt, KdfParams, UnlockedVault,
};
use vault_core::storage::{
    open_item_from_storage, seal_item_for_storage, ItemStore, NodbStore,
};

use crate::data::{self, Filter, Item, Kind};
use vault_ui::{TextArea, TextInput, WellStyle};

/// 由应用主题生成输入井外观（尺寸随调用点）。
fn well_style(t: &Theme, height: f32, font_size: f32) -> WellStyle {
    WellStyle {
        bg: t.card_sunken,
        border: t.border_soft,
        border_focused: rgba(0xE4739D_73), // rgba(228,115,157,0.45)（画板 well.focus）
        text: t.text,
        muted: t.muted,
        caret: t.primary,
        selection: t.primary_soft,
        chip_bg: t.card,
        chip_border: t.border_soft,
        shadow: t.sh_sunken.clone(),
        focus_ring: t.sh_ring.clone(),
        height,
        font_size,
    }
}
use crate::theme::{self, Theme};

/// 屏幕：一条单向流 Unlock → List ⇄ Detail / Edit；Create = 首启建库。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Unlock,
    Create,
    List,
    Detail,
    Edit,
    Settings,
}

/// 应用配置：~/.passpoart/config.json，最佳努力读写（损坏即回落默认值）。
fn default_autolock_minutes() -> u64 {
    10
}

fn default_true() -> bool {
    true
}

fn default_pw_reveal_secs() -> u64 {
    20
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AppConfig {
    /// true = 关闭按钮最小化窗口；false/缺省 = 直接退出。
    #[serde(default)]
    close_minimizes: bool,
    /// 无操作自动锁定（分钟）；缺省 10。
    #[serde(default = "default_autolock_minutes")]
    autolock_minutes: u64,
    /// 复制密码后 30 秒自动清空剪贴板；缺省开。
    #[serde(default = "default_true")]
    clip_autoclear: bool,
    /// 点眼睛明文显示密码后自动掩码的秒数；0 = 常亮。缺省 20。
    #[serde(default = "default_pw_reveal_secs")]
    pw_reveal_secs: u64,
}

/// 配置文件路径（与默认库同目录 ~/.passpoart/）。
fn config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".passpoart/config.json"))
        .unwrap_or_else(|| PathBuf::from("config.json"))
}

pub struct AppRoot {
    screen: Screen,
    dark: bool,
    filter: Filter,
    /// 当前选中条目（列表态高亮 + 详情态内容）；`None` = 未选中。
    selected: Option<usize>,
    /// 详情态里「复制密码」是否已点（按钮切换为已复制反馈）。
    copied: bool,
    /// 详情底部状态栏消息：`(是否成功, 文案)`，1.8s 自动消失。复制 / 打开网址都走这里。
    status: Option<(bool, SharedString)>,
    /// 状态代号防误清：新状态代号 +1，旧 timer 醒来发现代号变了就不清。
    status_gen: u64,
    // —— 真实解锁（TDdf7f1ac7）——
    /// 库文件路径：PASSPOART_DB 环境变量 > ~/.passpoart/vault.nodb。
    db_path: PathBuf,
    /// 库文件是否存在（解锁屏状态点；回解锁屏时重探）。
    vault_exists: bool,
    /// 主密码输入框组件（掩码；锁定即清空，只存在于内存）。
    password: Entity<TextInput>,
    /// 搜索输入框组件（只过滤内存快照，不落盘）。
    search_input: Entity<TextInput>,
    /// 剪贴板清空代号：新复制使代号 +1，旧定时器醒来发现代号变了就不清（防误清新内容）。
    clip_gen: u64,
    /// 眼睛按钮：圆点/明文切换。
    show_pw: bool,
    /// 点眼睛明文显示后自动掩码秒数（0=常亮），来自设置。
    pw_reveal_secs: u64,
    /// 明文自动掩码代号：新一次明文显示使代号 +1，旧定时器醒来发现代号变了就不动（防误遮）。
    pw_reveal_gen: u64,
    /// Argon2id 派生进行中（按钮转「派生中」，防重入）。
    unlocking: bool,
    /// 解锁失败/引导提示（输入框下红字）。
    unlock_error: Option<String>,
    /// 打开的库：句柄常驻到退出（redb 文件锁独占，RZ0012），锁定只清 vault。
    opened: Option<OpenedVault>,
    /// 解密后的条目快照（锁定即清空）。
    items: Vec<Item>,
    /// 编辑表单（Some = 处于编辑态；新建与编辑共用）。
    edit: Option<EditForm>,
    /// 建库表单（Some = 处于建库态）。
    create: Option<CreateForm>,
    /// 建库进行中（Argon2id 后台派生，防重入）。
    creating: bool,
    /// 删除二次确认：首击变「确认删除？」，再击才执行。
    confirm_delete: bool,
    /// 最近一次用户活动（键盘/滚轮/点击统一打点）；自动锁定巡检用。
    last_activity: std::time::Instant,
    /// 自动锁定已发生：解锁屏显示横幅，任何解锁尝试清除。
    auto_locked: bool,
    /// 关闭按钮行为：true=最小化窗口；false=直接退出（~/.passpoart/config.json）。
    close_minimizes: bool,
    /// 无操作自动锁定（分钟，可配置）。
    autolock_minutes: u64,
    /// 复制密码后 30 秒自动清空剪贴板（可关闭）。
    clip_autoclear: bool,
    /// 输入光标闪烁相位（时钟每 500ms 翻转；聚焦井尾部画 2px 竖条）。
    caret_on: bool,
}

/// 打开的库会话：存储句柄 + 解锁上下文。
struct OpenedVault {
    store: NodbStore,
    /// None = 锁定态（句柄保留，密钥已清零）。
    vault: Option<UnlockedVault>,
}

/// 编辑表单：新建（editing_id=None，version=1）与编辑（version=旧版+1）共用。
/// 各字段自绘输入井（focus handle + 键盘捕获），不引第三方输入组件。
struct EditForm {
    /// 编辑中的条目 id；None = 新建（id 由存储层生成）。
    editing_id: Option<String>,
    /// 本次保存要写入的版本号（新建=1；编辑=旧版+1）。
    editing_version: u32,
    kind: Kind,
    title: Entity<TextInput>,
    url: Entity<TextInput>,
    user: Entity<TextInput>,
    secret: Entity<TextInput>,
    totp: Entity<TextInput>,
    note: Entity<TextArea>,
    starred: bool,
    /// 保存失败提示（表单顶部红字）。
    error: Option<String>,
}

impl EditForm {
    /// 空表单（新建）：六字段组件 + Tab 链 + Enter 保存 / Esc 取消接线。
    fn empty(cx: &mut Context<AppRoot>, dark: bool) -> Self {
        let t = if dark { Theme::dark() } else { Theme::light() };
        let s = well_style(&t, 38.0, 13.0);
        let n = well_style(&t, 84.0, 13.0);
        let weak = cx.entity().downgrade();

        // 通用单行字段：Enter=保存，Esc=取消编辑
        let mk = |cx: &mut Context<AppRoot>, ph: &'static str, s: WellStyle| {
            let w1 = weak.clone();
            let w2 = weak.clone();
            cx.new(|cx| {
                TextInput::new(cx, ph, s)
                    .on_enter(move |_, cx| {
                        let w = w1.clone();
                        cx.spawn(async move |cx| {
                            let _ = w.update(cx, |app, cx| app.save_edit(cx));
                        })
                        .detach();
                    })
                    .on_escape(move |_, cx| {
                        let w = w2.clone();
                        cx.spawn(async move |cx| {
                            let _ = w.update(cx, |app, cx| {
                                app.edit = None;
                                app.screen = Screen::List;
                                cx.notify();
                            });
                        })
                        .detach();
                    })
            })
        };

        let title = mk(cx, "如 GitHub", s.clone());
        let url = mk(cx, "example.com/login", s.clone());
        let user = mk(cx, "you@example.com", s.clone());
        let totp = mk(cx, "JBSWY3DPEHPK3PXP", s.clone());

        // 密码井：后置骰子（生成 16 位密码）+ 字符数徽标
        let w1 = weak.clone();
        let w2 = weak.clone();
        let secret = {
            let s = s.clone();
            cx.new(|cx| {
                TextInput::new(cx, "留空则不设密码", s)
                    .on_enter(move |_, cx| {
                        let w = w1.clone();
                        cx.spawn(async move |cx| {
                            let _ = w.update(cx, |app, cx| app.save_edit(cx));
                        })
                        .detach();
                    })
                    .on_escape(move |_, cx| {
                        let w = w2.clone();
                        cx.spawn(async move |cx| {
                            let _ = w.update(cx, |app, cx| {
                                app.edit = None;
                                app.screen = Screen::List;
                                cx.notify();
                            });
                        })
                        .detach();
                    })
                    .trailing(|tcx, field, s| {
                        let len = field.as_str().chars().count();
                        let mut row = div().flex().flex_row().items_center().gap(px(4.0));
                        if len > 0 {
                            row = row.child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(6.0))
                                    .bg(s.chip_bg)
                                    .border_1()
                                    .border_color(s.chip_border)
                                    .text_size(px(9.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(s.muted)
                                    .child(format!("{len} 字符")),
                            );
                        }
                        row.child(
                            div()
                                .id("btn-dice")
                                .w(px(24.0))
                                .h(px(24.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(7.0))
                                .cursor_pointer()
                                .on_click(tcx.listener(|this, _, _, cx| {
                                    this.set_text(
                                        &vault_core::generator::generate_password(16),
                                        cx,
                                    );
                                }))
                                .child(
                                    gpui::svg()
                                        .path("icons/dice.svg")
                                        .w(px(13.0))
                                        .h(px(13.0))
                                        .text_color(s.muted),
                                ),
                        )
                        .into_any_element()
                    })
            })
        };

        // 备注（多行）：Esc=取消编辑；Enter 默认换行
        let w3 = weak.clone();
        let note = {
            let n = n.clone();
            cx.new(|cx| {
                TextArea::new(cx, "自由备注…", n).on_escape(move |_, cx| {
                    let w = w3.clone();
                    cx.spawn(async move |cx| {
                        let _ = w.update(cx, |app, cx| {
                            app.edit = None;
                            app.screen = Screen::List;
                            cx.notify();
                        });
                    })
                    .detach();
                })
            })
        };

        // Tab 链：名称→网址→用户名→密码→TOTP→备注
        let singles = [&title, &url, &user, &secret, &totp];
        let mut handles: Vec<gpui::FocusHandle> = singles
            .iter()
            .map(|e| e.read(cx).focus_handle())
            .collect();
        handles.push(note.read(cx).focus_handle());
        for (i, e) in singles.iter().enumerate() {
            let next = handles[i + 1].clone();
            e.update(cx, |input, _| {
                input.set_on_tab(move |window, cx| window.focus(&next, cx));
            });
        }

        Self {
            editing_id: None,
            editing_version: 1,
            kind: Kind::Login,
            title,
            url,
            user,
            secret,
            totp,
            note,
            starred: false,
            error: None,
        }
    }

    /// 从库里的权威明文填充（不从视图快照回填——快照的 sub 是有损投影）。
    fn from_core(
        core: &vault_core::model::Item,
        next_version: u32,
        dark: bool,
        cx: &mut Context<AppRoot>,
    ) -> Self {
        let mut form = Self::empty(cx, dark);
        form.editing_id = Some(core.id.clone());
        form.editing_version = next_version;
        form.kind = Kind::from_core(core.kind);
        form.title.update(cx, |i, cx| i.set_text(&core.title, cx));
        form.url.update(cx, |i, cx| i.set_text(&core.url, cx));
        form.user.update(cx, |i, cx| i.set_text(&core.subtitle, cx));
        form.secret.update(cx, |i, cx| i.set_text(&core.secret, cx));
        form.totp.update(cx, |i, cx| {
            i.set_text(core.totp_secret.as_deref().unwrap_or(""), cx)
        });
        form.note.update(cx, |i, cx| i.set_text(&core.note, cx));
        form.starred = core.favorite;
        form
    }
}

/// 建库表单：两次主密码（组件，掩码）+ 警示。
struct CreateForm {
    pw: Entity<TextInput>,
    pw2: Entity<TextInput>,
    error: Option<String>,
}

impl CreateForm {
    fn new(cx: &mut Context<AppRoot>, dark: bool) -> Self {
        let t = if dark { Theme::dark() } else { Theme::light() };
        let s = well_style(&t, 44.0, 15.0);
        let weak = cx.entity().downgrade();
        let mk = |cx: &mut Context<AppRoot>, ph: &'static str, s: WellStyle| {
            let w1 = weak.clone();
            let w2 = weak.clone();
            cx.new(|cx| {
                TextInput::new(cx, ph, s)
                    .masked()
                    .on_enter(move |_, cx| {
                        let w = w1.clone();
                        cx.spawn(async move |cx| {
                            let _ = w.update(cx, |app, cx| app.start_create(cx));
                        })
                        .detach();
                    })
                    .on_escape(move |_, cx| {
                        let w = w2.clone();
                        cx.spawn(async move |cx| {
                            let _ = w.update(cx, |app, cx| {
                                app.create = None;
                                app.screen = Screen::Unlock;
                                cx.notify();
                            });
                        })
                        .detach();
                    })
            })
        };
        let pw = mk(cx, "主密码", s.clone());
        let pw2 = mk(cx, "再输入一次", s.clone());
        // Tab：第一个 → 第二个
        let next = pw2.read(cx).focus_handle();
        pw.update(cx, |input, _| {
            input.set_on_tab(move |window, cx| window.focus(&next, cx));
        });
        Self { pw, pw2, error: None }
    }
}

impl AppRoot {
    /// 主密码输入框：掩码 + 眼睛切换（后置槽）+ Enter 解锁 / Esc 清空。
    fn new_password_input(cx: &mut Context<Self>) -> Entity<TextInput> {
        let style = well_style(&Theme::light(), 46.0, 17.0);
        let w_enter = cx.entity().downgrade();
        let w_escape = cx.entity().downgrade();
        let w_eye = cx.entity().downgrade();
        cx.new(|cx| {
            TextInput::new(cx, "主密码", style)
                .masked()
                .on_enter(move |_, cx| {
                    // 回调跑在 TextInput 的 update 借用内——经 spawn 推迟到
                    // 监听器返回后执行，否则 start_unlock 读回本实体即 BorrowMut panic
                    let w = w_enter.clone();
                    cx.spawn(async move |cx| {
                        let _ = w.update(cx, |app, cx| app.start_unlock(cx));
                    })
                    .detach();
                })
                .on_escape(move |_, cx| {
                    let w = w_escape.clone();
                    cx.spawn(async move |cx| {
                        let _ = w.update(cx, |app, cx| {
                            app.password.update(cx, |input, cx| input.clear(cx));
                            app.unlock_error = None;
                            cx.notify();
                        });
                    })
                    .detach();
                })
                .trailing(move |_tcx, _field, s| {
                    // 眼睛按钮：切换 AppRoot.show_pw + 组件掩码（工厂每帧调用，内部克隆弱引用）
                    let w = w_eye.clone();
                    div()
                        .id("pw-eye")
                        .w(px(24.0))
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            let _ = w.update(cx, |app, cx| {
                                app.show_pw = !app.show_pw;
                                let masked = !app.show_pw;
                                app.password.update(cx, |input, cx| {
                                    input.set_masked(masked, cx)
                                });
                                cx.notify();
                            });
                        })
                        .child(
                            gpui::svg()
                                .path("icons/eye.svg")
                                .w(px(13.0))
                                .h(px(13.0))
                                .text_color(s.muted),
                        )
                        .into_any_element()
                })
        })
    }

    /// 搜索输入框：前置搜索图标 + 后置清除按钮与 Ctrl F 徽标。
    fn new_search_input(cx: &mut Context<Self>) -> Entity<TextInput> {
        let style = well_style(&Theme::light(), 38.0, 12.0);
        cx.new(|cx| {
            TextInput::new(cx, "搜索…", style)
                .leading(|_tcx, _field, s| {
                    div()
                        .w(px(14.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .child(
                            gpui::svg()
                                .path("icons/search.svg")
                                .w(px(14.0))
                                .h(px(14.0))
                                .text_color(s.muted),
                        )
                        .into_any_element()
                })
                .trailing(|tcx, field, s| {
                    // 清除按钮（仅在有内容时显示）；缓冲由参数传入（不得重读自身实体）
                    let empty = field.is_empty();
                    let mut row = div().flex().flex_row().items_center().gap(px(4.0));
                    if !empty {
                        row = row.child(
                            div()
                                .id("btn-clear-search")
                                .w(px(18.0))
                                .h(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .on_click(tcx.listener(|this, _, _, cx| this.clear(cx)))
                                .child(
                                    gpui::svg()
                                        .path("icons/close.svg")
                                        .w(px(9.0))
                                        .h(px(9.0))
                                        .text_color(s.muted),
                                ),
                        );
                    }
                    row.child(
                        div()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(6.0))
                            .bg(s.chip_bg)
                            .border_1()
                            .border_color(s.chip_border)
                            .text_size(px(9.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(s.muted)
                            .child("Ctrl F"),
                    )
                    .into_any_element()
                })
        })
    }

    /// 主题切换：重刷全部存活输入组件的外观。
    fn restyle_inputs(&mut self, cx: &mut Context<Self>) {
        let t = if self.dark { Theme::dark() } else { Theme::light() };
        self.password
            .update(cx, |i, cx| i.restyle(well_style(&t, 46.0, 17.0), cx));
        self.search_input
            .update(cx, |i, cx| i.restyle(well_style(&t, 38.0, 12.0), cx));
        if let Some(form) = &self.edit {
            let s = well_style(&t, 38.0, 13.0);
            let n = well_style(&t, 84.0, 13.0);
            form.title.update(cx, |i, cx| i.restyle(s.clone(), cx));
            form.url.update(cx, |i, cx| i.restyle(s.clone(), cx));
            form.user.update(cx, |i, cx| i.restyle(s.clone(), cx));
            form.secret.update(cx, |i, cx| i.restyle(s.clone(), cx));
            form.totp.update(cx, |i, cx| i.restyle(s, cx));
            form.note.update(cx, |i, cx| i.restyle(n, cx));
        }
        if let Some(form) = &self.create {
            let s = well_style(&t, 44.0, 15.0);
            form.pw.update(cx, |i, cx| i.restyle(s.clone(), cx));
            form.pw2.update(cx, |i, cx| i.restyle(s, cx));
        }
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let db_path = std::env::var_os("PASSPOART_DB")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".passpoart/vault.nodb"))
            })
            .unwrap_or_else(|| PathBuf::from("vault.nodb"));
        let vault_exists = db_path.exists();
        let config: AppConfig = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(AppConfig {
                close_minimizes: false,
                autolock_minutes: default_autolock_minutes(),
                clip_autoclear: true,
                pw_reveal_secs: default_pw_reveal_secs(),
            });
        let root = Self {
            screen: Screen::Unlock,
            dark: false,
            filter: Filter::All,
            selected: None,
            copied: false,
            status: None,
            status_gen: 0,
            db_path,
            vault_exists,
            password: Self::new_password_input(cx),
            search_input: Self::new_search_input(cx),
            clip_gen: 0,
            show_pw: false,
            pw_reveal_gen: 0,
            unlocking: false,
            unlock_error: None,
            opened: None,
            items: Vec::new(),
            edit: None,
            create: None,
            creating: false,
            confirm_delete: false,
            last_activity: std::time::Instant::now(),
            auto_locked: false,
            close_minimizes: config.close_minimizes,
            autolock_minutes: config.autolock_minutes,
            clip_autoclear: config.clip_autoclear,
            pw_reveal_secs: config.pw_reveal_secs,
            caret_on: true,
        };
        root.start_clock(cx);
        root
    }

    /// 每秒一跳的时钟：驱动 TOTP 倒计时/动态码重绘（420×640 小窗整帧重绘开销可忽略）。
    fn start_clock(&self, cx: &Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.caret_on = !this.caret_on; // 输入光标闪烁
                        // 自动锁定：解锁后的三屏，无操作达配置分钟数即清密钥回解锁屏
                        let autolock_after =
                            std::time::Duration::from_secs(this.autolock_minutes * 60);
                        if this.last_activity.elapsed() >= autolock_after
                            && matches!(this.screen, Screen::List | Screen::Detail | Screen::Edit)
                        {
                            this.do_lock(true, cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return; // 实体已释放（窗口关闭）
                }
            }
        })
        .detach();
    }

    /// 记录一次用户活动（键盘/滚轮/点击统一入口）。
    fn touch(&mut self) {
        self.last_activity = std::time::Instant::now();
    }

    /// 配置落盘（最佳努力：失败不影响运行）。
    fn save_config(&self) {
        let cfg = AppConfig {
            close_minimizes: self.close_minimizes,
            autolock_minutes: self.autolock_minutes,
            clip_autoclear: self.clip_autoclear,
            pw_reveal_secs: self.pw_reveal_secs,
        };
        let path = config_path();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        if let Ok(json) = serde_json::to_string_pretty(&cfg) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// 锁定：清密钥（Drop 清零）与明文快照，回解锁屏；auto=true 显示自动锁定横幅。
    fn do_lock(&mut self, auto: bool, cx: &mut Context<Self>) {
        if let Some(opened) = self.opened.as_mut() {
            opened.vault = None;
        }
        self.items.clear();
        self.password.update(cx, |i, cx| {
            i.set_masked(true, cx);
            i.clear(cx);
        });
        self.show_pw = false;
        self.search_input.update(cx, |i, cx| i.clear(cx));
        self.copied = false;
        self.show_pw = false;
        self.unlock_error = None;
        self.edit = None;
        self.create = None;
        self.confirm_delete = false;
        self.vault_exists = self.db_path.exists();
        self.auto_locked = auto;
        self.screen = Screen::Unlock;
    }
}

/// 打开主窗口：系统模糊背板 + 透明系统标题栏（自绘标题栏成为唯一一条）。
pub fn open(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(theme::WINDOW_W), px(theme::WINDOW_H)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // 薄玻璃：macOS = NSVisualEffectView，Windows = DWM 组合属性。
        window_background: WindowBackgroundAppearance::Blurred,
        is_resizable: false,
        titlebar: Some(TitlebarOptions {
            title: Some(theme::BRAND_NAME.into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        ..Default::default()
    };
    let _ = cx.open_window(options, |window, cx| {
        crate::platform::hide_system_window_buttons(window);
        let root = cx.new(AppRoot::new);
        // 开窗即聚焦主密码框：直接可打字
        let focus = root.read(cx).password.read(cx).focus_handle();
        window.focus(&focus, cx);
        root
    });
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = match self.dark {
            false => Theme::light(),
            true => Theme::dark(),
        };
        let content = match self.screen {
            Screen::Unlock => self.unlock_view(&t, window, cx),
            Screen::Create => self.create_view(&t, window, cx),
            Screen::Settings => self.settings_view(&t, cx),
            Screen::List => self.list_view(&t, window, cx),
            Screen::Detail => self.detail_view(&t, cx),
            Screen::Edit => self.edit_view(&t, window, cx),
        };

        // 窗框：圆角 + 一根描边画在内容层（系统只管模糊背板）。
        div()
            .id("root")
            .flex()
            .flex_col()
            .w(px(theme::WINDOW_W))
            .h(px(theme::WINDOW_H))
            .overflow_hidden()
            .bg(t.window_bg)
            .child(content)
            // 遮住整窗的 1px 内描边（不占布局、不挡子元素事件）。
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .w(px(theme::WINDOW_W))
                    .h(px(theme::WINDOW_H))
                    .border_1()
                    .border_color(t.border),
            )
    }
}

// ================================================================ 小零件

/// 品牌方点：小样用纯主色近似设计稿的玫瑰→薰衣草渐变，内放一枚白盾。
fn brand_logo(t: &Theme) -> impl IntoElement {
    div()
        .w(px(20.0))
        .h(px(20.0))
        .rounded(px(6.0))
        // 画板 logo-mini：primary→lavender 对角渐变 + 凸影
        .bg(gpui::linear_gradient(
            135.0,
            gpui::linear_color_stop(t.primary, 0.0),
            gpui::linear_color_stop(t.lavender, 1.0),
        ))
        .shadow(vec![t.sh_raised.clone()])
        .flex()
        .items_center()
        .justify_center()
        .child(
            gpui::svg()
                .path("icons/shield.svg")
                .w(px(12.0))
                .h(px(12.0))
                .text_color(rgba(0xFFFFFF_FF)),
        )
}

/// 标题栏拖动区（ macOS 靠 start_window_move 拖窗）。
fn drag_area(child: impl IntoElement) -> Stateful<Div> {
    div()
        .id("titlebar-drag")
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.start_window_move();
        })
        .child(child)
}

/// 自绘窗控：只留关闭 ✕（紧凑小窗不需要最小化/最大化）。
/// 行为由设置决定：直接退出 / 最小化窗口。
fn win_dots(t: &Theme, cx: &Context<AppRoot>) -> impl IntoElement {
    let ink = t.muted;
    div()
        .flex()
        .flex_row()
        .items_center()
        .px(px(2.0))
        .child(
            div()
                .id("win-close")
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.close_minimizes {
                        window.minimize_window();
                    } else {
                        cx.quit();
                    }
                }))
                .child(
                    gpui::svg()
                        .path("icons/close.svg")
                        .w(px(13.0))
                        .h(px(13.0))
                        .text_color(ink),
                ),
        )
}

/// 图标按钮（可交互版，走 listener 由调用方传行为）。
fn icon_btn(
    id: &'static str,
    path: &'static str,
    ink: gpui::Rgba,
    hover_bg: gpui::Rgba,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(
            gpui::svg()
                .path(path)
                .w(px(14.0))
                .h(px(14.0))
                .text_color(ink),
        )
}

/// 分类 → 色对（图标底 soft / 墨色），对应 design.md 的分类色语义。
fn kind_colors(t: &Theme, kind: Kind) -> (gpui::Rgba, gpui::Rgba) {
    match kind {
        Kind::Login => (t.primary_soft, t.primary_strong),
        Kind::Note => (t.mint_soft, t.mint),
        Kind::Card => (t.peach_soft, t.peach),
        Kind::Identity => (t.lavender_soft, t.lavender),
    }
}

/// 条目 favicon：soft 底圆角方块 + 名字首字（真实站点图标随同步任务接入）。
fn favicon(t: &Theme, item: &Item, side: f32, text: f32) -> impl IntoElement {
    let (bg, ink) = kind_colors(t, item.kind);
    div()
        .w(px(side))
        .h(px(side))
        .rounded(px(side * 11.0 / 34.0))
        .bg(bg)
        // 画板 .favicon：顶部高光内阴影
        .shadow(vec![t.sh_highlight.clone()])
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(text))
        .font_weight(FontWeight::BOLD)
        .text_color(ink)
        .child(SharedString::from(data::first_char(&item.title)))
}

/// 胶囊小标签（文本可为运行时生成，如 TOTP 动态码）。
fn pill(bg: gpui::Rgba, ink: gpui::Rgba, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .px(px(9.0))
        .py(px(2.0))
        .rounded_full()
        .bg(bg)
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .child(text.into())
}

/// 密码强度四段条。
fn strength_bar(t: &Theme, level: u8) -> impl IntoElement {
    let ink = match level {
        0..=1 => t.danger,
        2 => t.warning,
        3 => t.primary,
        _ => t.success,
    };
    let label = match level {
        0..=1 => "弱",
        2 => "中",
        3 => "强",
        _ => "极强",
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(
            div().flex().flex_row().gap(px(2.0)).children((0..4).map(|i| {
                div()
                    .w(px(12.0))
                    .h(px(3.0))
                    .rounded(px(2.0))
                    .bg(if i < level { ink } else { t.border_soft })
            })),
        )
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink)
                .child(label),
        )
}

/// 详情字段行：标签 + 值 + 右侧工具位。
fn field_row(
    t: &Theme,
    label: &'static str,
    value: impl IntoElement,
    tools: Option<impl IntoElement>,
) -> Div {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .px(px(12.0))
        .py(px(9.0))
        .rounded(px(theme::RADIUS_SM))
        .bg(t.card_sunken)
        .border_1()
        .border_color(t.border_soft)
        .child(
            div()
                .w(px(64.0))
                .flex_none()
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted)
                .child(label),
        )
        .child(div().flex_1().min_w(px(0.0)).child(value));
    if let Some(tools) = tools {
        row = row.child(tools);
    }
    row
}

// ================================================================ 解锁屏

impl AppRoot {
    fn unlock_view(&self, t: &Theme, _window: &Window, cx: &Context<Self>) -> Div {
        let titlebar = div()
            .flex_none()
            .h(px(theme::TITLEBAR_H))
            .px(px(12.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(t.border_soft)
            .child(
                drag_area(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(brand_logo(t))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_2)
                                .child(theme::BRAND_NAME),
                        ),
                )
                .w(px(200.0)),
            )
            .child(win_dots(t, cx));

        // 主按钮：真实解锁（Argon2id 后台派生，见 start_unlock）。
        // 画板 btn-primary：主色垂直渐变 + 粉晕 + 顶部高光；派生中降级灰底平钮。
        let (unlock_icon, unlock_label) = if self.unlocking {
            ("icons/shield.svg", "派生密钥中…")
        } else {
            ("icons/lock.svg", "解锁密码库")
        };
        let unlock = div()
            .id("btn-unlock")
            .h(px(46.0))
            .w_full()
            .rounded(px(theme::RADIUS_MD))
            .when(self.unlocking, |s| s.bg(t.muted))
            .when(
                !self.unlocking,
                |s| {
                    s.bg(gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(t.primary, 0.0),
                        gpui::linear_color_stop(t.primary_strong, 1.0),
                    ))
                    .shadow(vec![t.sh_glow.clone(), t.sh_highlight.clone()])
                },
            )
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .cursor_pointer()
            .active(|s| s.opacity(0.85))
            .child(
                gpui::svg()
                    .path(unlock_icon)
                    .w(px(15.0))
                    .h(px(15.0))
                    .text_color(t.on_primary),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.on_primary)
                    .child(unlock_label),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.start_unlock(cx);
            }));

        let hello = div()
            .id("btn-hello")
            .h(px(44.0))
            .w_full()
            .rounded(px(theme::RADIUS_MD))
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .shadow(vec![t.sh_raised.clone(), t.sh_highlight.clone()]) // 画板 btn-ghost
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(move |s| s.bg(t.hover))
            .child(
                gpui::svg()
                    .path("icons/hello.svg")
                    .w(px(16.0))
                    .h(px(16.0))
                    .text_color(t.lavender),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child("使用 Windows Hello 解锁"),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                // Hello/生物识别在 Phase 2 接入；本版给出引导而不是假解锁。
                this.unlock_error =
                    Some("Windows Hello 将在 Phase 2 接入，本版请用主密码".into());
                cx.notify();
            }));

        // 状态胶囊：绿点=找到库文件；灰点=未找到（给出 vault-cli 引导）。
        let (note_bg, note_dot, note_text) = if self.vault_exists {
            (
                t.mint_soft,
                t.success,
                SharedString::from("我的密码库 · 本地加密"),
            )
        } else {
            (
                t.card_sunken,
                t.muted,
                SharedString::from("未找到密码库 · 先用 vault-cli init 创建"),
            )
        };
        let body = div()
            .flex_1()
            .flex()
            .flex_col()
            .justify_center()
            .px(px(32.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .w(px(56.0))
                            .h(px(56.0))
                            .rounded(px(17.0))
                            // 画板 .logo：对角渐变 + 粉晕 + 顶部高光
                            .bg(gpui::linear_gradient(
                                135.0,
                                gpui::linear_color_stop(t.primary, 0.0),
                                gpui::linear_color_stop(t.lavender, 1.0),
                            ))
                            .shadow(vec![t.sh_glow.clone(), t.sh_highlight.clone()])
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                gpui::svg()
                                    .path("icons/shield.svg")
                                    .w(px(27.0))
                                    .h(px(27.0))
                                    .text_color(rgba(0xFFFFFF_FF)),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(16.0))
                            .text_size(px(24.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text)
                            .child(theme::BRAND_NAME),
                    )
                    .child(
                        div()
                            .mt(px(4.0))
                            .text_size(px(12.0))
                            .text_color(t.muted)
                            .child("本地优先 · 端到端加密密码库"),
                    )
                    .child(
                        div()
                            .mt(px(12.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(12.0))
                            .py(px(4.0))
                            .rounded_full()
                            .bg(note_bg)
                            .child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .rounded_full()
                                    .bg(note_dot),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text_2)
                                    .child(note_text),
                            ),
                    ),
            )
            // 主密码输入井：真实键盘缓冲（圆点遮蔽，眼睛可切换明文）
            .child(
                div()
                    .mt(px(24.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_2)
                            .child("主密码"),
                    )
                    // 主密码输入框组件（掩码 + 眼睛 + Enter 解锁 / Esc 清空）
                    .child(self.password.clone())
                    // 失败/引导提示：红点 + 红字（对齐画板的错误态）
                    .children(self.unlock_error.as_ref().map(|msg| {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .px(px(2.0))
                            .child(
                                div()
                                    .w(px(5.0))
                                    .h(px(5.0))
                                    .rounded_full()
                                    .bg(t.danger),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(t.danger)
                                    .child(msg.clone()),
                            )
                    })),
            )
            .child(div().mt(px(20.0)).child(unlock))
            // 画板：两按钮之间一个裸「或」字，无分割线
            .child(
                div()
                    .mt(px(12.0))
                    .w_full()
                    .flex()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(t.muted)
                    .child("或"),
            )
            .child(div().mt(px(12.0)).child(hello))
            .child(if self.vault_exists {
                // 有库：忘记主密码 → 恢复密钥（Phase 2 接入）
                div()
                    .mt(px(20.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(t.muted)
                            .child("忘记主密码？"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.primary)
                            .child("使用恢复密钥恢复 →"),
                    )
                    .into_any_element()
            } else {
                // 无库：首启引导——在应用内直接建库（不再被 CLI 绑架）
                div()
                    .id("btn-create-vault")
                    .mt(px(20.0))
                    .h(px(40.0))
                    .w_full()
                    .rounded(px(theme::RADIUS_MD))
                    .bg(t.card)
                    .border_1()
                    .border_color(t.primary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(7.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.hover))
                    .child(
                        gpui::svg()
                            .path("icons/plus.svg")
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(t.primary),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.primary)
                            .child("创建新密码库"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.create = Some(CreateForm::new(cx, this.dark));
                        this.screen = Screen::Create;
                        let focus = this.create.as_ref().expect("刚置入").pw.read(cx).focus_handle();
                        window.focus(&focus, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            });

        let foot = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .py(px(12.0))
            .border_t_1()
            .border_color(t.border_soft)
            .child(
                gpui::svg()
                    .path("icons/shield.svg")
                    .w(px(12.0))
                    .h(px(12.0))
                    .text_color(t.success),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(t.text_2)
                    .child("数据加密保存在本机 · 不上传"),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(titlebar)
            .children(self.auto_locked.then(|| {
                // 自动锁定横幅（对齐画板解锁屏 toast）
                div()
                    .flex_none()
                    .mx(px(16.0))
                    .mt(px(10.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(theme::RADIUS_MD))
                    .bg(rgba(0xE0B54A_26))
                    .border_1()
                    .border_color(rgba(0xE0B54A_66))
                    .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(t.warning))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(t.text_2)
                            .child(SharedString::from(format!(
                                "检测到 {} 分钟无操作，密码库已自动锁定",
                                self.autolock_minutes
                            ))),
                    )
            }))
            .child(body)
            .child(foot)
    }

    /// 发起真实解锁：打开库句柄（一次，常驻）→ 后台 Argon2id 派生 + 校验器比对
    /// → 回主线程解密全部存活条目 → 进列表态。失败留在解锁屏给红字。
    fn start_unlock(&mut self, cx: &mut Context<Self>) {
        self.touch();
        self.auto_locked = false;
        if self.unlocking {
            return;
        }
        self.unlock_error = None;
        if !self.vault_exists {
            self.unlock_error = Some(format!(
                "未找到密码库 {}：先用 vault-cli init 创建（或设 PASSPOART_DB 指向库文件）",
                self.db_path.display()
            ));
            cx.notify();
            return;
        }
        if self.password.read(cx).text().is_empty() {
            self.unlock_error = Some("请输入主密码".into());
            cx.notify();
            return;
        }
        // 首次解锁打开句柄；句柄常驻（redb 文件锁独占，RZ0012）
        if self.opened.is_none() {
            match NodbStore::open(&self.db_path) {
                Ok(store) => {
                    let has_meta = matches!(store.read_vault_meta(), Ok(Some(_)));
                    self.opened = Some(OpenedVault { store, vault: None });
                    if !has_meta {
                        self.unlock_error =
                            Some("库未初始化（缺 vault_meta）：请用 vault-cli init 重建".into());
                        cx.notify();
                        return;
                    }
                }
                Err(e) => {
                    self.unlock_error = Some(format!("打开库失败：{e}"));
                    cx.notify();
                    return;
                }
            }
        }
        let Some(meta) = self
            .opened
            .as_ref()
            .and_then(|o| o.store.read_vault_meta().ok())
            .flatten()
        else {
            self.unlock_error = Some("读取库元数据失败".into());
            cx.notify();
            return;
        };
        let password = self.password.read(cx).text().to_string();
        self.unlocking = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            // Argon2id（生产 64MiB/t=3 级）放后台线程；主线程继续渲染「派生中」
            let derived = cx
                .background_executor()
                .spawn(async move { meta.unlock(&password) })
                .await;
            // 实体在窗口关闭时释放；届时更新结果无人接收，忽略即可
            let _ = this.update(cx, |this, cx| {
                this.unlocking = false;
                match derived {
                    Ok(vault) => {
                        let Some(opened) = this.opened.as_mut() else {
                            unreachable!("start_unlock 已保证 opened 初始化");
                        };
                        opened.vault = Some(vault);
                        this.password.update(cx, |i, cx| {
                            i.set_masked(true, cx);
                            i.clear(cx);
                        });
                        this.show_pw = false;
                        this.search_input.update(cx, |i, cx| i.clear(cx));
                        this.selected = None;
                        this.filter = Filter::All;
                        match this.reload_items() {
                            Ok(()) => this.screen = Screen::List,
                            Err(msg) => this.unlock_error = Some(msg),
                        }
                        cx.notify();
                    }
                    Err(_) => {
                        // 校验器不匹配 = 口令错误；KDF 异常同归红字（不区分措辞防侧信道）
                        this.unlock_error = Some("主密码错误，或库参数异常".into());
                        this.password.update(cx, |i, cx| i.clear(cx));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    // ================================================================ 编辑与写入

    /// 重解密全部存活条目刷新快照（解锁、保存、删除、收藏切换后统一走这里）。
    /// 最近更新在前；同刻按标题稳定排序。
    fn reload_items(&mut self) -> Result<(), String> {
        let opened = self.opened.as_ref().ok_or("库未打开")?;
        let vault = opened.vault.as_ref().ok_or("未解锁")?;
        let alive = opened.store.list_alive().map_err(|e| e.to_string())?;
        let mut items = Vec::with_capacity(alive.len());
        for stored in &alive {
            let core = open_item_from_storage(vault, stored).map_err(|e| e.to_string())?;
            items.push(data::Item::from_parts(&core, stored));
        }
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.title.cmp(&b.title)));
        self.items = items;
        Ok(())
    }

    /// 保存编辑表单：组装明文条目 → 信封加密 → 覆盖写 → 刷新快照回列表。
    fn save_edit(&mut self, cx: &mut Context<Self>) {
        self.touch();
        let Some(form) = self.edit.take() else { return };
        let title = form.title.read(cx).text().trim().to_string();
        if title.is_empty() {
            self.edit = Some(EditForm {
                error: Some("名称必填".into()),
                ..form
            });
            cx.notify();
            return;
        }
        let item = vault_core::model::Item {
            id: form.editing_id.clone().unwrap_or_default(),
            kind: form.kind.to_core(),
            title,
            subtitle: form.user.read(cx).text().trim().to_string(),
            url: form.url.read(cx).text().trim().to_string(),
            secret: form.secret.read(cx).text().to_string(),
            totp_secret: {
                let t = form.totp.read(cx).text().trim();
                if t.is_empty() { None } else { Some(t.to_string()) }
            },
            favorite: form.starred,
            note: form.note.read(cx).text().to_string(),
            version: form.editing_version,
        };
        let Some(opened) = self.opened.as_mut() else {
            self.edit = Some(form);
            return;
        };
        let Some(vault) = opened.vault.as_ref() else {
            self.edit = Some(form);
            return;
        };
        let sealed = seal_item_for_storage(vault, &item).map_err(|e| e.to_string());
        let outcome = sealed.and_then(|stored| {
            let id = stored.id.clone();
            opened
                .store
                .put(&stored)
                .map_err(|e| e.to_string())
                .and_then(|()| {
                    opened
                        .store
                        .log_action("put", Some(&id))
                        .map_err(|e| e.to_string())
                })
        });
        match outcome {
            Ok(()) => {
                if let Err(msg) = self.reload_items() {
                    self.unlock_error = Some(msg);
                    self.screen = Screen::Unlock;
                } else {
                    self.screen = Screen::List;
                }
                cx.notify();
            }
            Err(msg) => {
                self.edit = Some(EditForm {
                    error: Some(format!("保存失败：{msg}")),
                    ..form
                });
                cx.notify();
            }
        }
    }

    /// 重封一条：从库里解密权威原文 → 变换 → 版本+1 → 重新密封落盘。
    /// 收藏切换等不改正文的修改专用——视图快照是有损投影，不能当回写源。
    fn reseal_item(
        &mut self,
        id: &str,
        mutate: impl FnOnce(&mut vault_core::model::Item),
    ) -> Result<(), String> {
        let opened = self.opened.as_mut().ok_or("库未打开")?;
        let vault = opened.vault.as_ref().ok_or("未解锁")?;
        let stored = opened
            .store
            .get(id)
            .map_err(|e| e.to_string())?
            .ok_or("条目不存在")?;
        let mut core = open_item_from_storage(vault, &stored).map_err(|e| e.to_string())?;
        core.version = stored.version + 1;
        mutate(&mut core);
        let sealed = seal_item_for_storage(vault, &core).map_err(|e| e.to_string())?;
        // vault 借用在 seal 后结束，store 转可变
        opened.store.put(&sealed).map_err(|e| e.to_string())?;
        opened
            .store
            .log_action("put", Some(id))
            .map_err(|e| e.to_string())
    }

    /// 删除当前选中条目（墓碑），回列表。
    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        self.touch();
        let Some(view) = self.selected.and_then(|i| self.items.get(i)) else { return };
        let id = view.id.clone();
        let result = (|| -> Result<(), String> {
            let opened = self.opened.as_mut().ok_or("库未打开")?;
            opened
                .store
                .tombstone(&id, vault_core::storage::now_secs())
                .map_err(|e| e.to_string())?;
            opened
                .store
                .log_action("tombstone", Some(&id))
                .map_err(|e| e.to_string())
        })();
        if result.is_ok() {
            if let Err(msg) = self.reload_items() {
                self.unlock_error = Some(msg);
                self.screen = Screen::Unlock;
            } else {
                self.screen = Screen::List;
            }
            self.selected = None;
        }
        self.confirm_delete = false;
        cx.notify();
    }

    /// 收藏切换：重封 + 刷新后把选中钉回同一条（更新时间排序会挪位）。
    fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        self.touch();
        let Some(view) = self.selected.and_then(|i| self.items.get(i)) else { return };
        let id = view.id.clone();
        if self.reseal_item(&id, |it| it.favorite = !it.favorite).is_ok()
            && self.reload_items().is_ok() {
                self.selected = self
                    .items
                    .iter()
                    .position(|it| it.id == id);
            }
        cx.notify();
    }

    /// 打开新建表单（列表态 + 按钮）。
    fn open_edit_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.touch();
        let form = EditForm::empty(cx, self.dark);
        self.edit = Some(form);
        self.screen = Screen::Edit;
        let focus = self.edit.as_ref().expect("刚置入").title.read(cx).focus_handle();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// 建库密码框的独立构造（start_create 错误兜底用）。
    fn new_create_pw(&self, cx: &mut Context<Self>) -> Entity<TextInput> {
        let t = if self.dark { Theme::dark() } else { Theme::light() };
        let weak = cx.entity().downgrade();
        let s = well_style(&t, 44.0, 15.0);
        let w1 = weak.clone();
        cx.new(|cx| {
            TextInput::new(cx, "主密码", s)
                .masked()
                .on_enter(move |_, cx| {
                    let w = w1.clone();
                    cx.spawn(async move |cx| {
                        let _ = w.update(cx, |app, cx| app.start_create(cx));
                    })
                    .detach();
                })
        })
    }

    /// 打开编辑表单：从库解密权威原文回填（版本 = 旧版 + 1）。
    fn open_edit_existing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.touch();
        let Some(view) = self.selected.and_then(|i| self.items.get(i)) else { return };
        let id = view.id.clone();
        let Some(opened) = self.opened.as_ref() else { return };
        let Some(vault) = opened.vault.as_ref() else { return };
        let Some(stored) = opened.store.get(&id).ok().flatten() else {
            return;
        };
        let Ok(core) = open_item_from_storage(vault, &stored) else {
            return;
        };
        self.edit = Some(EditForm::from_core(&core, stored.version + 1, self.dark, cx));
        self.screen = Screen::Edit;
        let focus = self.edit.as_ref().expect("刚置入").title.read(cx).focus_handle();
        window.focus(&focus, cx);
        cx.notify();
    }
}

// ================================================================ 列表态

impl AppRoot {
    fn list_view(&self, t: &Theme, _window: &Window, cx: &Context<Self>) -> Div {
        // 标题栏：品牌 + 同步胶囊 + 主题切换 / 体检（带红点） / 锁定 / 窗控。
        let theme_btn = {
            let dark = self.dark;
            let (icon, ink) = if dark {
                ("icons/sun.svg", t.warning)
            } else {
                ("icons/moon.svg", t.lavender)
            };
            icon_btn("btn-theme", icon, ink, t.hover).on_click(cx.listener(move |this, _, _, cx| {
                this.dark = !dark;
                this.restyle_inputs(cx);
                cx.notify();
            }))
        };
        // 设置入口（v0.2.1：取代无实际功能的安全图标）
        let settings_btn = icon_btn(
            "btn-settings",
            "icons/settings.svg",
            t.text_2,
            t.hover,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.touch();
            this.screen = Screen::Settings;
            cx.notify();
        }));
        let add_btn = icon_btn("btn-add", "icons/plus.svg", t.text_2, t.hover).on_click(
            cx.listener(|this, _, window, cx| this.open_edit_new(window, cx)),
        );
        let lock_btn = icon_btn("btn-lock", "icons/lock.svg", t.text_2, t.hover).on_click(
            cx.listener(|this, _, window, cx| {
                this.touch();
                this.do_lock(false, cx);
                let focus = this.password.read(cx).focus_handle();
                window.focus(&focus, cx);
                cx.notify();
            }),
        );

        let titlebar = div()
            .flex_none()
            .h(px(theme::TITLEBAR_H))
            .px(px(12.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
                        .child(
                drag_area(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(brand_logo(t))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_2)
                                .child(theme::BRAND_NAME),
                        ),
                ),
            )
            .child(theme_btn)
            .child(settings_btn)
            .child(add_btn)
            .child(lock_btn)
            .child(div().w(px(4.0)))
            .child(win_dots(t, cx));

        // 搜索输入框组件（前置图标 + 后置清除 / Ctrl F）
        let search = self.search_input.clone();

        // 过滤 chips：计数取自真实解密快照。色相按分类走，不再一刷红。
        // 字段序：(filter, name, count, dot, accent, soft, border_active, border_idle)
        type ChipSpec = (
            Filter, &'static str, Option<u32>,
            Option<gpui::Rgba>, gpui::Rgba, gpui::Rgba, gpui::Rgba, gpui::Rgba,
        );
        let favorite_count = self.items.iter().filter(|i| i.starred).count() as u32;
        let chips: Vec<ChipSpec> = vec![
            (Filter::All, "全部", Some(self.items.len() as u32),
                None, t.primary, t.primary_soft, rgba(0xE4739D_59), rgba(0xE4739D_4D)),
            (Filter::Favorite, "收藏", Some(favorite_count),
                None, t.primary, t.primary_soft, rgba(0xE4739D_59), rgba(0xE4739D_4D)),
            (Filter::Kind(Kind::Login), "登录", None,
                Some(t.primary), t.primary, t.primary_soft, rgba(0xE4739D_59), rgba(0xE4739D_4D)),
            (Filter::Kind(Kind::Note), "笔记", None,
                Some(t.mint), t.mint, t.mint_soft, rgba(0x58B98C_59), rgba(0x58B98C_4D)),
            (Filter::Kind(Kind::Card), "银行卡", None,
                Some(t.peach), t.peach, t.peach_soft, rgba(0xE9945F_59), rgba(0xE9945F_4D)),
            (Filter::Kind(Kind::Identity), "身份", None,
                Some(t.lavender), t.lavender, t.lavender_soft, rgba(0x9B7FDF_59), rgba(0x9B7FDF_4D)),
        ];
        let chip_row = div()
            .mt(px(8.0))
            .flex()
            .flex_row()
            .gap(px(6.0))
            .children(chips.iter().enumerate().map(|(i, (f, name, cnt, dot, accent, soft, b_active, b_idle))| {
                let active = *f == self.filter;
                // listener 要求 'static：先把 Copy 的 Filter 解引用成值再捕获。
                let picked = *f;
                let (bg, _ink, border) = if active {
                    // 选中：同色相低透背景 + 35% 描边 + 凸影
                    (*soft, *accent, *b_active)
                } else {
                    // 默认：画板 card (warm white 60% / dark 黑紫 55%) + 30% 描边
                    (t.card, *accent, *b_idle)
                };
                div()
                    .id(ElementId::Name(SharedString::from(format!("chip-{i}"))))
                    .h(px(26.0))
                    .px(px(11.0))
                    .rounded_full()
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .when(active, |s| s.shadow(vec![t.sh_raised.clone()]))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.hover))
                    .children(dot.map(|c| {
                        div().w(px(5.0)).h(px(5.0)).rounded_full().bg(c)
                    }))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.muted)
                            .child(*name),
                    )
                    .children(cnt.map(|n| {
                        div()
                            .text_size(px(10.0))
                            .text_color(t.muted)
                            .child(n.to_string())
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = picked;
                        cx.notify();
                    }))
            }));

        // 条目行。（画板 .rows：容器 8px 内边距 + 行内 8px → 内容与行头 16px 对齐，
        // 比工具栏 12px 稍收进，不再超宽）
        let rows = div()
            .id("rows")
            .mt(px(8.0))
            .px(px(8.0))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .on_scroll_wheel(cx.listener(|this, _, _, _cx| this.touch()))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.muted)
                    .child(match self.filter {
                        Filter::All => "全部条目",
                        Filter::Favorite => "收藏",
                        Filter::Kind(k) => k.label(),
                    }),
            )
            .children(data::indices(&self.items, self.filter, self.search_input.read(cx).text())
                    .into_iter()
                    .map(|i| {
                let item = &self.items[i];
                let selected = self.selected == Some(i);
                let hover = t.hover;
                let row = div()
                    .id(ElementId::Name(SharedString::from(format!("item-{i}"))))
                    .mb(px(2.0))
                    .px(px(8.0))
                    .py(px(7.0))
                    .rounded(px(theme::RADIUS_MD))
                    .border_1()
                    .border_color(if selected {
                        rgba(0xE4739D_4D)
                    } else {
                        rgba(0x000000_00)
                    })
                    .bg(if selected {
                        t.primary_soft
                    } else {
                        // 默认行：画板 card（亮色 warm white 60% / 暗色黑紫 55%），叠在窗底上是设计稿浅粉白画板该看到的质感
                        t.card
                    })
                    // 画板 item.sel：选中行凸影
                    .when(selected, |s| s.shadow(vec![t.sh_raised.clone()]))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(favicon(t, item, 34.0, 14.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text)
                                    .child(item.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(t.muted)
                                    .child(item.sub.clone()),
                            ),
                    );
                // TOTP：渲染时现算当前码（每秒时钟驱动重绘；分组 3+3 显示）
                let row = match &item.totp_seed {
                    Some(seed) => {
                        let code = vault_core::totp::totp_code(
                            seed,
                            vault_core::storage::now_secs(),
                        )
                        .unwrap_or_else(|| "——————".into());
                        let grouped = format!("{} {}", &code[..3], &code[3..]);
                        row.child(pill(t.sky_soft, t.sky, &grouped))
                    }
                    None => row,
                };
                let row = if item.starred {
                    row.child(
                        gpui::svg()
                            .path("icons/star.svg")
                            .w(px(12.0))
                            .h(px(12.0))
                            .text_color(t.butter),
                    )
                } else {
                    row
                };
                row.child(
                    gpui::svg()
                        .path("icons/chevron.svg")
                        .w(px(10.0))
                        .h(px(10.0))
                        .text_color(t.muted),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.touch();
                    this.selected = Some(i);
                    this.copied = false;
                    this.show_pw = false;
                    this.screen = Screen::Detail;
                    cx.notify();
                }))
            }));

        div()
            .flex()
            .flex_col()
            .size_full()
            // Ctrl F：聚焦搜索框（画板提示语兑现）
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.touch();
                if ev.keystroke.modifiers.control && ev.keystroke.key == "f" {
                    let focus = this.search_input.read(cx).focus_handle();
                    window.focus(&focus, cx);
                }
            }))
            .child(titlebar)
            .child(
                div()
                    .flex_none()
                    .px(px(12.0))
                    .pt(px(12.0))
                    .child(search)
                    .child(chip_row),
            )
            .child(rows)
    }
}

// ================================================================ 详情态

impl AppRoot {
    /// 打开条目网址：补 https:// 前缀后交系统默认浏览器；空网址静默跳过。
    fn open_item_url(&mut self, cx: &mut Context<Self>) {
        self.touch();
        let Some(sel) = self.selected else { return };
        let raw = self.items[sel].url.trim().to_string();
        if raw.is_empty() {
            self.set_status(false, "该条目没有网址", cx);
            return;
        }
        let url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw
        } else {
            format!("https://{raw}")
        };
        cx.open_url(&url);
        self.set_status(true, "已在浏览器打开", cx);
    }

    /// 设置底部状态栏消息：`(成功?, 文案)`，1.8s 后自动恢复默认提示。代号防误清：
    /// 中途又设了新状态，旧 timer 醒来发现代号不一致就不动新内容。
    fn set_status(&mut self, ok: bool, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = Some((ok, msg.into()));
        self.status_gen += 1;
        let gen = self.status_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1800))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.status_gen == gen {
                    this.status = None;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn detail_view(&self, t: &Theme, cx: &Context<Self>) -> Div {
        let Some(sel) = self.selected else {
            // 不可达：Detail 仅从已选行进入；兑底返空 div。
            return div();
        };
        let item = &self.items[sel];
        let (kbg, kink) = kind_colors(t, item.kind);

        let titlebar = div()
            .flex_none()
            .h(px(theme::TITLEBAR_H))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .border_b_1()
            .border_color(t.border_soft)
            .child(icon_btn("btn-back", "icons/back.svg", t.text_2, t.hover).on_click(
                cx.listener(|this, _, _, cx| {
                    this.touch();
                    this.screen = Screen::List;
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        div()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(6.0))
                            .bg(kbg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(kink)
                            .child(SharedString::from(data::first_char(&item.title))),
                    )
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(item.title.clone()),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(t.muted)
                    .child("Esc 返回"),
            );

        // 主操作：复制密码 → 已复制反馈（真实实现走剪贴板 + 30s 清空）。
        // 画板 act-p：主色渐变 + 粉晕 + 高光；已复制态转成功绿平钮。
        let (copy_icon, copy_text) = if self.copied {
            if self.clip_autoclear {
                ("icons/check.svg", "已复制 · 30s 清空")
            } else {
                ("icons/check.svg", "已复制")
            }
        } else {
            ("icons/lock.svg", "复制密码")
        };
        let copy_btn = div()
            .id("btn-copy-pw")
            .h(px(30.0))
            .px(px(14.0))
            .rounded(px(theme::RADIUS_SM))
            .when(self.copied, |s| s.bg(t.success))
            .when(
                !self.copied,
                |s| {
                    s.bg(gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(t.primary, 0.0),
                        gpui::linear_color_stop(t.primary_strong, 1.0),
                    ))
                    .shadow(vec![t.sh_glow.clone(), t.sh_highlight.clone()])
                },
            )
            .flex()
            .items_center()
            .gap(px(5.0))
            .cursor_pointer()
            .active(|s| s.opacity(0.85))
            .child(
                gpui::svg()
                    .path(copy_icon)
                    .w(px(12.0))
                    .h(px(12.0))
                    .text_color(t.on_primary),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.on_primary)
                    .child(copy_text),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.touch();
                // 真实写入系统剪贴板；30 秒后自动清空（代号防误清：期间又复制了就不动）
                let Some(sel) = this.selected else { return };
                let secret = this.items[sel].secret.clone();
                if secret.is_empty() {
                    this.set_status(false, "该条目没有可复制内容", cx);
                    return;
                }
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(secret));
                this.copied = true;
                if this.clip_autoclear {
                    this.set_status(true, "已复制 · 30s 清空", cx);
                } else {
                    this.set_status(true, "已复制", cx);
                }
                if this.clip_autoclear {
                this.clip_gen += 1;
                let gen = this.clip_gen;
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(30))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.clip_gen == gen {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                String::new(),
                            ));
                            this.copied = false;
                        }
                        cx.notify();
                    });
                })
                .detach();
                }
                cx.notify();
            }));

        let ghost_btn = |id: &'static str, icon: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(30.0))
                .px(px(12.0))
                .rounded(px(theme::RADIUS_SM))
                .bg(t.card)
                .border_1()
                .border_color(t.border)
                .flex()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .hover(move |s| s.bg(t.hover))
                .child(
                    gpui::svg()
                        .path(icon)
                        .w(px(12.0))
                        .h(px(12.0))
                        .text_color(t.text_2),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text_2)
                        .child(label),
                )
        };

        // TOTP：渲染时现算动态码 + 周期倒计时条（每秒时钟驱动重绘）。
        let totp_value = match &item.totp_seed {
            Some(seed) => {
                let now = vault_core::storage::now_secs();
                let code =
                    vault_core::totp::totp_code(seed, now).unwrap_or_else(|| "——————".into());
                let left = vault_core::totp::seconds_remaining(now) as f32;
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(17.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text)
                            .child(format!("{} {}", &code[..3], &code[3..])),
                    )
                    .child(
                        div()
                            .w(px(44.0))
                            .h(px(3.0))
                            .rounded(px(2.0))
                            .bg(t.border_soft)
                            .child(
                                div()
                                    .w(px(44.0 * left / 30.0))
                                    .h(px(3.0))
                                    .rounded(px(2.0))
                                    .bg(if left <= 5.0 { t.danger } else { t.mint }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(t.muted)
                            .child(format!("{left}s")),
                    )
            }
            None => div().text_size(px(12.0)).text_color(t.muted).child("未配置"),
        };

        // 字段值复制按钮：真实写剪贴板 + 底部状态栏反馈；hover / 按下都有底色反馈。
        let totp_code = item
            .totp_seed
            .as_ref()
            .and_then(|seed| vault_core::totp::totp_code(seed, vault_core::storage::now_secs()))
            .unwrap_or_default();
        let copy_ic = |id: &'static str, text: String, label: &'static str| {
            div()
                .id(id)
                .w(px(25.0))
                .h(px(25.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .hover(move |s| s.bg(t.hover))
                .active(move |s| s.bg(t.hover))
                .child(
                    gpui::svg()
                        .path("icons/copy.svg")
                        .w(px(13.0))
                        .h(px(13.0))
                        .text_color(t.muted),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if text.is_empty() {
                        this.set_status(false, format!("该条目没有{label}"), cx);
                        return;
                    }
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    this.set_status(true, format!("已复制{label}"), cx);
                }))
        };

        let fields = div()
            .mt(px(12.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(field_row(
                t,
                "用户名",
                div()
                    .text_size(px(12.0))
                    .text_color(t.text)
                    .child(item.sub.clone()),
                Some(copy_ic("copy-user", item.sub.clone(), "用户名")),
            ))
            .child(field_row(
                t,
                "密码",
                div()
                    .text_size(px(13.0))
                    .text_color(t.text)
                    .child(if self.show_pw {
                        SharedString::from(item.secret.clone())
                    } else {
                        SharedString::from("••••••••••••••")
                    }),
                Some(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(strength_bar(t, item.strength))
                        .child(
                            div()
                                .id("reveal-eye")
                                .w(px(25.0))
                                .h(px(25.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(7.0))
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.hover))
                                .child(
                                    gpui::svg()
                                        .path("icons/eye.svg")
                                        .w(px(13.0))
                                        .h(px(13.0))
                                        .text_color(if self.show_pw { t.primary } else { t.muted }),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.touch();
                                    this.show_pw = !this.show_pw;
                                    if this.show_pw {
                                        // 切到明文：有设定时长则起自动掩码倒计时（代号防误遮）
                                        if this.pw_reveal_secs > 0 {
                                            let secs = this.pw_reveal_secs;
                                            this.pw_reveal_gen += 1;
                                            let gen = this.pw_reveal_gen;
                                            cx.spawn(async move |this, cx| {
                                                cx.background_executor()
                                                    .timer(std::time::Duration::from_secs(secs))
                                                    .await;
                                                let _ = this.update(cx, |this, cx| {
                                                    if this.pw_reveal_gen == gen && this.show_pw {
                                                        this.show_pw = false;
                                                    }
                                                    cx.notify();
                                                });
                                            })
                                            .detach();
                                        }
                                    } else {
                                        // 手动切回掩码：递增代号让在途定时器作废
                                        this.pw_reveal_gen += 1;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(copy_ic("copy-pw", item.secret.clone(), "密码")),
                ),
            ))
            .child(field_row(
                t,
                "网址",
                if item.url.is_empty() {
                    div()
                        .text_size(px(12.0))
                        .text_color(t.sky)
                        .child(SharedString::from("—"))
                        .into_any_element()
                } else {
                    div()
                        .text_size(px(12.0))
                        .text_color(t.sky)
                        .child(SharedString::from(item.url.clone()))
                        .into_any_element()
                },
                Some(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(2.0))
                        .child(copy_ic("copy-url", item.url.clone(), "网址"))
                        .child(
                            div()
                                .id("btn-open-url")
                                .w(px(25.0))
                                .h(px(25.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(7.0))
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.hover))
                                .child(
                                    gpui::svg()
                                        .path("icons/link.svg")
                                        .w(px(13.0))
                                        .h(px(13.0))
                                        .text_color(t.muted),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.open_item_url(cx))),
                        ),
                ),
            ))
            .child(field_row(
                t,
                "验证码",
                totp_value,
                Some(copy_ic("copy-totp", totp_code.clone(), "验证码")),
            ))
            .child(field_row(
                t,
                "备注",
                div()
                    .text_size(px(11.0))
                    .text_color(t.text_2)
                    .child(if item.note.is_empty() {
                        SharedString::from("—")
                    } else {
                        SharedString::from(item.note.clone())
                    }),
                None::<Div>,
            ));

        let body = div()
            .id("detail-body")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .on_scroll_wheel(cx.listener(|this, _, _, _cx| this.touch()))
            .px(px(16.0))
            .py(px(14.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .child(favicon(t, item, 44.0, 18.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(t.text)
                                        .child(item.title.clone()),
                                )
                                    .child(
                                        // 收藏星标：可点（重封翻转 favorite）
                                        div()
                                            .id("btn-star")
                                            .w(px(20.0))
                                            .h(px(20.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(6.0))
                                            .cursor_pointer()
                                            .hover(move |s| s.bg(t.hover))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.toggle_favorite(cx);
                                            }))
                                            .child(
                                                gpui::svg()
                                                    .path(if item.starred {
                                                        "icons/star.svg"
                                                    } else {
                                                        "icons/star-off.svg"
                                                    })
                                                    .w(px(13.0))
                                                    .h(px(13.0))
                                                    .text_color(if item.starred {
                                                        t.butter
                                                    } else {
                                                        t.muted
                                                    }),
                                            ),
                                    ),
                            )
                            .child(
                                div().mt(px(3.0)).flex().flex_row().gap(px(4.0)).children(
                                    if item.starred {
                                        Some(pill(t.butter_soft, t.butter, "已收藏"))
                                    } else {
                                        None
                                    },
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .text_size(px(10.0))
                    .text_color(t.muted)
                    .child(format!(
                        "最近更新 {} · 版本 v{}",
                        data::relative_time(item.updated_at),
                        item.version,
                    )),
            )
            .child(
                div()
                    .mt(px(12.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(copy_btn)
                    .child(ghost_btn("btn-edit", "icons/pencil.svg", "编辑").on_click(
                        cx.listener(|this, _, window, cx| this.open_edit_existing(window, cx)),
                    ))
                    .child(ghost_btn("btn-open", "icons/link.svg", "打开网址").on_click(
                        cx.listener(|this, _, _, cx| this.open_item_url(cx)),
                    ))
                    // 删除：二次确认（首击变红字，再击才执行；Esc 撤销）
                    .child({
                        let confirming = self.confirm_delete;
                        div()
                            .id("btn-delete")
                            .h(px(30.0))
                            .px(px(12.0))
                            .rounded(px(theme::RADIUS_SM))
                            .bg(if confirming { t.danger } else { t.card })
                            .border_1()
                            .border_color(if confirming { t.danger } else { t.border })
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .cursor_pointer()
                            .hover(move |s| s.opacity(0.9))
                            .child(
                                gpui::svg()
                                    .path("icons/close.svg")
                                    .w(px(11.0))
                                    .h(px(11.0))
                                    .text_color(if confirming { t.on_primary } else { t.danger }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if confirming { t.on_primary } else { t.danger })
                                    .child(if confirming { "确认删除？" } else { "删除" }),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.confirm_delete {
                                    this.delete_selected(cx);
                                } else {
                                    this.confirm_delete = true;
                                    cx.notify();
                                }
                            }))
                    }),
            )
            .child(fields)
            .child(
                div()
                    .mt(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                gpui::svg()
                                    .path("icons/eye.svg")
                                    .w(px(11.0))
                                    .h(px(11.0))
                                    .text_color(t.muted),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text_2)
                                    .child("密码历史"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.primary_strong)
                                            .child(""),
                                    ),
                            ),
                    )
                    .children(["历史版本 · 随同步任务（Phase 3）接入"].map(|line| {
                        div()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(8.0))
                            .bg(t.row_alt)
                            .text_size(px(11.0))
                            .text_color(t.text_2)
                            .child(line)
                    })),
            );

        let foot = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(16.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(t.border_soft)
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(t.muted)
                    .child(format!(
                        "更新于 {} · v{}",
                        data::relative_time(item.updated_at),
                        item.version,
                    )),
            )
            .child(match &self.status {
                Some((ok, msg)) => div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if *ok { t.success } else { t.danger })
                    .child(msg.clone())
                    .into_any_element(),
                None => div()
                    .text_size(px(10.0))
                    .text_color(t.muted)
                    .child("复制后 30 秒清空剪贴板")
                    .into_any_element(),
            });

        div()
            .flex()
            .flex_col()
            .size_full()
            // Esc 返回列表（画板右上角提示语兑现）；详情无其他键位
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                if ev.keystroke.key == "escape" {
                    this.screen = Screen::List;
                    this.copied = false;
                    this.confirm_delete = false;
                    cx.notify();
                }
            }))
            .child(titlebar)
            .child(body)
            .child(foot)
    }
}

// ================================================================ 编辑态

/// 字段标签行：名称 + 右侧弱提示（如「必填」）。
fn edit_label(t: &Theme, name: &'static str, hint: Option<&'static str>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_2)
                .child(name),
        )
        .children(hint.map(|h| {
            div().text_size(px(10.0)).text_color(t.muted).child(h)
        }))
}

// ================================================================ 编辑态（组件版）

impl AppRoot {
    /// 编辑屏：类型 chips + 六字段输入组件 + 收藏行 + 加密说明。
    fn edit_view(&self, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(form) = self.edit.as_ref() else {
            return self.list_view(t, window, cx);
        };
        let is_new = form.editing_id.is_none();

        let cancel_btn =
            icon_btn("btn-cancel-edit", "icons/back.svg", t.text_2, t.hover).on_click(
                cx.listener(|this, _, _, cx| {
                    this.touch();
                    this.edit = None;
                    this.screen = Screen::List;
                    cx.notify();
                }),
            );
        // 保存按钮：紧凑描边样式（主色文字），不抢标题栏视觉；凸影 + 顶部高光（画板 btn-ghost）
        let save_btn = div()
            .id("btn-save")
            .h(px(24.0))
            .px(px(10.0))
            .rounded(px(theme::RADIUS_SM))
            .bg(t.card_strong)
            .border_1()
            .border_color(t.primary)
            .shadow(vec![t.sh_raised.clone(), t.sh_highlight.clone()])
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(move |s| s.bg(t.hover))
            .active(|s| s.opacity(0.85))
            .child(
                gpui::svg()
                    .path("icons/check.svg")
                    .w(px(10.0))
                    .h(px(10.0))
                    .text_color(t.primary),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.primary)
                    .child("保存"),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.touch();
                this.save_edit(cx);
            }));

        let titlebar = div()
            .flex_none()
            .h(px(theme::TITLEBAR_H))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .border_b_1()
            .border_color(t.border_soft)
            .child(cancel_btn)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child(if is_new { "新建条目" } else { "编辑条目" }),
            )
            .child(save_btn);

        // 类型 chips（登录/笔记/银行卡/身份，分类色圆点）
        let kinds: [(Kind, &'static str, gpui::Rgba); 4] = [
            (Kind::Login, "登录", t.primary),
            (Kind::Note, "笔记", t.mint),
            (Kind::Card, "银行卡", t.peach),
            (Kind::Identity, "身份", t.lavender),
        ];
        let type_row = div()
            .mt(px(10.0))
            .flex()
            .flex_row()
            .gap(px(6.0))
            .children(kinds.iter().enumerate().map(|(i, (k, name, dot))| {
                let active = form.kind == *k;
                let picked = *k;
                div()
                    .id(ElementId::Name(SharedString::from(format!("ek-{i}"))))
                    .h(px(28.0))
                    .px(px(11.0))
                    .rounded_full()
                    .bg(if active { t.primary_soft } else { t.card })
                    .border_1()
                    // 画板 type-chip.on：主色 35% 描边 + 凸影
                    .border_color(if active {
                        rgba(0xE4739D_59)
                    } else {
                        t.border_soft
                    })
                    .when(active, |s| s.shadow(vec![t.sh_raised.clone()]))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.hover))
                    .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(*dot))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if active { t.primary_strong } else { t.text_2 })
                            .child(*name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.touch();
                        if let Some(form) = this.edit.as_mut() {
                            form.kind = picked;
                        }
                        cx.notify();
                    }))
            }));

        // 收藏行：星标 + 开关
        let fav_row = div()
            .id("row-fav")
            .h(px(38.0))
            .px(px(12.0))
            .rounded(px(theme::RADIUS_MD))
            .bg(t.card_sunken)
            .border_1()
            .border_color(t.border_soft)
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.touch();
                if let Some(form) = this.edit.as_mut() {
                    form.starred = !form.starred;
                }
                cx.notify();
            }))
            .child(
                gpui::svg()
                    .path(if form.starred { "icons/star.svg" } else { "icons/star-off.svg" })
                    .w(px(13.0))
                    .h(px(13.0))
                    .text_color(if form.starred { t.butter } else { t.muted }),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(t.text_2)
                    .child("加入收藏"),
            )
            .child(div().flex_1())
            .child(
                // 开关视觉：胶囊 + 圆钮（左关右开；两分支同为 Div）——凸影 + 旋钮影
                div()
                    .w(px(30.0))
                    .h(px(17.0))
                    .rounded_full()
                    .bg(if form.starred { t.success } else { t.border })
                    .shadow(vec![t.sh_raised.clone()])
                    .relative()
                    .child(if form.starred {
                        div()
                            .absolute()
                            .top(px(2.0))
                            .right(px(2.0))
                            .w(px(13.0))
                            .h(px(13.0))
                            .rounded_full()
                            .bg(rgba(0xFFFFFF_FF))
                            .shadow(vec![t.sh_knob.clone()])
                    } else {
                        div()
                            .absolute()
                            .top(px(2.0))
                            .left(px(2.0))
                            .w(px(13.0))
                            .h(px(13.0))
                            .rounded_full()
                            .bg(rgba(0xFFFFFF_FF))
                            .shadow(vec![t.sh_knob.clone()])
                    }),
            );

        let body = div()
            .id("edit-body")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .on_scroll_wheel(cx.listener(|this, _, _, _cx| this.touch()))
            .px(px(16.0))
            .py(px(10.0))
            .flex()
            .flex_col()
            .children(form.error.as_ref().map(|msg| {
                div()
                    .mb(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(t.danger))
                    .child(div().text_size(px(11.0)).text_color(t.danger).child(msg.clone()))
            }))
            .child(type_row)
            .child(
                div()
                    .mt(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(edit_label(t, "名称", Some("必填")))
                    .child(form.title.clone())
                    .child(edit_label(t, "网址", None))
                    .child(form.url.clone())
                    .child(edit_label(t, "用户名", None))
                    .child(form.user.clone())
                    .child(edit_label(t, "密码", Some("本地随机生成 · 永不上传")))
                    .child(form.secret.clone())
                    .child(strength_bar(
                        t,
                        data::strength_of(form.secret.read(cx).text()),
                    ))
                    .child(edit_label(t, "验证器密钥", Some("TOTP base32")))
                    .child(form.totp.clone())
                    .child(fav_row)
                    .child(edit_label(t, "备注", Some("加密保存")))
                    .child(form.note.clone()),
            )
            .child(
                div()
                    .mt(px(8.0))
                    .mb(px(2.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        gpui::svg()
                            .path("icons/shield.svg")
                            .w(px(11.0))
                            .h(px(11.0))
                            .text_color(t.muted),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(t.muted)
                            .child("保存时在本机加密"),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            // 兜底 Esc：焦点不在任何字段组件时取消编辑（组件内的 Esc 走 on_escape）
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key != "escape" {
                    return;
                }
                let Some(form) = this.edit.as_ref() else { return };
                let handles = [
                    form.title.read(cx).focus_handle(),
                    form.url.read(cx).focus_handle(),
                    form.user.read(cx).focus_handle(),
                    form.secret.read(cx).focus_handle(),
                    form.totp.read(cx).focus_handle(),
                    form.note.read(cx).focus_handle(),
                ];
                if !handles.iter().any(|h| h.is_focused(window)) {
                    this.touch();
                    this.edit = None;
                    this.screen = Screen::List;
                    cx.notify();
                }
            }))
            .child(titlebar)
            .child(body)
    }
}

// ================================================================ 建库态（首启）

impl AppRoot {
    /// 建库：校验 → 后台 Argon2id（生产参数）→ 主线程落盘 meta → 直接解锁进空列表。
    fn start_create(&mut self, cx: &mut Context<Self>) {
        self.touch();
        if self.creating {
            return;
        }
        let Some(form) = self.create.take() else { return };
        let fail = |this: &mut Self, msg: &str, mut form: CreateForm, cx: &mut Context<Self>| {
            form.error = Some(msg.to_string());
            this.create = Some(form);
            cx.notify();
        };
        let pw = form.pw.read(cx).text().to_string();
        if pw.chars().count() < 8 {
            fail(self, "主密码至少 8 个字符", form, cx);
            return;
        }
        if pw != form.pw2.read(cx).text() {
            fail(self, "两次输入不一致", form, cx);
            return;
        }
        // 打开句柄（文件不存在则建，父目录自动补齐）；句柄常驻到退出
        if self.opened.is_none() {
            match NodbStore::open(&self.db_path) {
                Ok(store) => {
                    self.opened = Some(OpenedVault { store, vault: None });
                }
                Err(e) => {
                    fail(self, &format!("创建库文件失败：{e}"), form, cx);
                    return;
                }
            }
        }
        self.create = Some(form);
        self.creating = true;
        cx.notify();

        let salt = generate_salt();
        let params = KdfParams::default(); // 生产参数：64MiB / t=3，建库不给弱开关
        cx.spawn(async move |this, cx| {
            // Argon2id 放后台；产出 vault（含 VK）+ 校验器
            let derived = cx
                .background_executor()
                .spawn(async move {
                    let mk = derive_master_key(&pw, &salt, params)
                        .map_err(|e| e.to_string())?;
                    let verifier = derive_local_verifier(&mk);
                    Ok::<_, String>((UnlockedVault::from_master_key(mk), verifier))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                match derived {
                    Ok((vault, verifier)) => {
                        let Some(opened) = this.opened.as_mut() else { return };
                        let written = opened
                            .store
                            .init_vault_meta(&salt, params, &verifier)
                            .and_then(|()| opened.store.log_action("init", None));
                        match written {
                            Ok(()) => {
                                opened.vault = Some(vault);
                                this.vault_exists = true;
                                this.create = None;
                                this.password.update(cx, |i, cx| i.clear(cx));
                                this.show_pw = false;
                                this.selected = None;
                                this.filter = Filter::All;
                                match this.reload_items() {
                                    Ok(()) => this.screen = Screen::List,
                                    Err(msg) => this.unlock_error = Some(msg),
                                }
                            }
                            Err(e) => {
                                this.create = Some(CreateForm {
                                    error: Some(format!("写入库元数据失败：{e}")),
                                    ..this.create.take().unwrap_or_else(|| CreateForm {
                                        pw: this.new_create_pw(cx),
                                        pw2: this.new_create_pw(cx),
                                        error: None,
                                    })
                                });
                            }
                        }
                    }
                    Err(msg) => {
                        if let Some(form) = this.create.as_mut() {
                            form.error = Some(msg);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create_view(&self, t: &Theme, window: &Window, cx: &Context<Self>) -> Div {
        let Some(form) = self.create.as_ref() else {
            return self.unlock_view(t, window, cx);
        };

        let titlebar = div()
            .flex_none()
            .h(px(theme::TITLEBAR_H))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .border_b_1()
            .border_color(t.border_soft)
            .child(
                icon_btn("btn-cancel-create", "icons/back.svg", t.text_2, t.hover).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.touch();
                        this.create = None;
                        this.screen = Screen::Unlock;
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child("创建密码库"),
            );

        let submit_label = if self.creating { "派生密钥中…" } else { "创建并解锁" };
        let submit = div()
            .id("btn-create-submit")
            .h(px(44.0))
            .w_full()
            .rounded(px(theme::RADIUS_MD))
            .when(self.creating, |s| s.bg(t.muted))
            .when(
                !self.creating,
                |s| {
                    s.bg(gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(t.primary, 0.0),
                        gpui::linear_color_stop(t.primary_strong, 1.0),
                    ))
                    .shadow(vec![t.sh_glow.clone(), t.sh_highlight.clone()])
                },
            )
            .flex()
            .items_center()
            .justify_center()
            .gap(px(7.0))
            .cursor_pointer()
            .active(|s| s.opacity(0.85))
            .child(
                gpui::svg()
                    .path("icons/shield.svg")
                    .w(px(14.0))
                    .h(px(14.0))
                    .text_color(t.on_primary),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.on_primary)
                    .child(submit_label),
            )
            .on_click(cx.listener(|this, _, _, cx| this.start_create(cx)));

        let body = div()
            .id("create-body")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .on_scroll_wheel(cx.listener(|this, _, _, _cx| this.touch()))
            .px(px(32.0))
            .py(px(18.0))
            .flex()
            .flex_col()
            .justify_center()
            .child(
                // 零知识警示：建库前必须理解「丢了就没了」
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(theme::RADIUS_MD))
                    .bg(rgba(0xDB5461_1A))
                    .border_1()
                    .border_color(rgba(0xDB5461_66))
                    .child(
                        div()
                            .mt(px(5.0))
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(t.danger)
                            .flex_none(),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(t.text_2)
                            .child(
                                "主密码是唯一钥匙：数据在本机加密，谁也无法帮你找回。\
                                 请记牢，或先写下并存放在安全的地方。",
                            ),
                    ),
            )
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(edit_label(t, "设置主密码", Some("至少 8 个字符")))
                    .child(form.pw.clone())
                    .child(strength_bar(t, data::strength_of(form.pw.read(cx).text())))
                    .child(edit_label(t, "确认主密码", None))
                    .child(form.pw2.clone()),
            )
            .children(form.error.as_ref().map(|msg| {
                div()
                    .mt(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(t.danger))
                    .child(div().text_size(px(11.0)).text_color(t.danger).child(msg.clone()))
            }))
            .child(div().mt(px(18.0)).child(submit))
            .child(
                div()
                    .mt(px(14.0))
                    .text_size(px(10.0))
                    .text_color(t.muted)
                    .child("密码在本机加密保存"),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(titlebar)
            .child(body)
    }
}

// ================================================================ 设置态（对齐 security 画板卡片结构）

/// 设置卡：彩色小图标 + 标题的头 + 内容行（调用方链式追加）。
fn settings_card(
    t: &Theme,
    icon_path: &'static str,
    icon_bg: gpui::Rgba,
    icon_ink: gpui::Rgba,
    title: &'static str,
) -> Div {
    div()
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(theme::RADIUS_MD))
        .bg(t.card_sunken)
        .border_1()
        .border_color(t.border_soft)
        .shadow(vec![t.sh_sunken.clone()])
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            // 卡头：22×22 彩色图标 + 标题
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .min_h(px(24.0))
                .child(
                    div()
                        .w(px(22.0))
                        .h(px(22.0))
                        .rounded(px(7.0))
                        .bg(icon_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            gpui::svg()
                                .path(icon_path)
                                .w(px(13.0))
                                .h(px(13.0))
                                .text_color(icon_ink),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(title),
                ),
        )
}

/// 设置行：左侧标签 + 右侧控件。
fn set_row(label: &'static str, control: impl IntoElement, t: &Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_h(px(26.0))
        .child(
            div()
                .flex_1()
                .text_size(px(11.0))
                .text_color(t.text_2)
                .child(label),
        )
        .child(control)
}

/// 卡内说明小字。
fn cap(text: &'static str, t: &Theme) -> Div {
    div()
        .px(px(2.0))
        .text_size(px(10.0))
        .text_color(t.muted)
        .child(text)
}

/// 分段控件（互斥选项；对齐画板 seg-ctl：凹轨 + 选中钮凸起）。
fn seg_ctl(
    t: &Theme,
    cx: &Context<AppRoot>,
    id: &'static str,
    options: &'static [&'static str],
    active: usize,
    apply: fn(&mut AppRoot, usize, &mut Context<AppRoot>),
) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded_full()
        .bg(t.card)
        .border_1()
        .border_color(t.border_soft)
        .shadow(vec![t.sh_sunken.clone()])
        .children(options.iter().enumerate().map(|(i, label)| {
            let on = i == active;
            div()
                .id(ElementId::Name(SharedString::from(format!("{id}-{i}"))))
                .px(px(9.0))
                .py(px(2.0))
                .rounded_full()
                .bg(if on { t.card_strong } else { rgba(0x000000_00) })
                .border_1()
                .border_color(if on { t.border } else { rgba(0x000000_00) })
                .when(on, |s| s.shadow(vec![t.sh_raised.clone(), t.sh_highlight.clone()]))
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(move |s| s.opacity(0.9))
                .on_click(cx.listener(move |this, _, _, cx| apply(this, i, cx)))
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if on { t.primary_strong } else { t.text_2 })
                        .child(*label),
                )
        }))
}

/// 开关（对齐画板 switch）。
fn switch_ctl(
    t: &Theme,
    cx: &Context<AppRoot>,
    id: &'static str,
    on: bool,
    apply: fn(&mut AppRoot, bool, &mut Context<AppRoot>),
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(32.0))
        .h(px(18.0))
        .rounded_full()
        .bg(if on { t.primary } else { t.border })
        .shadow(vec![t.sh_raised.clone()])
        .relative()
        .flex_none()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| apply(this, !on, cx)))
        .child(if on {
            div()
                .absolute()
                .top(px(2.0))
                .right(px(2.0))
                .w(px(14.0))
                .h(px(14.0))
                .rounded_full()
                .bg(rgba(0xFFFFFF_FF))
                .shadow(vec![t.sh_knob.clone()])
        } else {
            div()
                .absolute()
                .top(px(2.0))
                .left(px(2.0))
                .w(px(14.0))
                .h(px(14.0))
                .rounded_full()
                .bg(rgba(0xFFFFFF_FF))
                .shadow(vec![t.sh_knob.clone()])
        })
}

/// 信息行（本机库卡：标签 + 右侧值）。
fn info_row(label: &'static str, value: String, t: &Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_h(px(26.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(11.0))
                .text_color(t.text_2)
                .child(label),
        )
        .child(
            div()
                .max_w(px(200.0))
                .text_size(px(10.0))
                .text_color(t.muted)
                .child(SharedString::from(value)),
        )
}

// —— 选项落地动作（fn 指针供 seg/switch 复用）——
fn apply_close_action(this: &mut AppRoot, idx: usize, cx: &mut Context<AppRoot>) {
    this.touch();
    this.close_minimizes = idx == 1;
    this.save_config();
    cx.notify();
}
fn apply_autolock(this: &mut AppRoot, idx: usize, cx: &mut Context<AppRoot>) {
    this.touch();
    this.autolock_minutes = [5, 10, 15, 30][idx];
    this.save_config();
    cx.notify();
}
fn apply_clip_autoclear(this: &mut AppRoot, on: bool, cx: &mut Context<AppRoot>) {
    this.touch();
    this.clip_autoclear = on;
    this.save_config();
    cx.notify();
}
fn apply_pw_reveal(this: &mut AppRoot, idx: usize, cx: &mut Context<AppRoot>) {
    this.touch();
    this.pw_reveal_secs = [10, 20, 60, 0][idx];
    this.save_config();
    cx.notify();
}
fn apply_theme(this: &mut AppRoot, idx: usize, cx: &mut Context<AppRoot>) {
    this.touch();
    this.dark = idx == 1;
    this.restyle_inputs(cx);
    cx.notify();
}

impl AppRoot {
    fn settings_view(&self, t: &Theme, cx: &Context<Self>) -> Div {
        // KDF 参数：句柄常驻，meta 未加密，锁定态也读得到
        let kdf_line = self
            .opened
            .as_ref()
            .and_then(|o| o.store.read_vault_meta().ok().flatten())
            .map(|m| {
                format!(
                    "Argon2id · {} MiB · {} 次",
                    m.kdf_params.memory_kib / 1024,
                    m.kdf_params.iterations
                )
            })
            .unwrap_or_else(|| "（库未初始化）".to_string());

        let titlebar = div()
            .flex_none()
            .h(px(theme::TITLEBAR_H))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                icon_btn("btn-back-settings", "icons/back.svg", t.text_2, t.hover).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.touch();
                        this.screen = Screen::List;
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child("设置"),
            );

        let close_seg = seg_ctl(
            t,
            cx,
            "seg-close",
            &["直接退出", "最小化"],
            if self.close_minimizes { 1 } else { 0 },
            apply_close_action,
        );
        let autolock_seg = seg_ctl(
            t,
            cx,
            "seg-autolock",
            &["5", "10", "15", "30"],
            [5, 10, 15, 30]
                .iter()
                .position(|m| *m == self.autolock_minutes)
                .unwrap_or(1),
            apply_autolock,
        );
        let theme_seg = seg_ctl(
            t,
            cx,
            "seg-theme",
            &["亮色", "暗色"],
            if self.dark { 1 } else { 0 },
            apply_theme,
        );
        let clip_switch = switch_ctl(
            t,
            cx,
            "sw-clip",
            self.clip_autoclear,
            apply_clip_autoclear,
        );
        let reveal_seg = seg_ctl(
            t,
            cx,
            "seg-reveal",
            &["10 秒", "20 秒", "1 分钟", "常亮"],
            [10u64, 20, 60, 0]
                .iter()
                .position(|s| *s == self.pw_reveal_secs)
                .unwrap_or(1),
            apply_pw_reveal,
        );

        let body = div()
            .id("settings-body")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .on_scroll_wheel(cx.listener(|this, _, _, _cx| this.touch()))
            .px(px(16.0))
            .py(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            // 窗口
            .child(
                settings_card(t, "icons/settings.svg", t.lavender_soft, t.lavender, "窗口")
                    .child(set_row("点击关闭按钮", close_seg, t))
                    .child(cap(
                        "选择「最小化」时，关闭窗口会收进程序坞，点按图标即可唤回。",
                        t,
                    )),
            )
            // 自动锁定：分钟数分段控件（5/10/15/30）
            .child(
                settings_card(t, "icons/clock.svg", t.sky_soft, t.sky, "自动锁定")
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .min_h(px(26.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.0))
                                    .text_color(t.text_2)
                                    .child("无操作后自动锁定（分钟）"),
                            )
                            .child(autolock_seg),
                    )
                    .child(cap("锁定后立即清除内存中的密钥与明文。", t)),
            )
            // 剪贴板保护
            .child(
                settings_card(
                    t,
                    "icons/clipboard.svg",
                    t.mint_soft,
                    t.mint,
                    "剪贴板保护",
                )
                .child(set_row("复制密码 30 秒后自动清空", clip_switch, t))
                .child(cap("关闭后，复制的密码将一直留在系统剪贴板。", t)),
            )
            // 密码明文显示时长
            .child(
                settings_card(t, "icons/eye.svg", t.sky_soft, t.sky, "密码显示")
                    .child(set_row("点眼睛后明文自动隐藏", reveal_seg, t))
                    .child(cap("选「常亮」则明文一直显示，需再点眼睛手动隐藏。", t)),
            )
            // 外观
            .child(
                settings_card(t, "icons/sun.svg", t.butter_soft, t.butter, "外观")
                    .child(set_row("主题", theme_seg, t)),
            )
            // 本机库
            .child(
                settings_card(
                    t,
                    "icons/database.svg",
                    t.peach_soft,
                    t.peach,
                    "本机库",
                )
                .child(info_row("库文件", self.db_path.display().to_string(), t))
                .child(info_row("加密参数", kdf_line, t))
                .child(info_row(
                    "条目",
                    format!("{} 条（当前解锁快照）", self.items.len()),
                    t,
                )),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                if ev.keystroke.key == "escape" {
                    this.touch();
                    this.screen = Screen::List;
                    cx.notify();
                }
            }))
            .child(titlebar)
            .child(body)
    }
}

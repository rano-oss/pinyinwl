mod dbus;
mod settings;

use std::collections::HashSet;

use libchinese_core::{ImeEngine, KeyEvent as ImeKeyEvent, KeyResult};
use libpinyin::{parser::Parser, Engine};
use settings::CharacterSet;

use cosmic::app::Core;
use cosmic::iced::core::event::wayland::input_method::{
    InputMethodEvent, InputMethodKeyboardEvent, KeyEvent, Modifiers,
};
use cosmic::iced::event::{self, listen_raw};
use cosmic::iced::keyboard::key::Named;
use cosmic::iced::keyboard::Key;
use cosmic::iced::platform_specific::runtime::wayland::input_method::InputMethodPopupSettings;
use cosmic::iced::platform_specific::shell::wayland::commands::input_method::{
    self, PopupPositionMode,
};
use cosmic::iced::{self, window, Subscription, Task};
use cosmic::iced::{Color, Event, Size};
use cosmic::widget::{self, container, row, text};
use cosmic::Element;

use tokio::sync::mpsc;

type CosmicAction = cosmic::Action<Message>;

/// Wrap a Task<Message> into Task<CosmicAction>
fn wrap(task: Task<Message>) -> Task<CosmicAction> {
    task.map(cosmic::action::app)
}

/// Default data directory for libpinyin model files
const DATA_DIR: &str = "/usr/share/libpinyin/data";

fn main() -> iced::Result {
    env_logger::init();

    // Check for --settings flag
    if std::env::args().any(|arg| arg == "--settings") {
        return settings::run_settings();
    }

    cosmic::app::run::<PinyinWl>(
        cosmic::app::Settings::default()
            .no_main_window(true)
            .exit_on_close(false),
        (),
    )
}

fn resolve_data_dir(character_set: CharacterSet) -> String {
    std::env::var("PINYINWL_DATA_DIR").unwrap_or_else(|_| {
        let suffix = match character_set {
            CharacterSet::Simplified => "simplified",
            CharacterSet::Traditional => "traditional",
        };
        let candidates = [
            format!("{}/{}", DATA_DIR, suffix),
            DATA_DIR.to_string(),
            format!(
                "{}/.local/share/libpinyin/data/{}",
                std::env::var("HOME").unwrap_or_default(),
                suffix
            ),
            format!("../libchinese/data/converted/{}", suffix),
        ];
        for path in &candidates {
            if std::path::Path::new(path).join("lexicon.fst").exists() {
                return path.clone();
            }
        }
        DATA_DIR.to_string()
    })
}

/// Build fuzzy + optional correction rules from the settings PinyinConfig.
fn build_fuzzy_rules(config: &settings::PinyinConfig) -> Vec<String> {
    let mut rules = Vec::new();
    if config.fuzzy_pinyin {
        let pairs: &[(&str, &str, bool)] = &[
            ("zh", "z", config.fuzzy_zh_z),
            ("ch", "c", config.fuzzy_ch_c),
            ("sh", "s", config.fuzzy_sh_s),
            ("l", "n", config.fuzzy_l_n),
            ("l", "r", config.fuzzy_l_r),
            ("f", "h", config.fuzzy_f_h),
            ("g", "k", config.fuzzy_g_k),
            ("an", "ang", config.fuzzy_an_ang),
            ("en", "eng", config.fuzzy_en_eng),
            ("in", "ing", config.fuzzy_in_ing),
            ("ian", "iang", config.fuzzy_ian_iang),
            ("uan", "uang", config.fuzzy_uan_uang),
        ];
        for &(a, b, enabled) in pairs {
            if enabled {
                rules.push(format!("{}={}", a, b));
                rules.push(format!("{}={}", b, a));
            }
        }
    }
    if config.correct_pinyin {
        let corrections: &[(&str, &str, bool)] = &[
            ("ng", "gn", config.correct_gn_ng),
            ("ng", "mg", config.correct_mg_ng),
            ("iu", "iou", config.correct_iou_iu),
            ("ui", "uei", config.correct_uei_ui),
            ("un", "uen", config.correct_uen_un),
            ("ue", "ve", config.correct_ue_ve),
            ("ong", "on", config.correct_on_ong),
        ];
        for &(a, b, enabled) in corrections {
            if enabled {
                rules.push(format!("{}={}:1.5", a, b));
                rules.push(format!("{}={}:1.5", b, a));
            }
        }
        if config.correct_v_u {
            for &(a, b) in &[
                ("ju", "jv"),
                ("qu", "qv"),
                ("xu", "xv"),
                ("yu", "yv"),
                ("nue", "nve"),
                ("lue", "lve"),
            ] {
                rules.push(format!("{}={}:2.0", a, b));
                rules.push(format!("{}={}:2.0", b, a));
            }
        }
    }
    rules
}

/// Handshake for StartOfPreedit popup lock (engine preedit is source of truth).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AnchorPhase {
    /// Preedit empty — popup follows the text cursor.
    #[default]
    Idle,
    /// Caret-at-0 preedit sent; waiting for Done before real caret.
    Probing,
    /// Commit sent; waiting for Done before caret-at-0 re-lock.
    PartialCommitPending,
    Composing,
}

struct PinyinWl {
    core: Core,
    /// The pinyin engine (for candidate generation and user dictionary learning)
    engine: Engine,
    /// The IME engine (session + key processing)
    ime: ImeEngine<Parser>,
    /// Current pinyin config (for detecting changes on reload)
    pinyin_config: settings::PinyinConfig,
    /// Shift key tracking for 中/英 toggle (modifier-only press)
    shift_set: bool,
    /// The window ID of the popup surface
    popup_id: window::Id,
    /// In-process settings window (D-Bus OpenSettings); None when closed.
    settings_window: Option<window::Id>,
    /// Settings form state while the in-process window is open (also mirrors disk).
    settings_config: settings::PinyinConfig,
    /// Status line for user-dict actions in the in-process settings window.
    settings_status: Option<String>,
    anchor_phase: AnchorPhase,
    /// Track raw_codes of key presses that were consumed (filtered),
    /// so we can pair the release filter decision correctly.
    consumed_keys: HashSet<u32>,
}

impl PinyinWl {
    /// Reload config from disk and apply any changes to the running engine.
    fn reload_config(&mut self) {
        let new_config = settings::PinyinConfig::load();
        if self.config_hash(&new_config) == self.config_hash(&self.pinyin_config) {
            return;
        }
        self.apply_config_diff(new_config);
    }

    fn config_hash(&self, config: &settings::PinyinConfig) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        config.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Activate,
    Deactivate,
    KeyPressed(KeyEvent, Key, Modifiers, u32),
    KeyRepeat(KeyEvent, Key, Modifiers, u32),
    KeyReleased(KeyEvent, Key, Modifiers, u32),
    Modifiers(Modifiers),
    Done,
    DbusToggleMode,
    DbusSetPassthrough(bool),
    DbusToggleHalfFullWidth,
    DbusOpenSettings,
    Settings(settings::Msg),
    SettingsWindowClosed(window::Id),
    SettingsOpened,
    ConfigChanged,
}

impl cosmic::Application for PinyinWl {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.pinyinwl.InputMethod";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<CosmicAction>) {
        let pinyin_config = settings::PinyinConfig::load();
        let data_dir = resolve_data_dir(pinyin_config.character_set);
        let engine = Engine::from_data_dir(&data_dir).unwrap_or_else(|e| {
            log::error!("Failed to load pinyin data from {}: {}", data_dir, e);
            log::error!("Set PINYINWL_DATA_DIR to point to a directory containing lexicon.fst + lexicon.dat + word_bigram.dat + word_bigram_words.fst");
            std::process::exit(1);
        });
        let mut ime = ImeEngine::from_arc_with_page_size(
            engine.inner_arc(),
            pinyin_config.candidates_per_page,
        );
        if pinyin_config.default_fullwidth {
            ime.set_fullwidth(true);
        }
        if pinyin_config.select_keys != "123456789" {
            ime.set_select_keys(&pinyin_config.select_keys);
        }
        // Enable emoji candidates if configured
        engine.set_emoji_enabled(pinyin_config.emoji_candidate);
        engine.set_english_enabled(pinyin_config.english_candidate);
        engine.set_sort_by_pinyin_length(pinyin_config.sort_by_pinyin_length);
        engine.set_double_pinyin_scheme(pinyin_config.double_pinyin_scheme.clone());
        // Apply fuzzy+correction rules, auto_suggestion, incomplete, select_keys
        {
            let mut cfg = engine.config_mut();
            cfg.fuzzy = build_fuzzy_rules(&pinyin_config);
            cfg.auto_suggestion = pinyin_config.auto_suggestion;
            cfg.select_keys = pinyin_config.select_keys.clone();
            cfg.incomplete_penalty = if pinyin_config.pinyin_incomplete {
                500
            } else {
                i32::MAX / 4
            };
        }
        // Enable configured addon dictionaries
        for addon_name in &pinyin_config.enabled_addons {
            engine.set_addon_enabled(addon_name, true);
        }
        let settings_config = pinyin_config.clone();
        let mut app = PinyinWl {
            core,
            engine,
            ime,
            pinyin_config,
            shift_set: false,
            popup_id: window::Id::NONE,
            settings_window: None,
            settings_config,
            settings_status: None,
            anchor_phase: AnchorPhase::Idle,
            consumed_keys: HashSet::new(),
        };
        app.publish_mode_status();
        let popup_settings = InputMethodPopupSettings::default();
        app.popup_id = popup_settings.id;
        let task = Task::batch([
            input_method::get_input_method_popup(popup_settings),
            input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
        ]);
        (app, wrap(task))
    }

    fn view(&self) -> Element<'_, Message> {
        // Required by trait but never called in daemon mode
        widget::Space::new().width(0).height(0).into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        if self.settings_window == Some(id) {
            return settings::settings_view(&self.settings_config, self.settings_status.as_deref())
                .map(Message::Settings);
        }
        if !self.popup_visible() {
            return container(
                widget::column![
                    row![text::body(String::new())],
                    row([]),
                    widget::Space::new().width(0).height(0),
                ]
                .spacing(0),
            )
            .padding(0)
            .width(iced::Length::Fixed(0.0))
            .height(iced::Length::Fixed(0.0))
            .into();
        }
        let ctx = self.ime.context();
        let candidates = &ctx.candidates;
        let candidate_cursor = ctx.candidate_cursor;
        let cosmic_theme = cosmic::theme::active();
        let cosmic = cosmic_theme.cosmic();
        let accent = cosmic.accent_color();
        let selected_bg = Color::from(accent);
        let selected_fg = Color::from(cosmic.on_accent_color());
        let normal_fg = Color::from(cosmic.primary(false).on);
        let dim_fg = Color::from(cosmic.primary(false).component.on_disabled);
        let spacing = cosmic.spacing;
        let corner_radius = cosmic.corner_radii.radius_s;
        let candidates_row: Element<'_, Message> = if candidates.is_empty() {
            row([]).into()
        } else {
            row(candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    let is_selected = index == candidate_cursor;
                    let num_label = format!("{}", (index + 1) % 10);
                    let num_color = if is_selected { selected_fg } else { dim_fg };
                    let text_color = if is_selected { selected_fg } else { normal_fg };
                    let content = row![
                        text::body(num_label).class(cosmic::theme::style::Text::Color(num_color)),
                        text::body(candidate.to_string())
                            .class(cosmic::theme::style::Text::Color(text_color)),
                    ]
                    .align_y(iced::Alignment::Center)
                    .spacing(spacing.space_xxxs);
                    let item: Element<'_, Message> = if is_selected {
                        container(content)
                            .padding([spacing.space_xxxs, spacing.space_xs])
                            .class(cosmic::theme::Container::custom(move |_| {
                                container::Style {
                                    background: Some(iced::Background::Color(selected_bg)),
                                    border: iced::Border {
                                        radius: corner_radius.into(),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }
                            }))
                            .into()
                    } else {
                        container(content)
                            .padding([spacing.space_xxxs, spacing.space_xs])
                            .into()
                    };
                    item
                })
                .collect::<Vec<_>>())
            .spacing(spacing.space_xs)
            .into()
        };
        cosmic::widget::autosize::autosize(
            container(candidates_row)
                .padding(spacing.space_xs)
                .width(iced::Length::Shrink)
                .height(iced::Length::Shrink)
                .class(cosmic::theme::Container::Dropdown),
            widget::Id::new("im-popup"),
        )
        .into()
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::SettingsWindowClosed(id))
    }

    fn update(&mut self, message: Message) -> Task<CosmicAction> {
        match message {
            Message::Activate => {
                self.ime.reset();
                self.anchor_phase = AnchorPhase::Idle;
                self.publish_mode_status();
                wrap(input_method::set_popup_position_mode(PopupPositionMode::FollowCursor))
            }
            Message::Deactivate => {
                self.ime.reset();
                self.consumed_keys.clear();
                self.anchor_phase = AnchorPhase::Idle;
                Task::none()
            }
            Message::KeyPressed(key_event, key, _modifiers, serial) => {
                let raw_code = key_event.raw_code;
                if !self.preedit().is_empty() || !self.candidates().is_empty() {
                    let ime_key = match key.as_ref() {
                        Key::Character(c)
                            if c.len() == 1
                                && c.as_bytes()[0] >= b'1'
                                && c.as_bytes()[0] <= b'9' =>
                        {
                            Some(ImeKeyEvent::Number((c.as_bytes()[0] - b'0') as u8))
                        }
                        Key::Character(c) if c == " " => Some(ImeKeyEvent::Space),
                        Key::Named(Named::ArrowDown) => Some(ImeKeyEvent::Down),
                        Key::Named(Named::ArrowUp) => Some(ImeKeyEvent::Up),
                        Key::Named(Named::ArrowLeft) => Some(ImeKeyEvent::Left),
                        Key::Named(Named::ArrowRight) => Some(ImeKeyEvent::Right),
                        Key::Named(Named::PageDown) => Some(ImeKeyEvent::PageDown),
                        Key::Named(Named::PageUp) => Some(ImeKeyEvent::PageUp),
                        Key::Named(Named::Enter) => Some(ImeKeyEvent::Enter),
                        Key::Named(Named::Escape) => Some(ImeKeyEvent::Escape),
                        Key::Named(Named::Backspace) => Some(ImeKeyEvent::Backspace),
                        _ => {
                            if let Some(ch) = key_event.utf8.as_ref().and_then(|s| s.chars().last())
                            {
                                if ch.is_ascii_lowercase() {
                                    Some(ImeKeyEvent::Char(ch))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                    };
                    if let Some(ime_key) = ime_key {
                        let result = self.ime.process_key(ime_key);
                        if result == KeyResult::Handled {
                            self.consumed_keys.insert(raw_code);
                            let cmd = self.process_and_sync();
                            return wrap(Task::batch(vec![            cmd,
                                input_method::filter_key(serial, true),
                            ]));
                        }
                    }
                }
                if self.ime.is_passthrough() {
                    if key == Key::Named(Named::Shift) {
                        self.shift_set = true;
                        self.consumed_keys.insert(raw_code);
                        return wrap(input_method::filter_key(serial, true));
                    } else {
                        self.shift_set = false;
                        self.consumed_keys.remove(&raw_code);
                        return wrap(input_method::filter_key(serial, false));
                    }
                }
                if key == Key::Named(Named::Shift) {
                    self.shift_set = true;
                    self.consumed_keys.insert(raw_code);
                    return wrap(input_method::filter_key(serial, true));
                }
                if let Some(ch) = key_event.utf8.as_ref().and_then(|s| s.chars().last()) {
                    self.shift_set = false;
                    if self.ime.is_fullwidth() && ch.is_ascii() && ch != '\n' && ch != '\r' {
                        let fullwidth = libchinese_core::utils::to_fullwidth(&ch.to_string());
                        self.consumed_keys.insert(raw_code);
                        let cmd = self.send_commit(fullwidth);
                        return wrap(Task::batch(vec![        cmd,
                            input_method::filter_key(serial, true),
                        ]));
                    }
                    if ch.is_ascii_lowercase() {
                        let result = self.ime.process_key(ImeKeyEvent::Char(ch));
                        if result == KeyResult::Handled {
                            self.consumed_keys.insert(raw_code);
                            let cmd = self.process_and_sync();
                            return wrap(Task::batch(vec![            cmd,
                                input_method::filter_key(serial, true),
                            ]));
                        }
                    }
                    self.consumed_keys.remove(&raw_code);
                    wrap(input_method::filter_key(serial, false))
                } else {
                    self.shift_set = false;
                    self.consumed_keys.remove(&raw_code);
                    wrap(input_method::filter_key(serial, false))
                }
            }
            Message::KeyRepeat(_key_event, key, _modifiers, serial) => {
                // Repeats only matter when we have active preedit or candidates
                let consumed = if !self.preedit().is_empty() || !self.candidates().is_empty() {
                    let ime_key = match key.as_ref() {
                        Key::Named(Named::ArrowDown) => Some(ImeKeyEvent::Down),
                        Key::Named(Named::ArrowUp) => Some(ImeKeyEvent::Up),
                        Key::Named(Named::ArrowLeft) => Some(ImeKeyEvent::Left),
                        Key::Named(Named::ArrowRight) => Some(ImeKeyEvent::Right),
                        Key::Named(Named::PageDown) => Some(ImeKeyEvent::PageDown),
                        Key::Named(Named::PageUp) => Some(ImeKeyEvent::PageUp),
                        Key::Named(Named::Backspace) => Some(ImeKeyEvent::Backspace),
                        _ => None,
                    };
                    if let Some(ime_key) = ime_key {
                        let result = self.ime.process_key(ime_key);
                        if result == KeyResult::Handled {
                            let sync = self.process_and_sync();
                            if serial != 0 {
                                return wrap(Task::batch(vec![                sync,
                                    input_method::filter_key(serial, true),
                                ]));
                            }
                            return wrap(sync);
                        }
                    }
                    false
                } else {
                    false
                };
                if serial != 0 {
                    wrap(input_method::filter_key(serial, consumed))
                } else {
                    Task::none()
                }
            }
            Message::KeyReleased(key_event, key, _modifiers, serial) => {
                let raw_code = key_event.raw_code;
                let was_consumed = self.consumed_keys.remove(&raw_code);

                if key == Key::Named(Named::Shift) && self.shift_set {
                    self.shift_set = false;
                    let english = !self.ime.is_passthrough();
                    let cmd = self.set_english_mode(english);
                    wrap(Task::batch(vec![cmd, input_method::filter_key(serial, true)]))
                } else {
                    // Match the release filter to the press filter
                    wrap(input_method::filter_key(serial, was_consumed))
                }
            }
            Message::Modifiers(_modifiers) => Task::none(),
            Message::Done => wrap(self.handle_done()),
            Message::DbusToggleMode => {
                let english = !self.ime.is_passthrough();
                wrap(self.set_english_mode(english))
            }
            Message::DbusSetPassthrough(passthrough) => {
                wrap(self.set_english_mode(passthrough))
            }
            Message::DbusToggleHalfFullWidth => {
                // If preedit is active, commit it first
                let cmd = if !self.preedit().is_empty() {
                    self.ime.process_key(ImeKeyEvent::Enter);
                    wrap(self.process_and_sync())
                } else {
                    Task::none()
                };
                self.ime.toggle_fullwidth();
                self.publish_mode_status();
                cmd
            }
            Message::DbusOpenSettings => {
                if let Some(id) = self.settings_window {
                    return wrap(window::gain_focus(id));
                }
                self.settings_config = settings::PinyinConfig::load();
                self.settings_status = None;
                let (id, open) = window::open(window::Settings {
                    size: Size::new(560.0, 640.0),
                    min_size: Some(Size::new(420.0, 400.0)),
                    exit_on_close_request: true,
                    decorations: true,
                    transparent: false,
                    resizable: true,
                    ..Default::default()
                });
                self.settings_window = Some(id);
                wrap(open.map(|_| Message::SettingsOpened))
            }
            Message::Settings(msg) => {
                self.settings_status = settings::apply_msg(&mut self.settings_config, msg);
                self.apply_live_config(self.settings_config.clone());
                self.publish_mode_status();
                Task::none()
            }
            Message::SettingsOpened => Task::none(),
            Message::SettingsWindowClosed(id) => {
                if self.settings_window == Some(id) {
                    self.settings_window = None;
                    self.settings_status = None;
                }
                wrap(input_method::reset_popup_size())
            }
            Message::ConfigChanged => {
                self.reload_config();
                Task::none()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let wayland_sub = listen_raw(|event, status, _id| match (event.clone(), status) {
            (
                Event::PlatformSpecific(event::PlatformSpecific::Wayland(
                    event::wayland::Event::InputMethod(event),
                )),
                event::Status::Ignored,
            ) => match event {
                InputMethodEvent::Activate => Some(Message::Activate),
                InputMethodEvent::Deactivate => Some(Message::Deactivate),
                InputMethodEvent::Done => Some(Message::Done),
                _ => None,
            },
            (
                Event::PlatformSpecific(event::PlatformSpecific::Wayland(
                    event::wayland::Event::InputMethodKeyboard(event),
                )),
                event::Status::Ignored,
            ) => match event {
                InputMethodKeyboardEvent::Press(key, key_code, modifiers, serial) => {
                    Some(Message::KeyPressed(key, key_code, modifiers, serial))
                }
                InputMethodKeyboardEvent::Release(key, key_code, modifiers, serial) => {
                    Some(Message::KeyReleased(key, key_code, modifiers, serial))
                }
                InputMethodKeyboardEvent::Repeat(key, key_code, modifiers, serial) => {
                    Some(Message::KeyRepeat(key, key_code, modifiers, serial))
                }
                InputMethodKeyboardEvent::Modifiers(modifiers) => {
                    Some(Message::Modifiers(modifiers))
                }
            },
            _ => None,
        });

        let dbus_sub = Subscription::run(dbus::dbus_subscription);
        let config_sub = Subscription::run(config_watcher_subscription);

        Subscription::batch([wayland_sub, dbus_sub, config_sub])
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        // Must stay transparent for every window: iced has a single clear color,
        // and an opaque clear paints the IM popup surface as a solid box that
        // follows the cursor. Settings paints its own opaque fill in-view.
        let cosmic = cosmic::theme::active();
        let cosmic = cosmic.cosmic();
        Some(cosmic::iced::theme::Style {
            background_color: Color::TRANSPARENT,
            text_color: cosmic.on_bg_color().into(),
            icon_color: cosmic.on_bg_color().into(),
        })
    }
}

// Helper methods on PinyinWl
impl PinyinWl {
    fn publish_mode_status(&self) {
        dbus::publish_mode_label(self.mode_status_text());
    }

    fn mode_status_text(&self) -> &str {
        if self.ime.is_passthrough() {
            "英"
        } else if self.ime.is_fullwidth() {
            "全"
        } else {
            "中"
        }
    }

    /// Switch 中/英 (ibus-libpinyin-style).
    ///
    /// Entering 英 with an active preedit commits the raw buffer text, then
    /// passthrough so subsequent keys go to the client. Leaving 英 clears
    /// passthrough so lowercase again forms pinyin.
    fn set_english_mode(&mut self, english: bool) -> Task<Message> {
        if english == self.ime.is_passthrough() {
            self.publish_mode_status();
            return Task::none();
        }

        let commit = if english && !self.preedit().is_empty() {
            // Match ibus-libpinyin: commit raw composition text, then reset.
            let text = self.preedit().to_owned();
            self.anchor_phase = AnchorPhase::Idle;
            Some(Task::batch([
                input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
                input_method::reset_popup_size(),
                self.send_commit(text),
            ]))
        } else if english {
            self.anchor_phase = AnchorPhase::Idle;
            Some(Task::batch([
                input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
                input_method::reset_popup_size(),
                input_method::set_preedit_string(String::new(), 0, 0),
                input_method::commit(),
            ]))
        } else {
            None
        };

        self.ime.set_passthrough(english);
        self.publish_mode_status();

        commit.unwrap_or_else(Task::none)
    }

    /// Apply a full config snapshot to the running engine (in-process settings path).
    fn apply_live_config(&mut self, new_config: settings::PinyinConfig) {
        if self.config_hash(&new_config) == self.config_hash(&self.pinyin_config) {
            self.pinyin_config = new_config;
            return;
        }
        self.apply_config_diff(new_config);
    }

    fn apply_config_diff(&mut self, new_config: settings::PinyinConfig) {
        let old = self.pinyin_config.clone();

        if new_config.character_set != old.character_set {
            let data_dir = resolve_data_dir(new_config.character_set);
            let fst_path = std::path::Path::new(&data_dir).join("lexicon.fst");
            let dat_path = std::path::Path::new(&data_dir).join("lexicon.dat");
            match libchinese_core::Lexicon::load(&fst_path, &dat_path) {
                Ok(lexicon) => self.engine.swap_lexicon(lexicon),
                Err(e) => log::error!(
                    "Failed to load {:?} lexicon: {}",
                    new_config.character_set,
                    e
                ),
            }
        }

        if new_config.candidates_per_page != old.candidates_per_page {
            self.ime.set_page_size(new_config.candidates_per_page);
        }

        if new_config.select_keys != old.select_keys {
            self.ime.set_select_keys(&new_config.select_keys);
            self.engine.config_mut().select_keys = new_config.select_keys.clone();
        }

        if new_config.emoji_candidate != old.emoji_candidate {
            self.engine.set_emoji_enabled(new_config.emoji_candidate);
        }

        if new_config.english_candidate != old.english_candidate {
            self.engine.set_english_enabled(new_config.english_candidate);
        }

        if new_config.sort_by_pinyin_length != old.sort_by_pinyin_length {
            self.engine
                .set_sort_by_pinyin_length(new_config.sort_by_pinyin_length);
        }

        if new_config.double_pinyin_scheme != old.double_pinyin_scheme {
            self.engine
                .set_double_pinyin_scheme(new_config.double_pinyin_scheme.clone());
        }

        if new_config.auto_suggestion != old.auto_suggestion {
            self.engine.config_mut().auto_suggestion = new_config.auto_suggestion;
        }

        {
            let new_fuzzy = build_fuzzy_rules(&new_config);
            let old_fuzzy = build_fuzzy_rules(&old);
            if new_fuzzy != old_fuzzy {
                self.engine.config_mut().fuzzy = new_fuzzy;
            }
        }

        if new_config.pinyin_incomplete != old.pinyin_incomplete {
            self.engine.config_mut().incomplete_penalty = if new_config.pinyin_incomplete {
                500
            } else {
                i32::MAX / 4
            };
        }

        for addon in &old.enabled_addons {
            if !new_config.enabled_addons.contains(addon) {
                self.engine.set_addon_enabled(addon, false);
            }
        }
        for addon in &new_config.enabled_addons {
            if !old.enabled_addons.contains(addon) {
                self.engine.set_addon_enabled(addon, true);
            }
        }

        if new_config.default_fullwidth != old.default_fullwidth {
            self.ime.set_fullwidth(new_config.default_fullwidth);
        }

        self.pinyin_config = new_config;
        self.settings_config = self.pinyin_config.clone();
    }

    fn preedit(&self) -> &str {
        &self.ime.context().preedit_text
    }

    fn candidates(&self) -> &[String] {
        &self.ime.context().candidates
    }

    fn cursor_byte_position(&self) -> i32 {
        self.ime.context().preedit_cursor as i32
    }

    fn take_commit(&mut self) -> Option<String> {
        let ctx = self.ime.context_mut();
        if ctx.has_commit() {
            Some(ctx.take_commit())
        } else {
            None
        }
    }

    fn send_commit(&self, text: String) -> Task<Message> {
        Task::batch([
            input_method::commit_string(text),
            input_method::commit(),
        ])
    }

    fn follow_cursor_sync(&mut self) -> Task<Message> {
        self.anchor_phase = AnchorPhase::Idle;
        Task::batch([
            input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
            input_method::reset_popup_size(),
            input_method::set_preedit_string(String::new(), 0, 0),
            input_method::commit(),
        ])
    }

    fn probe_preedit_at_zero(preedit: String) -> Task<Message> {
        Task::batch([
            input_method::set_preedit_string(preedit, 0, 0),
            input_method::commit(),
        ])
    }

    /// Arm StartOfPreedit and probe with caret at byte 0.
    fn start_preedit_anchor(&mut self, preedit: String) -> Task<Message> {
        self.anchor_phase = AnchorPhase::Probing;
        Task::batch([
            input_method::set_popup_position_mode(PopupPositionMode::StartOfPreedit),
            Self::probe_preedit_at_zero(preedit),
        ])
    }

    fn send_preedit_sync(&self, preedit: String) -> Task<Message> {
        let cursor = self.cursor_byte_position();
        Task::batch([
            input_method::set_preedit_string(preedit, cursor, cursor),
            input_method::commit(),
        ])
    }

    fn handle_done(&mut self) -> Task<Message> {
        match self.anchor_phase {
            AnchorPhase::Probing => {
                self.anchor_phase = AnchorPhase::Composing;
                self.send_preedit_sync(self.preedit().to_owned())
            }
            AnchorPhase::PartialCommitPending => {
                // Mode is still StartOfPreedit; caret-at-0 SetPreedit re-arms/seeds.
                self.anchor_phase = AnchorPhase::Probing;
                Self::probe_preedit_at_zero(self.preedit().to_owned())
            }
            _ => Task::none(),
        }
    }

    fn process_and_sync(&mut self) -> Task<Message> {
        // Auto-commit when exactly one candidate remains (libpinyin-style AutoCommit).
        if self.pinyin_config.auto_commit_single
            && self.candidates().len() == 1
            && !self.preedit().is_empty()
            && !self.ime.context().has_commit()
        {
            let _ = self.ime.process_key(ImeKeyEvent::Number(1));
        }

        if let Some(text) = self.take_commit() {
            self.engine.commit(&text);
            let preedit = self.preedit().to_owned();
            if preedit.is_empty() {
                self.anchor_phase = AnchorPhase::Idle;
                return Task::batch([
                    input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
                    input_method::reset_popup_size(),
                    self.send_commit(text),
                ]);
            }
            // Commit alone first; on Done, caret-at-0 re-locks then real caret.
            self.anchor_phase = AnchorPhase::PartialCommitPending;
            return self.send_commit(text);
        }

        let preedit = self.preedit().to_owned();
        if preedit.is_empty() {
            self.follow_cursor_sync()
        } else if self.anchor_phase == AnchorPhase::Idle {
            self.start_preedit_anchor(preedit)
        } else if matches!(
            self.anchor_phase,
            AnchorPhase::Probing | AnchorPhase::PartialCommitPending
        ) {
            // Handshake in progress; Done will sync the current engine preedit.
            Task::none()
        } else {
            self.send_preedit_sync(preedit)
        }
    }

    fn popup_visible(&self) -> bool {
        !self.candidates().is_empty()
    }
}

/// Subscription that watches pinyin config for changes and emits ConfigChanged.
fn config_watcher_subscription() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(1, async |mut sender| {
        use cosmic::iced::futures::SinkExt;

        let (tx, mut rx) = mpsc::unbounded_channel();

        let _watcher = match cosmic::cosmic_config::Config::new("com.pinyinwl.Settings", 1) {
            Ok(config) => match config.watch(move |_, _| {
                let _ = tx.send(()).ok();
            }) {
                Ok(w) => w,
                Err(e) => {
                    log::error!("Failed to watch pinyin config: {}", e);
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            },
            Err(e) => {
                log::error!("Failed to open pinyin config: {}", e);
                std::future::pending::<()>().await;
                unreachable!()
            }
        };

        while rx.recv().await.is_some() {
            let _ = sender.send(Message::ConfigChanged).await;
        }
    })
}

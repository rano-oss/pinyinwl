mod settings;

use std::collections::HashSet;

use libchinese_core::{ImeEngine, KeyEvent as ImeKeyEvent, KeyResult};
use libpinyin::{parser::Parser, Engine};
use settings::CharacterSet;

use cosmic::app::Core;
use cosmic::cosmic_config::{self, ConfigSet};
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
use cosmic::iced::{Color, Event};
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

/// Build fuzzy rules Vec from the settings PinyinConfig.
fn build_fuzzy_rules(config: &settings::PinyinConfig) -> Vec<String> {
    if !config.fuzzy_pinyin {
        return Vec::new();
    }
    let mut rules = Vec::new();
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
    rules
}

/// Whether a composition segment is active (popup anchored at preedit start).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AnchorPhase {
    /// Preedit empty — popup follows the text cursor.
    #[default]
    Idle,
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
    /// Shift key tracking for toggle
    shift_set: bool,
    /// Whether in passthrough (English) mode
    passthrough_mode: bool,
    /// cosmic-config handle for writing IME status text
    config_handler: Option<cosmic_config::Config>,
    /// The window ID of the popup surface
    popup_id: window::Id,
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
            return; // No changes
        }
        let old = &self.pinyin_config;

        // Character set changed -> swap lexicon
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

        // Candidates per page
        if new_config.candidates_per_page != old.candidates_per_page {
            self.ime.set_page_size(new_config.candidates_per_page);
        }

        // Select keys
        if new_config.select_keys != old.select_keys {
            self.ime.set_select_keys(&new_config.select_keys);
        }

        // Emoji
        if new_config.emoji_candidate != old.emoji_candidate {
            self.engine.set_emoji_enabled(new_config.emoji_candidate);
        }

        // Auto-suggestion
        if new_config.auto_suggestion != old.auto_suggestion {
            self.engine.config_mut().auto_suggestion = new_config.auto_suggestion;
        }

        // Fuzzy pinyin rules
        {
            let new_fuzzy = build_fuzzy_rules(&new_config);
            let old_fuzzy = build_fuzzy_rules(old);
            if new_fuzzy != old_fuzzy {
                self.engine.config_mut().fuzzy = new_fuzzy;
            }
        }

        // Addons: disable removed, enable added
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

        // Fullwidth
        if new_config.default_fullwidth != old.default_fullwidth {
            self.ime.set_fullwidth(new_config.default_fullwidth);
        }

        self.pinyin_config = new_config;
    }

    fn config_hash(&self, config: &settings::PinyinConfig) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        config.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Debug)]
enum Message {
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
        // Apply fuzzy rules and auto_suggestion from config
        {
            let mut cfg = engine.config_mut();
            cfg.fuzzy = build_fuzzy_rules(&pinyin_config);
            cfg.auto_suggestion = pinyin_config.auto_suggestion;
        }
        // Enable configured addon dictionaries
        for addon_name in &pinyin_config.enabled_addons {
            engine.set_addon_enabled(addon_name, true);
        }
        let config_handler = cosmic_config::Config::new("com.system76.CosmicComp", 1)
            .map_err(|e| log::error!("Failed to create cosmic-config handler: {}", e))
            .ok();
        let mut app = PinyinWl {
            core,
            engine,
            ime,
            pinyin_config,
            shift_set: false,
            passthrough_mode: false,
            config_handler,
            popup_id: window::Id::NONE,
            anchor_phase: AnchorPhase::Idle,
            consumed_keys: HashSet::new(),
        };
        app.write_ime_status(app.mode_status_text());
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

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
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
        let normal_fg = Color::from(cosmic.primary.on);
        let dim_fg = Color::from(cosmic.primary.component.on_disabled);
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

    fn update(&mut self, message: Message) -> Task<CosmicAction> {
        match message {
            Message::Activate => {
                self.ime.reset();
                self.anchor_phase = AnchorPhase::Idle;
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
                if self.passthrough_mode {
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
                    self.passthrough_mode = !self.passthrough_mode;
                    self.write_ime_status(self.mode_status_text());
                    wrap(input_method::filter_key(serial, true))
                } else {
                    // Match the release filter to the press filter
                    wrap(input_method::filter_key(serial, was_consumed))
                }
            }
            Message::Modifiers(_modifiers) => Task::none(),
            Message::Done => wrap(self.handle_done()),
            Message::DbusToggleMode => {
                self.passthrough_mode = !self.passthrough_mode;
                self.write_ime_status(self.mode_status_text());
                Task::none()
            }
            Message::DbusSetPassthrough(passthrough) => {
                self.passthrough_mode = passthrough;
                self.write_ime_status(self.mode_status_text());
                Task::none()
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
                self.write_ime_status(self.mode_status_text());
                cmd
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

        let dbus_sub = Subscription::run(dbus_subscription);
        let config_sub = Subscription::run(config_watcher_subscription);

        Subscription::batch([wayland_sub, dbus_sub, config_sub])
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
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
    fn write_ime_status(&self, text: &str) {
        if let Some(ref handler) = self.config_handler {
            if let Err(e) = handler.set("ime_status_text", &text.to_string()) {
                log::error!("Failed to write ime_status_text: {}", e);
            }
        }
    }

    fn mode_status_text(&self) -> &str {
        if self.passthrough_mode {
            "英"
        } else if self.ime.is_fullwidth() {
            "全"
        } else {
            "中"
        }
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
        Task::batch(vec![
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

    fn start_preedit_anchor(&mut self, preedit: String) -> Task<Message> {
        self.anchor_phase = AnchorPhase::Composing;
        let cursor = self.cursor_byte_position();
        Task::batch([
            input_method::set_popup_position_mode(PopupPositionMode::StartOfPreedit),
            input_method::set_preedit_string(preedit, cursor, cursor),
            input_method::commit(),
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
        Task::none()
    }

    fn process_and_sync(&mut self) -> Task<Message> {
        if let Some(text) = self.take_commit() {
            self.engine.commit(&text);
            let preedit = self.preedit().to_owned();
            if preedit.is_empty() {
                self.anchor_phase = AnchorPhase::Idle;
                return Task::batch([
                    input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
                    input_method::reset_popup_size(),
                    input_method::commit_string(text),
                    input_method::commit(),
                ]);
            }
            self.anchor_phase = AnchorPhase::Composing;
            let cursor = self.cursor_byte_position();
            return Task::batch([
                input_method::commit_string(text),
                input_method::set_preedit_string(preedit, cursor, cursor),
                input_method::commit(),
            ]);
        }

        let preedit = self.preedit().to_owned();
        if preedit.is_empty() {
            self.follow_cursor_sync()
        } else if self.anchor_phase == AnchorPhase::Idle {
            self.start_preedit_anchor(preedit)
        } else {
            self.send_preedit_sync(preedit)
        }
    }

    fn popup_visible(&self) -> bool {
        !self.candidates().is_empty()
    }
}

/// D-Bus interface for external control of the IME
struct PinyinWlDbus {
    tx: mpsc::UnboundedSender<Message>,
}

#[zbus::interface(name = "com.pinyinwl.InputMethod1")]
impl PinyinWlDbus {
    async fn toggle_mode(&self) {
        let _ = self.tx.send(Message::DbusToggleMode);
    }

    async fn set_passthrough(&self, passthrough: bool) {
        let _ = self.tx.send(Message::DbusSetPassthrough(passthrough));
    }

    async fn toggle_half_full_width(&self) {
        let _ = self.tx.send(Message::DbusToggleHalfFullWidth);
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

/// Subscription that registers D-Bus service and forwards method calls as Messages
fn dbus_subscription() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(10, async |mut sender| {
        use cosmic::iced::futures::SinkExt;

        let (tx, mut rx) = mpsc::unbounded_channel();

        let dbus_obj = PinyinWlDbus { tx };

        let conn = match zbus::Connection::session().await {
            Ok(conn) => conn,
            Err(e) => {
                log::error!("Failed to connect to D-Bus session bus: {}", e);
                std::future::pending::<()>().await;
                unreachable!()
            }
        };

        if let Err(e) = conn
            .object_server()
            .at("/com/pinyinwl/InputMethod1", dbus_obj)
            .await
        {
            log::error!("Failed to register D-Bus object: {}", e);
            std::future::pending::<()>().await;
            unreachable!()
        }

        if let Err(e) = conn.request_name("com.pinyinwl.InputMethod1").await {
            log::error!("Failed to request D-Bus name: {}", e);
            std::future::pending::<()>().await;
            unreachable!()
        }

        log::info!("D-Bus service registered: com.pinyinwl.InputMethod1");

        while let Some(msg) = rx.recv().await {
            let _ = sender.send(msg).await;
        }
    })
}

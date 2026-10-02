mod dbus;
mod settings;
mod state;
mod view;

use libchinese_core::{KeyEvent as ImeKeyEvent, KeyResult};

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
use cosmic::widget;
use state::{AnchorPhase, InputMethodState};

type CosmicAction = cosmic::Action<Message>;

fn wrap(task: Task<Message>) -> Task<CosmicAction> {
    task.map(cosmic::action::app)
}

fn main() -> iced::Result {
    env_logger::init();

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

struct PinyinWl {
    core: Core,
    state: InputMethodState,
    settings_window: Option<window::Id>,
    settings_config: settings::PinyinConfig,
    /// Status line for user-dict actions in the in-process settings window.
    settings_status: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Activate,
    Deactivate,
    KeyPressed(KeyEvent, Key, Modifiers, u32),
    KeyReleased(KeyEvent, Key, Modifiers, u32),
    Modifiers(Modifiers),
    Done,
    DbusToggleMode,
    DbusSetPassthrough(bool),
    DbusToggleHalfFullWidth,
    DbusToggleCharset,
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
        let settings_config = pinyin_config.clone();
        let mut state = InputMethodState::new(pinyin_config);
        state.publish_mode_status();

        let popup_settings = InputMethodPopupSettings::default();
        state.popup_id = popup_settings.id;
        let task = Task::batch([
            input_method::get_input_method_popup(popup_settings),
            input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
        ]);
        (
            PinyinWl {
                core,
                state,
                settings_window: None,
                settings_config,
                settings_status: None,
            },
            wrap(task),
        )
    }

    fn view(&self) -> cosmic::Element<'_, Message> {
        widget::Space::new().width(0).height(0).into()
    }

    fn view_window(&self, id: window::Id) -> cosmic::Element<'_, Message> {
        if self.settings_window == Some(id) {
            return settings::settings_view(&self.settings_config, self.settings_status.as_deref())
                .map(Message::Settings);
        }
        view::view(&self.state, id)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::SettingsWindowClosed(id))
    }

    fn update(&mut self, message: Message) -> Task<CosmicAction> {
        let state = &mut self.state;
        match message {
            Message::Activate => {
                state.ime.reset();
                state.anchor_phase = AnchorPhase::Idle;
                state.publish_mode_status();
                wrap(input_method::set_popup_position_mode(PopupPositionMode::FollowCursor))
            }
            Message::Deactivate => wrap(apply_switch_im_behavior(state)),
            Message::KeyPressed(key_event, key, modifiers, serial) => {
                // serial == 0: client-side key repeat (same Message as Press).
                if serial == 0 {
                    return handle_key_repeat(state, key, serial);
                }
                handle_key_press(state, key_event, key, modifiers, serial)
            }
            Message::KeyReleased(key_event, key, _modifiers, serial) => {
                let raw_code = key_event.raw_code;
                let was_consumed = state.consumed_keys.remove(&raw_code);

                if key == Key::Named(Named::Shift) && state.shift_set {
                    state.shift_set = false;
                    let english = !state.ime.is_passthrough();
                    let cmd = state.set_english_mode(english);
                    wrap(Task::batch(vec![cmd, input_method::filter_key(serial, true)]))
                } else {
                    wrap(input_method::filter_key(serial, was_consumed))
                }
            }
            Message::Modifiers(_modifiers) => Task::none(),
            Message::Done => wrap(state.handle_done()),
            Message::DbusToggleMode => {
                let english = !state.ime.is_passthrough();
                wrap(state.set_english_mode(english))
            }
            Message::DbusSetPassthrough(passthrough) => wrap(state.set_english_mode(passthrough)),
            Message::DbusToggleHalfFullWidth => {
                let cmd = if !state.preedit().is_empty() {
                    state.ime.process_key(ImeKeyEvent::Enter);
                    wrap(state.process_key_and_sync())
                } else {
                    Task::none()
                };
                state.ime.toggle_fullwidth();
                state.publish_mode_status();
                cmd
            }
            Message::DbusToggleCharset => {
                state.toggle_character_set();
                Task::none()
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
                self.state.apply_live_config(self.settings_config.clone());
                self.state.publish_mode_status();
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
                self.state.reload_config();
                self.settings_config = self.state.pinyin_config.clone();
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
                    Some(Message::KeyPressed(key, key_code, modifiers, serial))
                }
                InputMethodKeyboardEvent::Modifiers(modifiers) => {
                    Some(Message::Modifiers(modifiers))
                }
            },
            _ => None,
        });

        Subscription::batch([
            wayland_sub,
            Subscription::run(dbus::dbus_subscription),
            Subscription::run(config_watcher),
        ])
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

fn handle_key_press(
    state: &mut InputMethodState,
    key_event: KeyEvent,
    key: Key,
    modifiers: Modifiers,
    serial: u32,
) -> Task<CosmicAction> {
    let raw_code = key_event.raw_code;

    // Ctrl+Shift+F: toggle 简/繁 when idle.
    if modifiers.ctrl
        && modifiers.shift
        && !modifiers.alt
        && matches!(key.as_ref(), Key::Character(c) if c.eq_ignore_ascii_case("f"))
        && state.preedit().is_empty()
        && state.candidates().is_empty()
    {
        state.shift_set = false;
        state.consumed_keys.insert(raw_code);
        state.toggle_character_set();
        return wrap(input_method::filter_key(serial, true));
    }

    let composing = !state.preedit().is_empty() || !state.candidates().is_empty();
    if composing {
        let ime_key = selection_number(state, &key, &key_event, modifiers)
            .map(ImeKeyEvent::Number)
            .or_else(|| (!state.candidates().is_empty()).then(|| page_alias(&key)).flatten())
            .or_else(|| InputMethodState::composing_ime_key(&key, false))
            .or_else(|| {
                key_event
                    .utf8
                    .as_ref()
                    .and_then(|s| s.chars().last())
                    .filter(|ch| ch.is_ascii_lowercase())
                    .map(ImeKeyEvent::Char)
            });
        if let Some(ime_key) = ime_key {
            if state.ime.process_key(ime_key) == KeyResult::Handled {
                state.shift_set = false;
                state.consumed_keys.insert(raw_code);
                return wrap(Task::batch([
                    state.process_key_and_sync(),
                    input_method::filter_key(serial, true),
                ]));
            }
        }
    }

    if key == Key::Named(Named::Shift) {
        state.shift_set = true;
        state.consumed_keys.insert(raw_code);
        return wrap(input_method::filter_key(serial, true));
    }

    // Any other key cancels a pending Shift 中/英 toggle. Otherwise Shift+letter
    // (capitals in 英 passthrough, or accidental caps while composing) flips mode
    // when Shift is released.
    state.shift_set = false;

    // Shift+Space toggles fullwidth (ibus-libpinyin style), including in 英 mode.
    if matches!(key.as_ref(), Key::Character(c) if c == " ") && modifiers.shift {
        state.ime.toggle_fullwidth();
        state.publish_mode_status();
        state.consumed_keys.insert(raw_code);
        return wrap(input_method::filter_key(serial, true));
    }

    if let Some(ch) = key_event.utf8.as_ref().and_then(|s| s.chars().last()) {
        // Fullwidth must run even in 英 passthrough — otherwise ASCII never converts.
        if state.ime.is_fullwidth() && ch.is_ascii() && ch != '\n' && ch != '\r' {
            state.consumed_keys.insert(raw_code);
            return wrap(Task::batch([
                state.send_commit(libchinese_core::utils::to_fullwidth(&ch.to_string())),
                input_method::filter_key(serial, true),
            ]));
        }
    }

    if state.ime.is_passthrough() {
        state.consumed_keys.remove(&raw_code);
        return wrap(input_method::filter_key(serial, false));
    }

    if let Some(ch) = key_event.utf8.as_ref().and_then(|s| s.chars().last()) {
        if ch.is_ascii_lowercase()
            && state.ime.process_key(ImeKeyEvent::Char(ch)) == KeyResult::Handled
        {
            state.consumed_keys.insert(raw_code);
            return wrap(Task::batch([
                state.process_key_and_sync(),
                input_method::filter_key(serial, true),
            ]));
        }
        state.consumed_keys.remove(&raw_code);
        wrap(input_method::filter_key(serial, false))
    } else {
        state.consumed_keys.remove(&raw_code);
        wrap(input_method::filter_key(serial, false))
    }
}

fn page_alias(key: &Key) -> Option<ImeKeyEvent> {
    match key.as_ref() {
        Key::Character(c) if c == "," || c == "[" || c == "-" => Some(ImeKeyEvent::PageUp),
        Key::Character(c) if c == "." || c == "]" || c == "=" => Some(ImeKeyEvent::PageDown),
        _ => None,
    }
}

fn selection_number(
    state: &InputMethodState,
    key: &Key,
    key_event: &KeyEvent,
    modifiers: Modifiers,
) -> Option<u8> {
    if state.candidates().is_empty() {
        return None;
    }
    let keys = &state.pinyin_config.select_keys;
    let idx_to_num = |idx: usize| (idx + 1) as u8;

    if state.pinyin_config.use_keypad_as_selection_key {
        const KP_0: u32 = 0xffb0;
        if (KP_0..=KP_0 + 9).contains(&key_event.keysym) {
            let digit = (key_event.keysym - KP_0) as u8;
            let ch = char::from(b'0' + digit);
            if let Some(idx) = keys.find(ch) {
                return Some(idx_to_num(idx));
            }
        }
    }

    if state.pinyin_config.shift_select_candidate && modifiers.shift && !modifiers.ctrl {
        let idx = match key_event.keysym {
            k @ 0x0031..=0x0039 => Some((k - 0x0031) as usize),
            0x0030 => Some(9),
            _ => None,
        };
        if let Some(idx) = idx.filter(|&i| i < keys.chars().count()) {
            return Some(idx_to_num(idx));
        }
        if let Key::Character(c) = key.as_ref() {
            if let Some(ch) = c.chars().next().map(|c| c.to_ascii_lowercase()) {
                if let Some(idx) = keys.find(ch) {
                    return Some(idx_to_num(idx));
                }
            }
        }
    }
    None
}

fn apply_switch_im_behavior(state: &mut InputMethodState) -> Task<Message> {
    state.consumed_keys.clear();
    match state.pinyin_config.switch_im_behavior.as_str() {
        "Keep" => {
            state.anchor_phase = AnchorPhase::Idle;
            Task::none()
        }
        "CommitDefault" if !state.candidates().is_empty() => {
            let _ = state.ime.process_key(ImeKeyEvent::Number(1));
            state.process_key_and_sync()
        }
        behavior => {
            let commit = matches!(behavior, "CommitPreedit" | "CommitDefault")
                .then(|| state.preedit().to_owned())
                .filter(|t| !t.is_empty());
            state.ime.reset();
            state.anchor_phase = AnchorPhase::Idle;
            match commit {
                Some(text) => Task::batch([
                    input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
                    input_method::reset_popup_size(),
                    state.send_commit(text),
                ]),
                None => Task::batch([
                    input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
                    input_method::reset_popup_size(),
                    input_method::set_preedit_string(String::new(), 0, 0),
                    input_method::commit(),
                ]),
            }
        }
    }
}

fn handle_key_repeat(
    state: &mut InputMethodState,
    key: Key,
    serial: u32,
) -> Task<CosmicAction> {
    if !state.preedit().is_empty() || !state.candidates().is_empty() {
        if let Some(ime_key) = InputMethodState::composing_ime_key(&key, true)
            .or_else(|| (!state.candidates().is_empty()).then(|| page_alias(&key)).flatten())
        {
            if state.ime.process_key(ime_key) == KeyResult::Handled {
                let sync = state.process_key_and_sync();
                return if serial != 0 {
                    wrap(Task::batch([sync, input_method::filter_key(serial, true)]))
                } else {
                    wrap(sync)
                };
            }
        }
    }
    if serial != 0 {
        wrap(input_method::filter_key(serial, false))
    } else {
        Task::none()
    }
}

fn config_watcher() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(1, async |mut sender| {
        use cosmic::iced::futures::SinkExt;
        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::unbounded_channel();

        let _watcher =
            match cosmic::cosmic_config::Config::new(settings::CONFIG_NAME, settings::CONFIG_VERSION)
            {
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

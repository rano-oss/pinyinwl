//! Input method state and libpinyin engine lifecycle.

use std::collections::HashSet;

use libpinyin::lexicon::Lexicon;
use libpinyin::path::{self, DataVariant};
use libpinyin::word_bigram::WordBigram;
use libpinyin::{
    CandidateRankMode, CharacterForm, Editor, EditorOptions, Engine, InputPurpose,
    KeyEvent as ImeKeyEvent, LanguageMode,
};

use cosmic::iced::platform_specific::shell::wayland::commands::input_method::{
    self, PopupPositionMode,
};
use cosmic::iced::{window, Task};

use crate::dbus;
use crate::settings::{CharacterSet, PinyinConfig};
use crate::Message;

/// StartOfPreedit probe: caret-at-0 commit, then real caret after IM `Done`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnchorPhase {
    #[default]
    Idle,
    /// Caret-at-0 preedit sent; waiting for Done before real caret.
    Probing,
    Composing,
    /// Partial commit flushed; next Done re-anchors like a fresh composition.
    ReanchorPending,
    /// Re-anchor probe in flight; leave caret at 0 so the latch takes the
    /// start rect (syncing to the end immediately raced and locked the end).
    ReanchorProbing,
}

pub struct InputMethodState {
    pub editor: Editor,
    pub pinyin_config: PinyinConfig,
    /// Pending 中/英 toggle on Shift release (cleared if another key is pressed).
    pub shift_set: bool,
    /// Shift is down for this hold (key and/or modifiers). Prevents Modifiers from
    /// re-arming `shift_set` after Shift+letter cancelled the toggle.
    pub shift_held: bool,
    pub popup_id: window::Id,
    pub anchor_phase: AnchorPhase,
    /// Track raw_codes of key presses that were consumed (filtered).
    pub consumed_keys: HashSet<u32>,
}

impl InputMethodState {
    pub fn new(pinyin_config: PinyinConfig) -> Self {
        let data_dir = resolve_data_dir(pinyin_config.character_set);
        let mut engine = Engine::from_data_dir(&data_dir).unwrap_or_else(|e| {
            log::error!("Failed to load pinyin data from {}: {}", data_dir, e);
            log::error!("Set LIBPINYIN_DATA_DIR to a directory with lexicon.fst + lexicon.dat + word_bigram.dat + word_bigram_words.fst");
            log::error!("If the error is 'Database already open', another pinyinwl holds ~/.pinyin/userdict.redb — set LIBPINYIN_USERDICT (or LIBPINYIN_DATA_DIR, which uses <data_dir>/userdict.redb)");
            std::process::exit(1);
        });
        for addon_name in &pinyin_config.enabled_addons {
            engine.set_addon_enabled(addon_name, true);
        }
        let mut editor = Editor::new(engine);
        editor.set_editor_options(editor_options_from_config(&pinyin_config, false));
        Self {
            editor,
            pinyin_config,
            shift_set: false,
            shift_held: false,
            popup_id: window::Id::NONE,
            anchor_phase: AnchorPhase::Idle,
            consumed_keys: HashSet::new(),
        }
    }

    /// Panel label: 英 / 全 / 繁 / 中 (passthrough → fullwidth → traditional → simplified).
    pub fn mode_status_text(&self) -> &str {
        if self.editor.language_mode() == LanguageMode::English {
            "英"
        } else if self.editor.character_form() == CharacterForm::Fullwidth {
            "全"
        } else if self.pinyin_config.character_set == CharacterSet::Traditional {
            "繁"
        } else {
            "中"
        }
    }

    pub fn publish_mode_status(&self) {
        let english = self.editor.language_mode() == LanguageMode::English;
        ime_control::publish_mode_label(self.mode_status_text());
        ime_control::publish_menu(dbus::menu_for_state(
            english,
            self.pinyin_config.character_set,
        ));
    }

    pub fn reload_config(&mut self) {
        let new_config = PinyinConfig::load();
        if self.config_hash(&new_config) == self.config_hash(&self.pinyin_config) {
            return;
        }
        self.apply_config_diff(new_config);
    }

    fn config_hash(&self, config: &PinyinConfig) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        config.hash(&mut hasher);
        hasher.finish()
    }

    /// Switch 中/英 (ibus-libpinyin-style).
    pub fn set_english_mode(&mut self, english: bool) -> Task<Message> {
        if english == (self.editor.language_mode() == LanguageMode::English) {
            self.publish_mode_status();
            return Task::none();
        }

        let commit = if english && !self.preedit().is_empty() {
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

        self.editor.set_language_mode(if english { LanguageMode::English } else { LanguageMode::Chinese });
        self.publish_mode_status();

        commit.unwrap_or_else(Task::none)
    }

    pub fn apply_live_config(&mut self, new_config: PinyinConfig) {
        if self.config_hash(&new_config) == self.config_hash(&self.pinyin_config) {
            self.pinyin_config = new_config;
            return;
        }
        self.apply_config_diff(new_config);
    }

    fn apply_config_diff(&mut self, new_config: PinyinConfig) {
        let charset_changed = new_config.character_set != self.pinyin_config.character_set;
        let old_addons = self.pinyin_config.enabled_addons.clone();
        self.pinyin_config = new_config;

        if charset_changed {
            let data_dir = resolve_data_dir(self.pinyin_config.character_set);
            let dir = std::path::Path::new(&data_dir);
            match Lexicon::load(&dir.join("lexicon.fst"), &dir.join("lexicon.dat")) {
                Ok(lex) => self.editor.engine.swap_lexicon(lex),
                Err(e) => log::error!(
                    "Failed to load {:?} lexicon: {}",
                    self.pinyin_config.character_set,
                    e
                ),
            }
            match WordBigram::load(
                &dir.join("word_bigram.dat"),
                &dir.join("word_bigram_words.fst"),
            ) {
                Ok(wb) => self.editor.engine.swap_word_bigram(wb),
                Err(e) => log::error!(
                    "Failed to load {:?} word_bigram: {}",
                    self.pinyin_config.character_set,
                    e
                ),
            }
        }

        let c = &self.pinyin_config;
        let mut opts = editor_options_from_config(c, self.editor.language_mode() == LanguageMode::English);
        opts.input_purpose = self.editor.context.input_purpose;
        self.editor.set_editor_options(opts);

        for addon in &old_addons {
            if !c.enabled_addons.contains(addon) {
                self.editor.engine.set_addon_enabled(addon, false);
            }
        }
        for addon in &c.enabled_addons {
            if !old_addons.contains(addon) {
                self.editor.engine.set_addon_enabled(addon, true);
            }
        }
    }

    /// Toggle 简/繁 at runtime (hotkey / applet menu).
    pub fn toggle_character_set(&mut self) {
        let next = match self.pinyin_config.character_set {
            CharacterSet::Simplified => CharacterSet::Traditional,
            CharacterSet::Traditional => CharacterSet::Simplified,
        };
        let mut cfg = self.pinyin_config.clone();
        cfg.character_set = next;
        cfg.save();
        self.apply_config_diff(cfg);
        self.publish_mode_status();
    }

    pub fn set_input_purpose(&mut self, purpose: InputPurpose) {
        self.editor.set_input_purpose(purpose);
    }

    pub fn set_content_type(&mut self, hint: u32, purpose: u32) {
        const HINT_HIDDEN_TEXT: u32 = 0x40;
        const HINT_SENSITIVE_DATA: u32 = 0x80;
        let mapped = match purpose {
            2 | 3 => InputPurpose::Number,
            4 => InputPurpose::Phone,
            5 => InputPurpose::Url,
            6 => InputPurpose::Email,
            8 | 9 => InputPurpose::Password,
            13 => InputPurpose::Terminal,
            _ => InputPurpose::FreeForm,
        };
        let mapped = if hint & (HINT_HIDDEN_TEXT | HINT_SENSITIVE_DATA) != 0 {
            InputPurpose::Password
        } else {
            mapped
        };
        log::info!("content_type hint={hint:#x} raw_purpose={purpose} -> {mapped:?}");
        self.set_input_purpose(mapped);
    }

    pub fn preedit(&self) -> &str {
        &self.editor.context.preedit_text
    }

    pub fn candidates(&self) -> &[String] {
        &self.editor.context.candidates
    }

    pub fn candidate_cursor(&self) -> usize {
        self.editor.context.candidate_cursor
    }

    pub fn selection_keys(&self) -> &str {
        &self.pinyin_config.select_keys
    }

    fn cursor_byte_position(&self) -> i32 {
        let preedit = self.preedit();
        let mut c = self.editor.context.preedit_cursor.min(preedit.len());
        while c > 0 && !preedit.is_char_boundary(c) {
            c -= 1;
        }
        c as i32
    }

    fn take_commit(&mut self) -> Option<String> {
        let ctx = &mut self.editor.context;
        if ctx.has_commit() {
            Some(ctx.take_commit())
        } else {
            None
        }
    }

    pub fn send_commit(&self, text: String) -> Task<Message> {
        input_method::commit_string(text).chain(input_method::commit())
    }

    /// One `done` with commit + remaining preedit at caret 0 (keeps commit on time).
    /// Popup re-anchors on the following IM `Done` via [`AnchorPhase::ReanchorPending`].
    /// Caret stays at 0 through [`AnchorPhase::ReanchorProbing`] so StartOfPreedit
    /// latches the new start; the next key's `sync_preedit` moves the caret.
    fn send_partial_commit(&mut self, text: String, preedit: String) -> Task<Message> {
        self.anchor_phase = AnchorPhase::ReanchorPending;
        input_method::commit_string(text)
            .chain(input_method::set_preedit_string(preedit, 0, 0))
            .chain(input_method::commit())
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
        self.anchor_phase = AnchorPhase::Probing;
        input_method::set_popup_position_mode(PopupPositionMode::StartOfPreedit)
            .chain(input_method::set_preedit_string(preedit, 0, 0))
            .chain(input_method::commit())
    }

    fn sync_preedit(&self) -> Task<Message> {
        let preedit = self.preedit().to_owned();
        if preedit.is_empty() {
            Task::batch([
                input_method::set_preedit_string(String::new(), 0, 0),
                input_method::commit(),
            ])
        } else {
            let cursor = self.cursor_byte_position();
            Task::batch([
                input_method::set_preedit_string(preedit, cursor, cursor),
                input_method::commit(),
            ])
        }
    }

    pub fn handle_done(&mut self) -> Task<Message> {
        match self.anchor_phase {
            AnchorPhase::Probing => {
                self.anchor_phase = AnchorPhase::Composing;
                self.sync_preedit()
            }
            AnchorPhase::ReanchorPending => {
                let preedit = self.preedit().to_owned();
                if preedit.is_empty() {
                    self.follow_cursor_sync()
                } else {
                    // Same probe as a fresh composition, but do not sync caret to
                    // the end on the next Done — that raced the latch onto the end.
                    self.anchor_phase = AnchorPhase::ReanchorProbing;
                    input_method::set_popup_position_mode(PopupPositionMode::StartOfPreedit)
                        .chain(input_method::set_preedit_string(preedit, 0, 0))
                        .chain(input_method::commit())
                }
            }
            AnchorPhase::ReanchorProbing => {
                self.anchor_phase = AnchorPhase::Composing;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub fn process_key_and_sync(&mut self) -> Task<Message> {
        if self.pinyin_config.auto_commit_single
            && matches!(
                self.anchor_phase,
                AnchorPhase::Idle | AnchorPhase::Composing
            )
            && self.candidates().len() == 1
            && !self.preedit().is_empty()
            && !self.editor.context.has_commit()
        {
            let _ = self.editor.process_key(ImeKeyEvent::Number(1));
        }

        if let Some(text) = self.take_commit() {
            self.editor.engine.commit(&text);
            let preedit = self.preedit().to_owned();
            if preedit.is_empty() {
                self.anchor_phase = AnchorPhase::Idle;
                return Task::batch([
                    input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
                    input_method::reset_popup_size(),
                    self.send_commit(text),
                    self.sync_preedit(),
                ]);
            }
            return self.send_partial_commit(text, preedit);
        }

        let preedit = self.preedit().to_owned();
        if preedit.is_empty() {
            // Only push FollowCursor + empty preedit when leaving a composing
            // handshake. Re-sending empty commit on every idle Handled key
            // (suggestion Enter/Backspace, etc.) floods Alacritty's IME path.
            if self.anchor_phase != AnchorPhase::Idle {
                self.follow_cursor_sync()
            } else {
                Task::batch([
                    input_method::set_popup_position_mode(PopupPositionMode::FollowCursor),
                    input_method::reset_popup_size(),
                ])
            }
        } else if self.anchor_phase == AnchorPhase::Idle {
            self.start_preedit_anchor(preedit)
        } else if matches!(
            self.anchor_phase,
            AnchorPhase::Probing | AnchorPhase::ReanchorPending | AnchorPhase::ReanchorProbing
        ) {
            Task::none()
        } else {
            self.sync_preedit()
        }
    }

    pub fn popup_visible(&self) -> bool {
        !self.candidates().is_empty()
    }

    /// Map iced key → IME key while composing.
    /// `nav_only`: arrows/page/backspace only (key repeat).
    /// `arrow_select`: when false, ↑/↓ are not candidate navigation (setting off).
    pub fn composing_ime_key(
        key: &cosmic::iced::keyboard::Key,
        nav_only: bool,
        arrow_select: bool,
    ) -> Option<ImeKeyEvent> {
        use cosmic::iced::keyboard::key::Named;
        use cosmic::iced::keyboard::Key;
        let nav = match key.as_ref() {
            Key::Named(Named::ArrowDown) if arrow_select => Some(ImeKeyEvent::Down),
            Key::Named(Named::ArrowUp) if arrow_select => Some(ImeKeyEvent::Up),
            // ←/→ always move the preedit cursor while composing.
            Key::Named(Named::ArrowLeft) => Some(ImeKeyEvent::Left),
            Key::Named(Named::ArrowRight) => Some(ImeKeyEvent::Right),
            Key::Named(Named::Home) => Some(ImeKeyEvent::Home),
            Key::Named(Named::End) => Some(ImeKeyEvent::End),
            Key::Named(Named::PageDown) => Some(ImeKeyEvent::PageDown),
            Key::Named(Named::PageUp) => Some(ImeKeyEvent::PageUp),
            Key::Named(Named::Backspace) => Some(ImeKeyEvent::Backspace),
            Key::Named(Named::Delete) => Some(ImeKeyEvent::Delete),
            _ => None,
        };
        if nav_only {
            return nav;
        }
        nav.or_else(|| match key.as_ref() {
            Key::Character(c)
                if c.len() == 1 && c.as_bytes()[0] >= b'1' && c.as_bytes()[0] <= b'9' =>
            {
                Some(ImeKeyEvent::Number((c.as_bytes()[0] - b'0') as u8))
            }
            Key::Character(c) if c == " " => Some(ImeKeyEvent::Space),
            Key::Named(Named::Enter) => Some(ImeKeyEvent::Enter),
            Key::Named(Named::Escape) => Some(ImeKeyEvent::Escape),
            Key::Named(Named::Tab) => Some(ImeKeyEvent::Tab),
            _ => None,
        })
    }
}

pub fn resolve_data_dir(character_set: CharacterSet) -> String {
    let variant = match character_set {
        CharacterSet::Simplified => DataVariant::Simplified,
        CharacterSet::Traditional => DataVariant::Traditional,
    };
    path::resolve_data_dir(variant)
        .to_string_lossy()
        .into_owned()
}

/// Map settings to a full [`EditorOptions`] blob (Best UX ranking).
pub fn editor_options_from_config(c: &PinyinConfig, english: bool) -> EditorOptions {
    let mut opts = EditorOptions {
        candidates_per_page: c.candidates_per_page,
        language_mode: if english {
            LanguageMode::English
        } else {
            LanguageMode::Chinese
        },
        emoji_enabled: c.emoji_candidate,
        english_enabled: c.english_candidate,
        ..EditorOptions::default()
    };
    opts.select_keys = if c.select_keys.is_empty() {
        "123456789".to_string()
    } else {
        c.select_keys.clone()
    };
    opts.character_form = if c.default_fullwidth {
        CharacterForm::Fullwidth
    } else {
        CharacterForm::Halfwidth
    };
    opts.chinese_punctuation = c.chinese_punctuation;
    opts.auto_suggestion = c.auto_suggestion;
    opts.min_suggestion_trigger_length = if c.auto_suggestion { 1 } else { 2 };
    opts.learning_enabled = c.learning_enabled;
    opts.v_mode_enabled = c.v_mode_enabled;
    opts.u_mode_enabled = c.u_mode_enabled;
    opts.inline_prediction = c.inline_prediction;
    opts.show_raw_double_pinyin = c.show_raw_double_pinyin;
    opts.choose_char_from_phrase = c.choose_char_from_phrase;
    opts.allow_fuzzy = c.fuzzy_pinyin || c.pinyin_incomplete || c.correct_pinyin;
    opts.sort_by_pinyin_length = c.sort_by_pinyin_length;
    opts.double_pinyin_scheme = c.double_pinyin_scheme.clone();
    opts.emit_composed_sentences = true;
    opts.candidate_rank_mode = CandidateRankMode::MatchedSpanThenScore;
    opts.fuzzy = build_fuzzy_rules(c);
    opts.incomplete_penalty = if c.pinyin_incomplete {
        500
    } else {
        i32::MAX / 4
    };
    opts
}

pub fn build_fuzzy_rules(config: &PinyinConfig) -> Vec<String> {
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

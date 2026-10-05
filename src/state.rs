//! Input method state and libpinyin engine lifecycle.

use std::collections::HashSet;

use libchinese_core::{ImeEngine, KeyEvent as ImeKeyEvent};
use libpinyin::{parser::Parser, Engine};

use cosmic::iced::platform_specific::shell::wayland::commands::input_method::{
    self, PopupPositionMode,
};
use cosmic::iced::{window, Task};

use crate::dbus;
use crate::settings::{CharacterSet, PinyinConfig};
use crate::Message;

/// Default data directory for libpinyin model files
const DATA_DIR: &str = "/usr/share/libpinyin/data";

/// Handshake for StartOfPreedit popup lock (engine preedit is source of truth).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnchorPhase {
    /// Preedit empty — popup follows the text cursor.
    #[default]
    Idle,
    /// Caret-at-0 preedit sent; waiting for Done before real caret.
    Probing,
    /// Commit sent; waiting for Done before caret-at-0 re-lock.
    PartialCommitPending,
    Composing,
}

pub struct InputMethodState {
    pub engine: Engine,
    pub ime: ImeEngine<Parser>,
    pub pinyin_config: PinyinConfig,
    pub shift_set: bool,
    pub popup_id: window::Id,
    pub anchor_phase: AnchorPhase,
    /// Track raw_codes of key presses that were consumed (filtered).
    pub consumed_keys: HashSet<u32>,
}

impl InputMethodState {
    pub fn new(pinyin_config: PinyinConfig) -> Self {
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
        // Profile first (sentences / rank / mixed-input defaults), then fine overrides.
        engine.set_profile(pinyin_config.ime_profile);
        engine.set_emoji_enabled(pinyin_config.emoji_candidate);
        engine.set_english_enabled(pinyin_config.english_candidate);
        engine.set_sort_by_pinyin_length(pinyin_config.sort_by_pinyin_length);
        engine.set_double_pinyin_scheme(pinyin_config.double_pinyin_scheme.clone());
        engine.set_allow_fuzzy(
            pinyin_config.fuzzy_pinyin
                || pinyin_config.pinyin_incomplete
                || pinyin_config.correct_pinyin,
        );
        {
            let mut cfg = engine.config_mut();
            cfg.fuzzy = build_fuzzy_rules(&pinyin_config);
            cfg.auto_suggestion = pinyin_config.auto_suggestion;
            cfg.learning_enabled = pinyin_config.learning_enabled;
            cfg.chinese_punctuation = pinyin_config.chinese_punctuation;
            cfg.v_mode_enabled = pinyin_config.v_mode_enabled;
            cfg.u_mode_enabled = pinyin_config.u_mode_enabled;
            cfg.inline_prediction = pinyin_config.inline_prediction;
            cfg.select_keys = pinyin_config.select_keys.clone();
            cfg.incomplete_penalty = if pinyin_config.pinyin_incomplete {
                500
            } else {
                i32::MAX / 4
            };
            cfg.show_raw_double_pinyin = pinyin_config.show_raw_double_pinyin;
            cfg.choose_char_from_phrase = pinyin_config.choose_char_from_phrase;
        }
        for addon_name in &pinyin_config.enabled_addons {
            engine.set_addon_enabled(addon_name, true);
        }
        Self {
            engine,
            ime,
            pinyin_config,
            shift_set: false,
            popup_id: window::Id::NONE,
            anchor_phase: AnchorPhase::Idle,
            consumed_keys: HashSet::new(),
        }
    }

    pub fn mode_status_text(&self) -> &str {
        if self.ime.is_passthrough() {
            "英"
        } else if self.ime.is_fullwidth() {
            "全"
        } else if self.pinyin_config.character_set == CharacterSet::Traditional {
            "繁"
        } else {
            "中"
        }
    }

    pub fn publish_mode_status(&self) {
        let english = self.ime.is_passthrough();
        ime_control::publish_mode_label(self.mode_status_text());
        ime_control::publish_menu(dbus::menu_for_state(english, self.pinyin_config.character_set));
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
        if english == self.ime.is_passthrough() {
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

        self.ime.set_passthrough(english);
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
            match libchinese_core::Lexicon::load(&dir.join("lexicon.fst"), &dir.join("lexicon.dat"))
            {
                Ok(lex) => self.engine.swap_lexicon(lex),
                Err(e) => log::error!(
                    "Failed to load {:?} lexicon: {}",
                    self.pinyin_config.character_set,
                    e
                ),
            }
            match libchinese_core::WordBigram::load(
                &dir.join("word_bigram.dat"),
                &dir.join("word_bigram_words.fst"),
            ) {
                Ok(wb) => self.engine.swap_word_bigram(wb),
                Err(e) => log::error!(
                    "Failed to load {:?} word_bigram: {}",
                    self.pinyin_config.character_set,
                    e
                ),
            }
        }

        let c = &self.pinyin_config;
        self.ime.set_page_size(c.candidates_per_page);
        self.ime.set_select_keys(&c.select_keys);
        self.ime.set_fullwidth(c.default_fullwidth);
        {
            let mut cfg = self.engine.config_mut();
            cfg.select_keys = c.select_keys.clone();
            cfg.auto_suggestion = c.auto_suggestion;
            cfg.learning_enabled = c.learning_enabled;
            cfg.chinese_punctuation = c.chinese_punctuation;
            cfg.v_mode_enabled = c.v_mode_enabled;
            cfg.u_mode_enabled = c.u_mode_enabled;
            cfg.inline_prediction = c.inline_prediction;
            cfg.show_raw_double_pinyin = c.show_raw_double_pinyin;
            cfg.fuzzy = build_fuzzy_rules(c);
            cfg.incomplete_penalty = if c.pinyin_incomplete {
                500
            } else {
                i32::MAX / 4
            };
            cfg.choose_char_from_phrase = c.choose_char_from_phrase;
        }
        self.engine.set_profile(c.ime_profile);
        self.engine.set_emoji_enabled(c.emoji_candidate);
        self.engine.set_english_enabled(c.english_candidate);
        self.engine
            .set_sort_by_pinyin_length(c.sort_by_pinyin_length);
        self.engine
            .set_double_pinyin_scheme(c.double_pinyin_scheme.clone());
        self.engine.set_allow_fuzzy(
            c.fuzzy_pinyin || c.pinyin_incomplete || c.correct_pinyin,
        );

        for addon in &old_addons {
            if !c.enabled_addons.contains(addon) {
                self.engine.set_addon_enabled(addon, false);
            }
        }
        for addon in &c.enabled_addons {
            if !old_addons.contains(addon) {
                self.engine.set_addon_enabled(addon, true);
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

    pub fn preedit(&self) -> &str {
        &self.ime.context().preedit_text
    }

    pub fn candidates(&self) -> &[String] {
        &self.ime.context().candidates
    }

    pub fn candidate_cursor(&self) -> usize {
        self.ime.context().candidate_cursor
    }

    pub fn selection_keys(&self) -> &str {
        &self.pinyin_config.select_keys
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

    pub fn send_commit(&self, text: String) -> Task<Message> {
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

    pub fn handle_done(&mut self) -> Task<Message> {
        match self.anchor_phase {
            AnchorPhase::Probing => {
                self.anchor_phase = AnchorPhase::Composing;
                self.send_preedit_sync(self.preedit().to_owned())
            }
            AnchorPhase::PartialCommitPending => {
                self.anchor_phase = AnchorPhase::Probing;
                Self::probe_preedit_at_zero(self.preedit().to_owned())
            }
            _ => Task::none(),
        }
    }

    pub fn process_key_and_sync(&mut self) -> Task<Message> {
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
            Task::none()
        } else {
            self.send_preedit_sync(preedit)
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
            Key::Named(Named::PageDown) => Some(ImeKeyEvent::PageDown),
            Key::Named(Named::PageUp) => Some(ImeKeyEvent::PageUp),
            Key::Named(Named::Backspace) => Some(ImeKeyEvent::Backspace),
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

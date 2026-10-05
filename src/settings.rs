//! Pinyin IME settings window
//!
//! Launched via `pinyinwl --settings`. Uses full libcosmic Application
//! for proper cosmic theme.
//!
//! # Defaults vs ibus / fcitx / Mac / Windows
//!
//! | Knob | Our Best UX default | ibus-libpinyin | fcitx5-pinyin | Mac/Win |
//! |---|---|---|---|---|
//! | Fuzzy AMB master | off | off | AMB pairs off | off / optional |
//! | Corrections | on | on | typo/ue→ve on | autocorrect on |
//! | Incomplete / jianpin | on | on | partial finals | abbreviated on |
//! | Predict after commit | on | suggestion off | Prediction off* | 联想 on |
//! | Cloud | n/a (opt-in later) | off | off | opt-in |
//! | Page `-`/`=` | on | on | on | arrows/pg |
//! | Page `,`/`.` | off | off | — | off |
//! | Page `[`/`]` | off | off | — | off |
//! | Choose char from phrase | on (`[`/`]`) | — | on | 以词定字 |
//! | Learning / forget word | on / Ctrl+7 | dynamic-adjust | Learning / Ctrl+7 | frequency adjust |
//! | Emoji / English | on | on | on | on |
//! | Sentences in bar | on (Best UX) | via sentence API | n-best sentences | composed |
//!
//! \* fcitx enables Prediction on Android/iOS only.
//!
//! Intentionally deferred (rare / contradictory / niche): Lua converter,
//! custom table mode, chaizi, stroke filter, network dictionary timestamps,
//! cloud backends (privacy; optional later).

use std::collections::HashMap;
use std::path::PathBuf;

use cosmic::app::{Core, Task};
use cosmic::iced::{self, Length};
use cosmic::widget::{self, checkbox, container, row, settings, toggler};
use cosmic::{executor, Element};
use serde::{Deserialize, Serialize};

pub const CONFIG_NAME: &str = "com.pinyinwl.Settings";
pub const CONFIG_VERSION: u64 = 1;

const SELECT_KEY_OPTIONS: &[&str] = &["123456789", "asdfghjkl", "qwertyuio"];
const CHARSET_LABELS: &[&str] = &["Simplified", "Traditional"];
/// ibus-libpinyin order: MSPY, ZRM, ABC, ZGPY, PYJJ, XHE (+ None).
const DOUBLE_PINYIN_LABELS: &[&str] = &[
    "None",
    "Microsoft",
    "ZiRanMa",
    "ABC",
    "ZiGuang",
    "PinyinJiaJia",
    "XiaoHe",
];
const IME_PROFILE_LABELS: &[&str] = &["Best UX", "libpinyin compat"];
const IME_PROFILE_VALUES: &[libpinyin::ImeProfile] = &[
    libpinyin::ImeProfile::BestUx,
    libpinyin::ImeProfile::LibpinyinCompat,
];

/// Available addon dictionary names (shipped with the data package).
const AVAILABLE_ADDONS: &[&str] = &[
    "art",
    "culture",
    "economy",
    "geology",
    "history",
    "life",
    "nature",
    "people",
    "science",
    "society",
    "sport",
    "technology",
];

/// Character set for candidate generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CharacterSet {
    Simplified,
    Traditional,
}

impl Default for CharacterSet {
    fn default() -> Self {
        Self::Simplified
    }
}

/// Pinyin settings stored in cosmic-config
#[derive(Debug, Clone, PartialEq, Hash, Serialize, Deserialize)]
pub struct PinyinConfig {
    // -- General --
    pub candidates_per_page: usize,
    pub default_fullwidth: bool,
    pub auto_commit_single: bool,
    pub select_keys: String,

    // -- Pinyin behavior --
    /// Best UX (default) vs closer C/C++ libpinyin candidate-bar behaviour.
    #[serde(default)]
    pub ime_profile: libpinyin::ImeProfile,
    pub pinyin_incomplete: bool,
    pub sort_by_pinyin_length: bool,
    pub auto_suggestion: bool,
    /// Learn from commits (ibus dynamic-adjust / fcitx Learning). Default on.
    #[serde(default = "default_true")]
    pub learning_enabled: bool,
    /// Chinese punctuation in 中 mode (`.` → `。`). Default on.
    #[serde(default = "default_true")]
    pub chinese_punctuation: bool,
    /// Windows-style `v` numeral/date helpers. Default on.
    #[serde(default = "default_true")]
    pub v_mode_enabled: bool,
    /// Windows-style `u` symbol helpers. Default on.
    #[serde(default = "default_true")]
    pub u_mode_enabled: bool,
    /// Predictions in the candidate bar while composing. Default on.
    #[serde(default = "default_true")]
    pub inline_prediction: bool,
    pub emoji_candidate: bool,
    /// Show English word candidates while composing (混输).
    pub english_candidate: bool,
    pub character_set: CharacterSet,

    // -- Candidate UI / navigation (chewingwl parity) --
    #[serde(default = "default_true")]
    pub show_page_number: bool,
    #[serde(default = "default_true")]
    pub vertical_lookup_table: bool,
    #[serde(default = "default_true")]
    pub select_candidate_with_arrow_key: bool,
    #[serde(default)]
    pub use_keypad_as_selection_key: bool,
    /// Clear | Keep | CommitPreedit | CommitDefault
    #[serde(default = "default_switch_im")]
    pub switch_im_behavior: String,
    /// Show raw double-pinyin keys in preedit (e.g. `wd` not `wo'de`).
    #[serde(default)]
    pub show_raw_double_pinyin: bool,
    /// Shift+selection-key commits that candidate (without toggling 中/英).
    /// Default false: matches ibus-libpinyin (Shift alone is 中/英).
    #[serde(default)]
    pub shift_select_candidate: bool,

    /// Page with `-` / `=` (ibus default: on).
    #[serde(default = "default_true")]
    pub minus_equal_page: bool,
    /// Page with `,` / `.` (ibus default: off — keeps Chinese punctuation).
    #[serde(default)]
    pub comma_period_page: bool,
    /// Page with `[` / `]` (ibus default: off).
    #[serde(default)]
    pub square_bracket_page: bool,
    /// 以词定字: `[` first / `]` last char of highlighted phrase (fcitx default on).
    /// Takes priority over bracket paging when the selected candidate has 2+ chars.
    #[serde(default = "default_true")]
    pub choose_char_from_phrase: bool,

    // -- Addon dictionaries --
    /// List of enabled addon dictionary names
    pub enabled_addons: Vec<String>,

    // -- Double pinyin --
    pub double_pinyin_scheme: Option<String>,

    // -- Fuzzy pinyin --
    /// Master switch (ibus/fcitx/Mac/Windows: off by default).
    pub fuzzy_pinyin: bool,
    pub fuzzy_zh_z: bool,
    pub fuzzy_ch_c: bool,
    pub fuzzy_sh_s: bool,
    pub fuzzy_l_n: bool,
    pub fuzzy_l_r: bool,
    pub fuzzy_f_h: bool,
    pub fuzzy_g_k: bool,
    pub fuzzy_an_ang: bool,
    pub fuzzy_en_eng: bool,
    pub fuzzy_in_ing: bool,
    pub fuzzy_ian_iang: bool,
    pub fuzzy_uan_uang: bool,

    // -- Corrections --
    pub correct_pinyin: bool,
    pub correct_gn_ng: bool,
    pub correct_mg_ng: bool,
    pub correct_iou_iu: bool,
    pub correct_uei_ui: bool,
    pub correct_uen_un: bool,
    pub correct_ue_ve: bool,
    pub correct_v_u: bool,
    pub correct_on_ong: bool,
}

fn default_true() -> bool {
    true
}

fn default_switch_im() -> String {
    "Clear".into()
}

const SWITCH_IM_VALUES: &[&str] = &["Clear", "Keep", "CommitPreedit", "CommitDefault"];
const SWITCH_IM_LABELS: &[&str] = &[
    "Clear",
    "Keep",
    "Commit preedit",
    "Commit default candidate",
];

impl Default for PinyinConfig {
    fn default() -> Self {
        Self {
            candidates_per_page: 9,
            default_fullwidth: false,
            auto_commit_single: false,
            select_keys: "123456789".to_string(),

            ime_profile: libpinyin::ImeProfile::BestUx,
            pinyin_incomplete: true,
            sort_by_pinyin_length: false,
            auto_suggestion: true,
            learning_enabled: true,
            chinese_punctuation: true,
            v_mode_enabled: true,
            u_mode_enabled: true,
            inline_prediction: true,
            emoji_candidate: true,
            english_candidate: true,
            character_set: CharacterSet::Simplified,

            show_page_number: true,
            vertical_lookup_table: true,
            select_candidate_with_arrow_key: true,
            use_keypad_as_selection_key: false,
            switch_im_behavior: "Clear".to_string(),
            show_raw_double_pinyin: false,
            shift_select_candidate: false,
            minus_equal_page: true,
            comma_period_page: false,
            square_bracket_page: false,
            choose_char_from_phrase: true,

            enabled_addons: Vec::new(),

            double_pinyin_scheme: None,

            // Fuzzy master off; when enabled, pair defaults match ibus-libpinyin
            // (g↔k and l↔r off — they hurt more than they help for most users).
            fuzzy_pinyin: false,
            fuzzy_zh_z: true,
            fuzzy_ch_c: true,
            fuzzy_sh_s: true,
            fuzzy_l_n: true,
            fuzzy_l_r: false,
            fuzzy_f_h: true,
            fuzzy_g_k: false,
            fuzzy_an_ang: true,
            fuzzy_en_eng: true,
            fuzzy_in_ing: true,
            // fcitx extras (not in ibus); useful when fuzzy is on, default on.
            fuzzy_ian_iang: true,
            fuzzy_uan_uang: true,

            correct_pinyin: true,
            correct_gn_ng: true,
            correct_mg_ng: true,
            correct_iou_iu: true,
            correct_uei_ui: true,
            correct_uen_un: true,
            correct_ue_ve: true,
            correct_v_u: true,
            correct_on_ong: true,
        }
    }
}

impl PinyinConfig {
    pub fn load() -> Self {
        let config = match cosmic::cosmic_config::Config::new(CONFIG_NAME, CONFIG_VERSION) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        use cosmic::cosmic_config::ConfigGet;
        let mut cfg: Self = config.get("settings").unwrap_or_default();
        if !SWITCH_IM_VALUES.contains(&cfg.switch_im_behavior.as_str()) {
            cfg.switch_im_behavior = default_switch_im();
        }
        cfg
    }

    pub fn save(&self) {
        let config = match cosmic::cosmic_config::Config::new(CONFIG_NAME, CONFIG_VERSION) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to open config for save: {}", e);
                return;
            }
        };
        use cosmic::cosmic_config::ConfigSet;
        if let Err(e) = config.set("settings", self) {
            log::error!("Failed to save settings: {}", e);
        }
    }
}

// --- Settings Window App ---

struct SettingsApp {
    core: Core,
    config: PinyinConfig,
    status_message: Option<String>,
    phrase_draft: String,
}

#[derive(Debug, Clone)]
pub enum Msg {
    // General
    ToggleDefaultFullwidth(bool),
    ToggleAutoCommitSingle(bool),
    SetSelectKeys(String),
    SetCandidatesPerPage(usize),
    // Behavior
    SetImeProfile(libpinyin::ImeProfile),
    TogglePinyinIncomplete(bool),
    ToggleSortByPinyinLength(bool),
    ToggleAutoSuggestion(bool),
    ToggleLearningEnabled(bool),
    ToggleChinesePunctuation(bool),
    ToggleVMode(bool),
    ToggleUMode(bool),
    ToggleInlinePrediction(bool),
    ToggleEmojiCandidate(bool),
    // Custom phrases
    CustomPhraseInput(String),
    AddCustomPhrase,
    DeleteCustomPhrase(String),
    ToggleEnglishCandidate(bool),
    SetCharacterSet(CharacterSet),
    ToggleShowPageNumber(bool),
    ToggleVerticalLookupTable(bool),
    ToggleSelectWithArrowKey(bool),
    ToggleKeypadAsSelection(bool),
    SetSwitchImBehavior(String),
    ToggleShowRawDoublePinyin(bool),
    ToggleShiftSelectCandidate(bool),
    ToggleMinusEqualPage(bool),
    ToggleCommaPeriodPage(bool),
    ToggleSquareBracketPage(bool),
    ToggleChooseCharFromPhrase(bool),
    // Addons
    ToggleAddon(String, bool),
    // Double pinyin
    SetDoublePinyinScheme(Option<String>),
    // Fuzzy
    ToggleFuzzyPinyin(bool),
    ToggleFuzzyZhZ(bool),
    ToggleFuzzyChC(bool),
    ToggleFuzzyShS(bool),
    ToggleFuzzyLN(bool),
    ToggleFuzzyLR(bool),
    ToggleFuzzyFH(bool),
    ToggleFuzzyGK(bool),
    ToggleFuzzyAnAng(bool),
    ToggleFuzzyEnEng(bool),
    ToggleFuzzyInIng(bool),
    ToggleFuzzyIanIang(bool),
    ToggleFuzzyUanUang(bool),
    // Corrections
    ToggleCorrectPinyin(bool),
    ToggleCorrectGnNg(bool),
    ToggleCorrectMgNg(bool),
    ToggleCorrectIouIu(bool),
    ToggleCorrectUeiUi(bool),
    ToggleCorrectUenUn(bool),
    ToggleCorrectUeVe(bool),
    ToggleCorrectVU(bool),
    ToggleCorrectOnOng(bool),
    // User dictionary
    ExportUserDict,
    ImportUserDict,
    ClearUserDict,
}

fn index_of(options: &[&str], current: &str) -> Option<usize> {
    options.iter().position(|v| *v == current)
}

fn charset_index(cs: CharacterSet) -> Option<usize> {
    match cs {
        CharacterSet::Simplified => Some(0),
        CharacterSet::Traditional => Some(1),
    }
}

fn double_pinyin_index(scheme: &Option<String>) -> Option<usize> {
    match scheme.as_deref() {
        None => Some(0),
        Some(s) => {
            if let Some(i) = DOUBLE_PINYIN_LABELS.iter().position(|v| *v == s) {
                return Some(i);
            }
            // Migrate aliases (ibus codes / old PinYinPlusPlus name).
            libpinyin::DoublePinyinScheme::parse(s)
                .and_then(|sch| {
                    DOUBLE_PINYIN_LABELS
                        .iter()
                        .position(|v| *v == sch.as_str())
                })
                .or(Some(0))
        }
    }
}

impl cosmic::Application for SettingsApp {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Msg;

    const APP_ID: &'static str = "com.pinyinwl.Settings";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = PinyinConfig::load();
        let app = SettingsApp {
            core,
            config,
            status_message: None,
            phrase_draft: String::new(),
        };
        (app, Task::none())
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![widget::text::title3("Pinyin Settings").into()]
    }

    fn update(&mut self, msg: Self::Message) -> Task<Self::Message> {
        match &msg {
            Msg::CustomPhraseInput(s) => {
                self.phrase_draft = s.clone();
                return Task::none();
            }
            Msg::AddCustomPhrase => {
                let phrase = self.phrase_draft.trim().to_string();
                if phrase.is_empty() {
                    self.status_message = Some("Enter a phrase to add.".into());
                } else {
                    self.status_message = Some(add_custom_phrase(&phrase));
                    self.phrase_draft.clear();
                }
                return Task::none();
            }
            Msg::DeleteCustomPhrase(p) => {
                self.status_message = Some(delete_custom_phrase(p));
                return Task::none();
            }
            _ => {}
        }
        if let Some(status) = apply_msg(&mut self.config, msg) {
            self.status_message = Some(status);
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        settings_view(
            &self.config,
            self.status_message.as_deref(),
            &self.phrase_draft,
        )
    }
}

/// Apply a settings message and persist. Returns a status string for user-dict actions.
pub fn apply_msg(config: &mut PinyinConfig, msg: Msg) -> Option<String> {
    let mut status = None;
    match msg {
        Msg::ToggleDefaultFullwidth(v) => config.default_fullwidth = v,
        Msg::ToggleAutoCommitSingle(v) => config.auto_commit_single = v,
        Msg::SetSelectKeys(k) => config.select_keys = k,
        Msg::SetCandidatesPerPage(n) => config.candidates_per_page = n,
        Msg::SetImeProfile(p) => {
            config.ime_profile = p;
            // Sync related toggles to the profile defaults; user can still change them after.
            match p {
                libpinyin::ImeProfile::BestUx => {
                    config.auto_suggestion = true;
                    config.learning_enabled = true;
                    config.chinese_punctuation = true;
                    config.v_mode_enabled = true;
                    config.u_mode_enabled = true;
                    config.inline_prediction = true;
                    config.emoji_candidate = true;
                    config.english_candidate = true;
                }
                libpinyin::ImeProfile::LibpinyinCompat => {
                    config.auto_suggestion = false;
                    config.emoji_candidate = false;
                    config.english_candidate = false;
                }
            }
        }
        Msg::TogglePinyinIncomplete(v) => config.pinyin_incomplete = v,
        Msg::ToggleSortByPinyinLength(v) => config.sort_by_pinyin_length = v,
        Msg::ToggleAutoSuggestion(v) => config.auto_suggestion = v,
        Msg::ToggleLearningEnabled(v) => config.learning_enabled = v,
        Msg::ToggleChinesePunctuation(v) => config.chinese_punctuation = v,
        Msg::ToggleVMode(v) => config.v_mode_enabled = v,
        Msg::ToggleUMode(v) => config.u_mode_enabled = v,
        Msg::ToggleInlinePrediction(v) => config.inline_prediction = v,
        Msg::ToggleEmojiCandidate(v) => config.emoji_candidate = v,
        Msg::CustomPhraseInput(_) | Msg::AddCustomPhrase | Msg::DeleteCustomPhrase(_) => {
            // Handled in SettingsApp with live userdict access.
        }
        Msg::ToggleEnglishCandidate(v) => config.english_candidate = v,
        Msg::SetCharacterSet(cs) => config.character_set = cs,
        Msg::ToggleShowPageNumber(v) => config.show_page_number = v,
        Msg::ToggleVerticalLookupTable(v) => config.vertical_lookup_table = v,
        Msg::ToggleSelectWithArrowKey(v) => config.select_candidate_with_arrow_key = v,
        Msg::ToggleKeypadAsSelection(v) => config.use_keypad_as_selection_key = v,
        Msg::SetSwitchImBehavior(v) => config.switch_im_behavior = v,
        Msg::ToggleShowRawDoublePinyin(v) => config.show_raw_double_pinyin = v,
        Msg::ToggleShiftSelectCandidate(v) => config.shift_select_candidate = v,
        Msg::ToggleMinusEqualPage(v) => config.minus_equal_page = v,
        Msg::ToggleCommaPeriodPage(v) => config.comma_period_page = v,
        Msg::ToggleSquareBracketPage(v) => config.square_bracket_page = v,
        Msg::ToggleChooseCharFromPhrase(v) => config.choose_char_from_phrase = v,
        Msg::ToggleAddon(name, enabled) => {
            if enabled {
                if !config.enabled_addons.contains(&name) {
                    config.enabled_addons.push(name);
                }
            } else {
                config.enabled_addons.retain(|n| *n != name);
            }
        }
        Msg::SetDoublePinyinScheme(s) => config.double_pinyin_scheme = s,
        Msg::ToggleFuzzyPinyin(v) => config.fuzzy_pinyin = v,
        Msg::ToggleFuzzyZhZ(v) => config.fuzzy_zh_z = v,
        Msg::ToggleFuzzyChC(v) => config.fuzzy_ch_c = v,
        Msg::ToggleFuzzyShS(v) => config.fuzzy_sh_s = v,
        Msg::ToggleFuzzyLN(v) => config.fuzzy_l_n = v,
        Msg::ToggleFuzzyLR(v) => config.fuzzy_l_r = v,
        Msg::ToggleFuzzyFH(v) => config.fuzzy_f_h = v,
        Msg::ToggleFuzzyGK(v) => config.fuzzy_g_k = v,
        Msg::ToggleFuzzyAnAng(v) => config.fuzzy_an_ang = v,
        Msg::ToggleFuzzyEnEng(v) => config.fuzzy_en_eng = v,
        Msg::ToggleFuzzyInIng(v) => config.fuzzy_in_ing = v,
        Msg::ToggleFuzzyIanIang(v) => config.fuzzy_ian_iang = v,
        Msg::ToggleFuzzyUanUang(v) => config.fuzzy_uan_uang = v,
        Msg::ToggleCorrectPinyin(v) => config.correct_pinyin = v,
        Msg::ToggleCorrectGnNg(v) => config.correct_gn_ng = v,
        Msg::ToggleCorrectMgNg(v) => config.correct_mg_ng = v,
        Msg::ToggleCorrectIouIu(v) => config.correct_iou_iu = v,
        Msg::ToggleCorrectUeiUi(v) => config.correct_uei_ui = v,
        Msg::ToggleCorrectUenUn(v) => config.correct_uen_un = v,
        Msg::ToggleCorrectUeVe(v) => config.correct_ue_ve = v,
        Msg::ToggleCorrectVU(v) => config.correct_v_u = v,
        Msg::ToggleCorrectOnOng(v) => config.correct_on_ong = v,
        Msg::ExportUserDict => status = Some(export_user_dict()),
        Msg::ImportUserDict => status = Some(import_user_dict()),
        Msg::ClearUserDict => status = Some(clear_user_dict()),
    }
    if status.is_none() {
        config.save();
    }
    status
}

/// Settings form body (shared by standalone `--settings` and in-IME window).
pub fn settings_view<'a>(
    config: &'a PinyinConfig,
    status_message: Option<&'a str>,
    phrase_draft: &'a str,
) -> Element<'a, Msg> {
    let body = build_settings_view(config, status_message, phrase_draft);
    let bg_color = {
        let theme = cosmic::theme::active();
        iced::Color::from(theme.cosmic().bg_color())
    };
    // Full-window opaque fill. App clear color must stay transparent (shared with
    // the IM popup), so settings has to paint every pixel itself.
    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .class(cosmic::theme::Container::custom(move |_| {
            cosmic::widget::container::Style {
                background: Some(iced::Background::Color(bg_color)),
                text_color: None,
                ..Default::default()
            }
        }))
        .into()
}

/// Build the entire settings view from owned config data.
fn build_settings_view(
    config: &PinyinConfig,
    status_message: Option<&str>,
    phrase_draft: &str,
) -> Element<'static, Msg> {
    let candidates = config.candidates_per_page.clamp(5, 10);

    let general_section: Element<'static, Msg> = settings::section()
        .title("General")
        .add(settings::item(
            "Candidate selection keys",
            widget::dropdown(
                SELECT_KEY_OPTIONS,
                index_of(SELECT_KEY_OPTIONS, &config.select_keys),
                |i| Msg::SetSelectKeys(SELECT_KEY_OPTIONS[i].to_string()),
            ),
        ))
        .add(settings::item(
            format!("Candidates per page ({candidates})"),
            widget::slider(5.0..=10.0, candidates as f32, |v| {
                Msg::SetCandidatesPerPage(v.round() as usize)
            })
            .width(Length::Fixed(200.0)),
        ))
        .add(settings::item(
            "Character set",
            widget::dropdown(CHARSET_LABELS, charset_index(config.character_set), |i| {
                Msg::SetCharacterSet(if i == 0 {
                    CharacterSet::Simplified
                } else {
                    CharacterSet::Traditional
                })
            }),
        ))
        .add(settings::item(
            "IME profile",
            widget::dropdown(
                IME_PROFILE_LABELS,
                IME_PROFILE_VALUES
                    .iter()
                    .position(|p| *p == config.ime_profile),
                |i| Msg::SetImeProfile(IME_PROFILE_VALUES[i]),
            ),
        ))
        .add(settings::item(
            "Fullwidth ASCII (Shift+Space toggles while typing)",
            toggler(config.default_fullwidth).on_toggle(Msg::ToggleDefaultFullwidth),
        ))
        .add(settings::item(
            "Auto-commit single candidate",
            toggler(config.auto_commit_single).on_toggle(Msg::ToggleAutoCommitSingle),
        ))
        .add(settings::item(
            "Incomplete pinyin (jianpin)",
            toggler(config.pinyin_incomplete).on_toggle(Msg::TogglePinyinIncomplete),
        ))
        .add(settings::item(
            "Sort by pinyin length",
            toggler(config.sort_by_pinyin_length).on_toggle(Msg::ToggleSortByPinyinLength),
        ))
        .add(settings::item(
            "Predict next words after commit",
            toggler(config.auto_suggestion).on_toggle(Msg::ToggleAutoSuggestion),
        ))
        .add(settings::item(
            "Learn from typing (Ctrl+7 forget, Ctrl+8 pin)",
            toggler(config.learning_enabled).on_toggle(Msg::ToggleLearningEnabled),
        ))
        .add(settings::item(
            "Chinese punctuation in Chinese mode (Ctrl+. toggles punct picker)",
            toggler(config.chinese_punctuation).on_toggle(Msg::ToggleChinesePunctuation),
        ))
        .add(settings::item(
            "v-mode helpers (v123 → 一二三)",
            toggler(config.v_mode_enabled).on_toggle(Msg::ToggleVMode),
        ))
        .add(settings::item(
            "u-mode symbols (uheart → ♥)",
            toggler(config.u_mode_enabled).on_toggle(Msg::ToggleUMode),
        ))
        .add(settings::item(
            "Predictions while composing",
            toggler(config.inline_prediction).on_toggle(Msg::ToggleInlinePrediction),
        ))
        .add(settings::item(
            "Emoji candidates",
            toggler(config.emoji_candidate).on_toggle(Msg::ToggleEmojiCandidate),
        ))
        .add(settings::item(
            "English candidates",
            toggler(config.english_candidate).on_toggle(Msg::ToggleEnglishCandidate),
        ))
        .add(settings::item(
            "Vertical candidate list",
            toggler(config.vertical_lookup_table).on_toggle(Msg::ToggleVerticalLookupTable),
        ))
        .add(settings::item(
            "Show page number (N/M under candidates)",
            toggler(config.show_page_number).on_toggle(Msg::ToggleShowPageNumber),
        ))
        .add(settings::item(
            "Select candidate with ↑/↓ (Tab = tone filter while composing)",
            toggler(config.select_candidate_with_arrow_key)
                .on_toggle(Msg::ToggleSelectWithArrowKey),
        ))
        .add(settings::item(
            "Use keypad as selection keys",
            toggler(config.use_keypad_as_selection_key).on_toggle(Msg::ToggleKeypadAsSelection),
        ))
        .add(settings::item(
            "Shift + number selects candidate",
            toggler(config.shift_select_candidate).on_toggle(Msg::ToggleShiftSelectCandidate),
        ))
        .add(settings::item(
            "Page with - / =",
            toggler(config.minus_equal_page).on_toggle(Msg::ToggleMinusEqualPage),
        ))
        .add(settings::item(
            "Page with , / .",
            toggler(config.comma_period_page).on_toggle(Msg::ToggleCommaPeriodPage),
        ))
        .add(settings::item(
            "Page with [ / ]",
            toggler(config.square_bracket_page).on_toggle(Msg::ToggleSquareBracketPage),
        ))
        .add(settings::item(
            "Choose char from phrase ([ / ])",
            toggler(config.choose_char_from_phrase).on_toggle(Msg::ToggleChooseCharFromPhrase),
        ))
        .add(settings::item(
            "On focus out / switch IM",
            widget::dropdown(
                SWITCH_IM_LABELS,
                index_of(SWITCH_IM_VALUES, &config.switch_im_behavior),
                |i| Msg::SetSwitchImBehavior(SWITCH_IM_VALUES[i].to_string()),
            ),
        ))
        .into();

    let double_pinyin_section: Element<'static, Msg> = settings::section()
        .title("Double Pinyin")
        .add(settings::item(
            "Scheme",
            widget::dropdown(
                DOUBLE_PINYIN_LABELS,
                double_pinyin_index(&config.double_pinyin_scheme),
                |i| {
                    if i == 0 {
                        Msg::SetDoublePinyinScheme(None)
                    } else {
                        Msg::SetDoublePinyinScheme(Some(DOUBLE_PINYIN_LABELS[i].to_string()))
                    }
                },
            ),
        ))
        .add(settings::item(
            "Show raw keys in preedit",
            toggler(config.show_raw_double_pinyin).on_toggle(Msg::ToggleShowRawDoublePinyin),
        ))
        .into();

    // === Fuzzy Pinyin section ===
    let fuzzy_enabled = config.fuzzy_pinyin;
    let mut fuzzy_section = settings::section()
        .title("Fuzzy Pinyin")
        .add(settings::item(
            "Enable fuzzy pinyin",
            toggler(config.fuzzy_pinyin).on_toggle(Msg::ToggleFuzzyPinyin),
        ));

    let fuzzy_rules: &[(&str, bool, fn(bool) -> Msg)] = &[
        ("zh <=> z", config.fuzzy_zh_z, Msg::ToggleFuzzyZhZ),
        ("ch <=> c", config.fuzzy_ch_c, Msg::ToggleFuzzyChC),
        ("sh <=> s", config.fuzzy_sh_s, Msg::ToggleFuzzyShS),
        ("l <=> n", config.fuzzy_l_n, Msg::ToggleFuzzyLN),
        ("l <=> r", config.fuzzy_l_r, Msg::ToggleFuzzyLR),
        ("f <=> h", config.fuzzy_f_h, Msg::ToggleFuzzyFH),
        ("g <=> k", config.fuzzy_g_k, Msg::ToggleFuzzyGK),
        ("an <=> ang", config.fuzzy_an_ang, Msg::ToggleFuzzyAnAng),
        ("en <=> eng", config.fuzzy_en_eng, Msg::ToggleFuzzyEnEng),
        ("in <=> ing", config.fuzzy_in_ing, Msg::ToggleFuzzyInIng),
        (
            "ian <=> iang",
            config.fuzzy_ian_iang,
            Msg::ToggleFuzzyIanIang,
        ),
        (
            "uan <=> uang",
            config.fuzzy_uan_uang,
            Msg::ToggleFuzzyUanUang,
        ),
    ];
    for &(label, value, msg_fn) in fuzzy_rules {
        let t = if fuzzy_enabled {
            toggler(value).on_toggle(msg_fn)
        } else {
            toggler(value)
        };
        fuzzy_section = fuzzy_section.add(settings::item(label, t));
    }
    let fuzzy_section: Element<'static, Msg> = fuzzy_section.into();

    // === Corrections section ===
    let correct_enabled = config.correct_pinyin;
    let mut corrections_section = settings::section().title("Corrections").add(settings::item(
        "Enable pinyin corrections",
        toggler(config.correct_pinyin).on_toggle(Msg::ToggleCorrectPinyin),
    ));

    let correction_rules: &[(&str, bool, fn(bool) -> Msg)] = &[
        ("gn => ng", config.correct_gn_ng, Msg::ToggleCorrectGnNg),
        ("mg => ng", config.correct_mg_ng, Msg::ToggleCorrectMgNg),
        ("iou => iu", config.correct_iou_iu, Msg::ToggleCorrectIouIu),
        ("uei => ui", config.correct_uei_ui, Msg::ToggleCorrectUeiUi),
        ("uen => un", config.correct_uen_un, Msg::ToggleCorrectUenUn),
        ("ue => ve", config.correct_ue_ve, Msg::ToggleCorrectUeVe),
        (
            "v => u (nv => nu)",
            config.correct_v_u,
            Msg::ToggleCorrectVU,
        ),
        ("on => ong", config.correct_on_ong, Msg::ToggleCorrectOnOng),
    ];
    for &(label, value, msg_fn) in correction_rules {
        let t = if correct_enabled {
            toggler(value).on_toggle(msg_fn)
        } else {
            toggler(value)
        };
        corrections_section = corrections_section.add(settings::item(label, t));
    }
    let corrections_section: Element<'static, Msg> = corrections_section.into();

    // === Addon Dictionaries section ===
    let mut addon_section = settings::section().title("Topic Dictionaries");
    for &addon in AVAILABLE_ADDONS {
        let enabled = config.enabled_addons.iter().any(|n| n == addon);
        let name = addon.to_string();
        addon_section = addon_section.add(settings::item(
            addon,
            checkbox(enabled).on_toggle(move |v| Msg::ToggleAddon(name.clone(), v)),
        ));
    }
    let addon_section: Element<'static, Msg> = addon_section.into();

    // === Custom phrases ===
    let draft = phrase_draft.to_string();
    let mut custom_section = settings::section()
        .title("Custom Phrases")
        .add(settings::item(
            "New phrase",
            row![
                widget::text_input("例如：你好世界", draft)
                    .on_input(Msg::CustomPhraseInput)
                    .width(Length::Fixed(200.0)),
                widget::button::suggested("Add").on_press(Msg::AddCustomPhrase),
            ]
            .spacing(8),
        ));
    for (phrase, freq) in list_custom_phrases().into_iter().take(20) {
        let p = phrase.clone();
        custom_section = custom_section.add(settings::item(
            format!("{phrase} ({freq})"),
            widget::button::destructive("Delete")
                .on_press(Msg::DeleteCustomPhrase(p)),
        ));
    }
    let custom_section: Element<'static, Msg> = custom_section.into();

    // === User Dictionary section ===
    let mut user_dict_section = settings::section().title("User Dictionary").add(
        row![
            widget::button::standard("Export").on_press(Msg::ExportUserDict),
            widget::button::standard("Import").on_press(Msg::ImportUserDict),
            widget::button::destructive("Clear All").on_press(Msg::ClearUserDict),
        ]
        .spacing(8),
    );
    if let Some(status) = status_message {
        user_dict_section = user_dict_section.add(widget::text::caption(status.to_string()));
    }
    let user_dict_section: Element<'static, Msg> = user_dict_section.into();

    // === Assemble ===
    let content = settings::view_column(vec![
        general_section,
        double_pinyin_section,
        fuzzy_section,
        corrections_section,
        addon_section,
        custom_section,
        user_dict_section,
    ])
    .padding(20)
    .width(Length::Fill);

    widget::scrollable(
        container(content)
            .width(Length::Fill)
            .height(Length::Shrink),
    )
    .height(Length::Fill)
    .into()
}

pub fn run_settings() -> iced::Result {
    let settings = cosmic::app::Settings::default().size(iced::Size::new(500.0, 600.0));
    cosmic::app::run::<SettingsApp>(settings, ())
}

// --- User Dictionary helpers ---

fn user_dict_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(base).join("pinyinwl")
}

fn user_dict_path() -> PathBuf {
    user_dict_dir().join("userdict.redb")
}

fn user_dict_export_path() -> PathBuf {
    user_dict_dir().join("user_dict_export.ron")
}

fn open_user_dict() -> Result<libchinese_core::UserDict, String> {
    let path = user_dict_path();
    if !path.exists() {
        return Err("User dictionary not found. Start typing to create one.".into());
    }
    libchinese_core::UserDict::new(&path).map_err(|e| format!("Failed to open user dict: {}", e))
}

fn export_user_dict() -> String {
    let ud = match open_user_dict() {
        Ok(ud) => ud,
        Err(e) => return e,
    };
    let data = ud.snapshot();
    if data.is_empty() {
        return "User dictionary is empty, nothing to export.".into();
    }
    let export_path = user_dict_export_path();
    if let Some(parent) = export_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match ron::ser::to_string_pretty(&data, ron::ser::PrettyConfig::default()) {
        Ok(content) => match std::fs::write(&export_path, content) {
            Ok(_) => format!(
                "Exported {} entries to {}",
                data.len(),
                export_path.display()
            ),
            Err(e) => format!("Failed to write export file: {}", e),
        },
        Err(e) => format!("Failed to serialize: {}", e),
    }
}

fn import_user_dict() -> String {
    let import_path = user_dict_export_path();
    let content = match std::fs::read_to_string(&import_path) {
        Ok(c) => c,
        Err(e) => return format!("Failed to read {}: {}", import_path.display(), e),
    };
    let data: HashMap<String, u64> = match ron::from_str(&content) {
        Ok(d) => d,
        Err(e) => return format!("Failed to parse RON: {}", e),
    };
    if data.is_empty() {
        return "Import file is empty.".into();
    }
    let ud = match open_user_dict() {
        Ok(ud) => ud,
        Err(_) => {
            // Try to create it
            let path = user_dict_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match libchinese_core::UserDict::new(&path) {
                Ok(ud) => ud,
                Err(e) => return format!("Failed to create user dict: {}", e),
            }
        }
    };
    let mut count = 0;
    for (phrase, freq) in &data {
        if ud.add_phrase(phrase, *freq).is_ok() {
            count += 1;
        }
    }
    format!("Imported {} entries from {}", count, import_path.display())
}

fn clear_user_dict() -> String {
    let path = user_dict_path();
    if !path.exists() {
        return "No user dictionary to clear.".into();
    }
    match std::fs::remove_file(&path) {
        Ok(_) => "User dictionary cleared.".into(),
        Err(e) => format!("Failed to clear user dictionary: {}", e),
    }
}

fn list_custom_phrases() -> Vec<(String, u64)> {
    let Ok(ud) = open_user_dict() else {
        return Vec::new();
    };
    let mut all = ud.list_all();
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    all
}

pub fn add_custom_phrase(phrase: &str) -> String {
    let ud = match open_user_dict() {
        Ok(ud) => ud,
        Err(_) => {
            let path = user_dict_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match libchinese_core::UserDict::new(&path) {
                Ok(ud) => ud,
                Err(e) => return format!("Failed to open user dict: {e}"),
            }
        }
    };
    match ud.add_phrase(phrase, 500) {
        Ok(()) => format!("Pinned “{phrase}” (freq 500)"),
        Err(e) => format!("Failed to add phrase: {e}"),
    }
}

pub fn delete_custom_phrase(phrase: &str) -> String {
    let Ok(ud) = open_user_dict() else {
        return "User dictionary not found.".into();
    };
    match ud.delete_phrase(phrase) {
        Ok(()) => {
            let _ = ud.unmask_phrase(phrase);
            format!("Deleted “{phrase}”")
        }
        Err(e) => format!("Failed to delete: {e}"),
    }
}

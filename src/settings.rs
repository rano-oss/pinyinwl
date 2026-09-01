//! Pinyin IME settings window
//!
//! Launched via `pinyinwl --settings`. Uses full libcosmic Application
//! for proper cosmic theme.

use std::collections::HashMap;
use std::path::PathBuf;

use cosmic::app::{Core, Task};
use cosmic::iced::{self, Length};
use cosmic::widget::{self, checkbox, container, row, settings, toggler};
use cosmic::{executor, Element};
use serde::{Deserialize, Serialize};

const CONFIG_NAME: &str = "com.pinyinwl.Settings";
const CONFIG_VERSION: u64 = 1;

const DOUBLE_PINYIN_SCHEMES: &[&str] = &["ZiRanMa", "Microsoft", "XiaoHe", "ZiGuang", "ABC"];

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
    pub pinyin_incomplete: bool,
    pub sort_by_pinyin_length: bool,
    pub auto_suggestion: bool,
    pub emoji_candidate: bool,
    pub character_set: CharacterSet,

    // -- Addon dictionaries --
    /// List of enabled addon dictionary names
    pub enabled_addons: Vec<String>,

    // -- Double pinyin --
    pub double_pinyin_scheme: Option<String>,

    // -- Fuzzy pinyin --
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

impl Default for PinyinConfig {
    fn default() -> Self {
        Self {
            candidates_per_page: 9,
            default_fullwidth: false,
            auto_commit_single: false,
            select_keys: "123456789".to_string(),

            pinyin_incomplete: true,
            sort_by_pinyin_length: false,
            auto_suggestion: true,
            emoji_candidate: true,
            character_set: CharacterSet::Simplified,
            enabled_addons: Vec::new(),

            double_pinyin_scheme: None,

            fuzzy_pinyin: false,
            fuzzy_zh_z: true,
            fuzzy_ch_c: true,
            fuzzy_sh_s: true,
            fuzzy_l_n: true,
            fuzzy_l_r: true,
            fuzzy_f_h: true,
            fuzzy_g_k: true,
            fuzzy_an_ang: true,
            fuzzy_en_eng: true,
            fuzzy_in_ing: true,
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
        config.get("settings").unwrap_or_default()
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
}

#[derive(Debug, Clone)]
pub enum Msg {
    // General
    ToggleDefaultFullwidth(bool),
    ToggleAutoCommitSingle(bool),
    SetSelectKeys(String),
    SetCandidatesPerPage(usize),
    // Behavior
    TogglePinyinIncomplete(bool),
    ToggleSortByPinyinLength(bool),
    ToggleAutoSuggestion(bool),
    ToggleEmojiCandidate(bool),
    SetCharacterSet(CharacterSet),
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

const SELECT_KEY_OPTIONS: &[&str] = &["123456789", "asdfghjkl", "qwertyuio"];

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
        };
        (app, Task::none())
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![widget::text::title3("Pinyin Settings").into()]
    }

    fn update(&mut self, msg: Self::Message) -> Task<Self::Message> {
        match msg {
            // General
            Msg::ToggleDefaultFullwidth(v) => self.config.default_fullwidth = v,
            Msg::ToggleAutoCommitSingle(v) => self.config.auto_commit_single = v,
            Msg::SetSelectKeys(k) => self.config.select_keys = k,
            Msg::SetCandidatesPerPage(n) => self.config.candidates_per_page = n,
            // Behavior
            Msg::TogglePinyinIncomplete(v) => self.config.pinyin_incomplete = v,
            Msg::ToggleSortByPinyinLength(v) => self.config.sort_by_pinyin_length = v,
            Msg::ToggleAutoSuggestion(v) => self.config.auto_suggestion = v,
            Msg::ToggleEmojiCandidate(v) => self.config.emoji_candidate = v,
            Msg::SetCharacterSet(cs) => self.config.character_set = cs,
            Msg::ToggleAddon(name, enabled) => {
                if enabled {
                    if !self.config.enabled_addons.contains(&name) {
                        self.config.enabled_addons.push(name);
                    }
                } else {
                    self.config.enabled_addons.retain(|n| *n != name);
                }
            }
            // Double pinyin
            Msg::SetDoublePinyinScheme(s) => self.config.double_pinyin_scheme = s,
            // Fuzzy
            Msg::ToggleFuzzyPinyin(v) => self.config.fuzzy_pinyin = v,
            Msg::ToggleFuzzyZhZ(v) => self.config.fuzzy_zh_z = v,
            Msg::ToggleFuzzyChC(v) => self.config.fuzzy_ch_c = v,
            Msg::ToggleFuzzyShS(v) => self.config.fuzzy_sh_s = v,
            Msg::ToggleFuzzyLN(v) => self.config.fuzzy_l_n = v,
            Msg::ToggleFuzzyLR(v) => self.config.fuzzy_l_r = v,
            Msg::ToggleFuzzyFH(v) => self.config.fuzzy_f_h = v,
            Msg::ToggleFuzzyGK(v) => self.config.fuzzy_g_k = v,
            Msg::ToggleFuzzyAnAng(v) => self.config.fuzzy_an_ang = v,
            Msg::ToggleFuzzyEnEng(v) => self.config.fuzzy_en_eng = v,
            Msg::ToggleFuzzyInIng(v) => self.config.fuzzy_in_ing = v,
            Msg::ToggleFuzzyIanIang(v) => self.config.fuzzy_ian_iang = v,
            Msg::ToggleFuzzyUanUang(v) => self.config.fuzzy_uan_uang = v,
            // Corrections
            Msg::ToggleCorrectPinyin(v) => self.config.correct_pinyin = v,
            Msg::ToggleCorrectGnNg(v) => self.config.correct_gn_ng = v,
            Msg::ToggleCorrectMgNg(v) => self.config.correct_mg_ng = v,
            Msg::ToggleCorrectIouIu(v) => self.config.correct_iou_iu = v,
            Msg::ToggleCorrectUeiUi(v) => self.config.correct_uei_ui = v,
            Msg::ToggleCorrectUenUn(v) => self.config.correct_uen_un = v,
            Msg::ToggleCorrectUeVe(v) => self.config.correct_ue_ve = v,
            Msg::ToggleCorrectVU(v) => self.config.correct_v_u = v,
            Msg::ToggleCorrectOnOng(v) => self.config.correct_on_ong = v,
            // User dictionary
            Msg::ExportUserDict => {
                let status = export_user_dict();
                self.status_message = Some(status);
                return Task::none();
            }
            Msg::ImportUserDict => {
                let status = import_user_dict();
                self.status_message = Some(status);
                return Task::none();
            }
            Msg::ClearUserDict => {
                let status = clear_user_dict();
                self.status_message = Some(status);
                return Task::none();
            }
        }
        self.config.save();
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let dep = (self.config.clone(), self.status_message.clone());
        widget::lazy(dep, |(config, status_message)| {
            build_settings_view(config, status_message.as_deref())
        })
        .into()
    }
}

/// Build the entire settings view from owned config data.
/// Used inside `widget::lazy` so the widget tree is cached between frames.
fn build_settings_view(
    config: &PinyinConfig,
    status_message: Option<&str>,
) -> Element<'static, Msg> {
    // === General section ===
    let select_keys_buttons = {
        let buttons: Vec<Element<'static, Msg>> = SELECT_KEY_OPTIONS
            .iter()
            .map(|&k| {
                if k == config.select_keys {
                    widget::button::suggested(k).into()
                } else {
                    widget::button::standard(k)
                        .on_press(Msg::SetSelectKeys(k.to_string()))
                        .into()
                }
            })
            .collect();
        widget::flex_row(buttons).row_spacing(6).column_spacing(6)
    };

    let candidates_buttons = {
        let current = config.candidates_per_page;
        let buttons: Vec<Element<'static, Msg>> = (5..=10)
            .map(|n| {
                let label = format!("{}", n);
                if n == current {
                    widget::button::suggested(label).into()
                } else {
                    widget::button::standard(label)
                        .on_press(Msg::SetCandidatesPerPage(n))
                        .into()
                }
            })
            .collect();
        widget::flex_row(buttons).row_spacing(6).column_spacing(6)
    };

    let charset_buttons = {
        let sets: &[(CharacterSet, &str)] = &[
            (CharacterSet::Simplified, "Simplified"),
            (CharacterSet::Traditional, "Traditional"),
        ];
        let buttons: Vec<Element<'static, Msg>> = sets
            .iter()
            .map(|&(cs, label)| {
                if config.character_set == cs {
                    widget::button::suggested(label).into()
                } else {
                    widget::button::standard(label)
                        .on_press(Msg::SetCharacterSet(cs))
                        .into()
                }
            })
            .collect();
        widget::flex_row(buttons).row_spacing(6).column_spacing(6)
    };

    let general_section: Element<'static, Msg> = settings::section()
        .title("General")
        .add(settings::item(
            "Candidate selection keys",
            select_keys_buttons,
        ))
        .add(settings::item("Candidates per page", candidates_buttons))
        .add(settings::item("Character set", charset_buttons))
        .add(settings::item(
            "Default fullwidth mode",
            toggler(config.default_fullwidth).on_toggle(Msg::ToggleDefaultFullwidth),
        ))
        .add(settings::item(
            "Auto-commit single candidate",
            toggler(config.auto_commit_single).on_toggle(Msg::ToggleAutoCommitSingle),
        ))
        .add(settings::item(
            "Incomplete pinyin",
            toggler(config.pinyin_incomplete).on_toggle(Msg::TogglePinyinIncomplete),
        ))
        .add(settings::item(
            "Sort by pinyin length",
            toggler(config.sort_by_pinyin_length).on_toggle(Msg::ToggleSortByPinyinLength),
        ))
        .add(settings::item(
            "Auto-suggestion",
            toggler(config.auto_suggestion).on_toggle(Msg::ToggleAutoSuggestion),
        ))
        .add(settings::item(
            "Emoji candidates",
            toggler(config.emoji_candidate).on_toggle(Msg::ToggleEmojiCandidate),
        ))
        .into();

    // === Double Pinyin section ===
    let double_pinyin_buttons = {
        let current = config.double_pinyin_scheme.as_deref();
        let mut buttons: Vec<Element<'static, Msg>> = Vec::new();
        let none_btn: Element<'static, Msg> = if current.is_none() {
            widget::button::suggested("None").into()
        } else {
            widget::button::standard("None")
                .on_press(Msg::SetDoublePinyinScheme(None))
                .into()
        };
        buttons.push(none_btn);
        for &scheme in DOUBLE_PINYIN_SCHEMES {
            let btn: Element<'static, Msg> = if current == Some(scheme) {
                widget::button::suggested(scheme).into()
            } else {
                widget::button::standard(scheme)
                    .on_press(Msg::SetDoublePinyinScheme(Some(scheme.to_string())))
                    .into()
            };
            buttons.push(btn);
        }
        widget::flex_row(buttons).row_spacing(6).column_spacing(6)
    };

    let double_pinyin_section: Element<'static, Msg> = settings::section()
        .title("Double Pinyin")
        .add(settings::item("Scheme", double_pinyin_buttons))
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

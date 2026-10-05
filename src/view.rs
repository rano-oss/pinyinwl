//! Candidate popup rendering.

use cosmic::iced::{self, Color};
use cosmic::widget::{self, container, mouse_area, row, text};
use cosmic::Element;

use crate::state::InputMethodState;
use crate::Message;

pub fn view<'a>(state: &'a InputMethodState, _window: iced::window::Id) -> Element<'a, Message> {
    if !state.popup_visible() {
        return widget::Space::new().width(0).height(0).into();
    }

    let candidates = state.candidates();
    let cursor = state.candidate_cursor();
    let cosmic_theme = cosmic::theme::active();
    let cosmic = cosmic_theme.cosmic();
    let spacing = cosmic.spacing;
    let radius = cosmic.corner_radii.radius_xs;
    let colors = CandidateColors {
        selected_bg: Color::from(cosmic.accent_color()),
        selected_fg: Color::from(cosmic.on_accent_color()),
        normal_fg: Color::from(cosmic.primary(false).on),
        dim_fg: Color::from(cosmic.primary(false).component.on_disabled),
    };
    let sel_keys = state.selection_keys();

    let items: Vec<Element<'_, Message>> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            candidate_item(
                c,
                i,
                i == cursor,
                sel_keys,
                &colors,
                spacing.space_s,
                spacing.space_xxxs,
                [spacing.space_xxs, spacing.space_xs],
                radius,
            )
        })
        .collect();

    let list: Element<'_, Message> = if state.pinyin_config.vertical_lookup_table {
        widget::column(items).spacing(spacing.space_xxxs).into()
    } else {
        row(items).spacing(spacing.space_xxxs).into()
    };

    let footer = popup_footer(state, &colors);
    let body: Element<'_, Message> = if let Some(foot) = footer {
        widget::column![list, foot]
            .spacing(spacing.space_xxs)
            .into()
    } else {
        list
    };

    cosmic::widget::autosize::autosize(
        container(body)
            .padding(spacing.space_xxs)
            .width(iced::Length::Shrink)
            .height(iced::Length::Shrink)
            .class(cosmic::theme::Container::Dropdown),
        cosmic::widget::Id::new("im-popup"),
    )
    .into()
}

/// Page N/M (when enabled) + tone filter (always when active).
fn popup_footer<'a>(
    state: &'a InputMethodState,
    colors: &CandidateColors,
) -> Option<Element<'a, Message>> {
    let page = state.ime.context().page_text.as_str();
    let tone = state.ime.context().auxiliary_text.as_str();
    let show_page = state.pinyin_config.show_page_number && !page.is_empty();
    // Tone chrome always when filter active (not gated on show_page_number).
    let show_tone = !tone.is_empty()
        && (tone.ends_with('声') || tone == "标点" || tone == "预测");

    if !show_page && !show_tone {
        return None;
    }

    let mut parts = Vec::new();
    if show_tone {
        parts.push(tone.to_string());
    }
    if show_page {
        parts.push(page.to_string());
    }
    let label = parts.join(" · ");
    Some(
        text::caption(label)
            .class(cosmic::theme::style::Text::Color(colors.dim_fg))
            .into(),
    )
}

struct CandidateColors {
    selected_bg: Color,
    selected_fg: Color,
    normal_fg: Color,
    dim_fg: Color,
}

fn candidate_item<'a>(
    text_str: &str,
    index: usize,
    is_selected: bool,
    selection_keys: &str,
    colors: &CandidateColors,
    number_width: u16,
    item_spacing: u16,
    padding: [u16; 2],
    corner_radius: [f32; 4],
) -> Element<'a, Message> {
    let (num_color, text_color) = if is_selected {
        (colors.selected_fg, colors.selected_fg)
    } else {
        (colors.dim_fg, colors.normal_fg)
    };
    let number_text = selection_keys
        .chars()
        .nth(index)
        .map(|c| c.to_string())
        .unwrap_or_else(|| format!("{}", (index + 1) % 10));

    let content = row![
        container(text::body(number_text).class(cosmic::theme::style::Text::Color(num_color)))
            .width(number_width)
            .align_x(iced::alignment::Horizontal::Right),
        text::body(text_str.to_string()).class(cosmic::theme::style::Text::Color(text_color)),
    ]
    .align_y(iced::Alignment::Center)
    .spacing(item_spacing);

    let padded = if is_selected {
        let bg = colors.selected_bg;
        container(content)
            .padding(padding)
            .class(cosmic::theme::Container::custom(move |_| container::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: corner_radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }))
    } else {
        container(content).padding(padding)
    };

    // Click selects (same as pressing the selection key for this row).
    mouse_area(padded)
        .on_press(Message::SelectCandidate((index + 1) as u8))
        .into()
}

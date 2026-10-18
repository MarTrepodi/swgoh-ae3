//! The building blocks of the layout: cards, the context banner, group
//! labels, chips, segmented controls and the three kinds of button
//! (primary, secondary, icon-only). Colours come from [`theme`].
//!
//! Every control that shows an icon keeps a plain accessible name (the
//! text without the icon), which screen readers read and the UI tests look
//! widgets up by.

use eframe::egui::{
    self, Color32, CornerRadius, Frame, Margin, Response, RichText, Stroke, Ui, Vec2, vec2,
};

use super::icons;
use super::theme::{self, CARD_RADIUS, CONTROL_RADIUS, Palette, palette};

/// Corner radius of chips: a pill.
const PILL: u8 = 13;

/// A card: a raised surface with a thin border and generous padding.
pub fn card_frame(ui: &Ui) -> Frame {
    let p = palette(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(14))
}

/// Draws `add` in a card as wide as the space allows.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_frame(ui)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A card's title.
pub fn card_title(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).text_style(theme::section_style()));
}

/// A small muted label naming the group of controls below it.
pub fn group_label(ui: &mut Ui, text: &str) {
    let p = palette(ui);
    ui.label(
        RichText::new(text)
            .family(theme::medium())
            .size(theme::size::SMALL)
            .color(p.text_muted),
    );
}

/// Muted secondary text.
pub fn muted(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = palette(ui);
    ui.label(RichText::new(text.into()).color(p.text_muted))
}

/// Small muted secondary text.
pub fn caption(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = palette(ui);
    ui.label(
        RichText::new(text.into())
            .size(theme::size::SMALL)
            .color(p.text_muted),
    )
}

/// The accent-tinted banner above the main area.
pub fn banner_frame(ui: &Ui) -> Frame {
    let p = palette(ui);
    Frame::new()
        .fill(p.accent_tint)
        .stroke(Stroke::new(1.0, p.accent_border))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(10, 6))
}

/// The tone of a status line or chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Ok,
    Warning,
    Error,
}

impl Tone {
    /// The icon colour of this tone.
    #[must_use]
    pub fn color(self, p: &Palette) -> Color32 {
        match self {
            Self::Neutral => p.text_muted,
            Self::Ok => p.green,
            Self::Warning => p.amber,
            Self::Error => p.red,
        }
    }

    /// The icon of this tone.
    #[must_use]
    pub fn icon(self) -> &'static str {
        match self {
            Self::Neutral => icons::ABOUT,
            Self::Ok => icons::OK,
            Self::Warning => icons::WARNING,
            Self::Error => icons::ERROR,
        }
    }
}

/// `icon`, then the label `add_text` draws, in reading order whatever
/// the direction of `ui`. The icon is a label of its own, so the text's
/// label reads (and is found by the UI tests) without it.
pub fn with_icon(
    ui: &mut Ui,
    icon: RichText,
    add_text: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if ui.layout().prefer_right_to_left() {
            let text = add_text(ui);
            ui.label(icon);
            text
        } else {
            ui.label(icon);
            add_text(ui)
        }
    })
    .inner
}

/// A status message: its tone's icon in its colour, then `text`, which
/// wraps. Warnings and errors colour the text too.
pub fn status(ui: &mut Ui, tone: Tone, text: impl Into<String>) -> Response {
    let p = palette(ui);
    let color = tone.color(p);
    let text_color = match tone {
        Tone::Neutral | Tone::Ok => p.text,
        Tone::Warning | Tone::Error => color,
    };
    let text = text.into();
    with_icon(ui, RichText::new(tone.icon()).color(color), |ui| {
        ui.add(egui::Label::new(RichText::new(text).color(text_color)).wrap())
    })
}

/// A chip: `text` after `icon` in a pill, for status in the header.
pub fn chip(ui: &mut Ui, tone: Tone, icon: &str, text: &str) -> Response {
    let p = palette(ui);
    let size = theme::size::SMALL + 0.5;
    Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(PILL))
        .inner_margin(Margin::symmetric(10, 0))
        .show(ui, |ui| {
            with_icon(
                ui,
                RichText::new(icon).size(size).color(tone.color(p)),
                |ui| ui.label(RichText::new(text).size(size)),
            )
        })
        .inner
}

/// Button text: Ubuntu Medium, with `icon` in front when there is one.
fn button_text(icon: Option<&str>, text: &str) -> RichText {
    let text = match icon {
        Some(icon) => icons::with(icon, text),
        None => text.to_owned(),
    };
    RichText::new(text).family(theme::medium())
}

/// A secondary button, named `text`.
pub fn button(ui: &mut Ui, icon: Option<&str>, text: &str, enabled: bool) -> Response {
    ui.add_enabled(enabled, egui::Button::new(button_text(icon, text)))
        .accessible_name(text)
}

/// A secondary button filling the width, named `text`.
pub fn wide_button(ui: &mut Ui, icon: Option<&str>, text: &str, enabled: bool) -> Response {
    let size = vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0);
    ui.add_enabled(
        enabled,
        egui::Button::new(button_text(icon, text)).min_size(size),
    )
    .accessible_name(text)
}

/// The view's primary action: an accent-filled button, named `text`.
pub fn primary_button(ui: &mut Ui, icon: Option<&str>, text: &str, enabled: bool) -> Response {
    let p = palette(ui);
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for (state, fill) in [
            (&mut w.inactive, p.accent),
            (&mut w.hovered, p.accent_hover),
            (&mut w.active, p.accent_hover),
        ] {
            state.weak_bg_fill = fill;
            state.bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, fill);
            state.fg_stroke = Stroke::new(1.0, p.on_accent);
        }
        let size = vec2(0.0, ui.spacing().interact_size.y + 6.0);
        ui.add_enabled(
            enabled,
            egui::Button::new(button_text(icon, text)).min_size(size),
        )
        .accessible_name(text)
    })
    .inner
}

/// A thin progress bar in the accent colour, on a visible track.
pub fn progress(ui: &mut Ui, fraction: f32, width: f32) -> Response {
    let p = palette(ui);
    ui.scope(|ui| {
        ui.visuals_mut().extreme_bg_color = p.raised;
        ui.add(
            egui::ProgressBar::new(fraction)
                .fill(p.accent)
                .desired_width(width)
                .desired_height(8.0)
                .corner_radius(4.0),
        )
    })
    .inner
    .accessible_name(format!("{:.0}%", fraction * 100.0))
}

/// A small icon-only button, named `name` and explaining itself on hover.
pub fn icon_button(ui: &mut Ui, icon: &str, name: &str) -> Response {
    ui.add(egui::Button::new(icon).frame_when_inactive(false))
        .on_hover_text(name)
        .accessible_name(name)
}

/// Gives selected buttons drawn by `add` the accent outline.
fn with_selection_outline<R>(ui: &mut Ui, selected: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui);
    ui.scope(|ui| {
        if selected {
            let w = &mut ui.visuals_mut().widgets;
            for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                state.bg_stroke = Stroke::new(1.0, p.accent_text);
            }
        }
        add(ui)
    })
    .inner
}

/// A toggle chip for a filter: a pill, accent-outlined while on. Named
/// `text`.
pub fn toggle_chip(ui: &mut Ui, on: bool, icon: Option<&str>, text: &str) -> Response {
    let p = palette(ui);
    with_selection_outline(ui, on, |ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = p.card;
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
            state.corner_radius = CornerRadius::same(PILL);
        }
        let label = match icon {
            Some(icon) => icons::with(icon, text),
            None => text.to_owned(),
        };
        ui.add(egui::Button::new(label).selected(on).frame(true))
            .accessible_name(text)
    })
}

/// A chip that picks a Game Entity: like [`toggle_chip`], enabled when it
/// has somewhere to go.
pub fn entity_chip(ui: &mut Ui, on: bool, enabled: bool, label: &str, name: &str) -> Response {
    let p = palette(ui);
    with_selection_outline(ui, on, |ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = p.card;
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
            state.corner_radius = CornerRadius::same(PILL);
        }
        ui.add_enabled(enabled, egui::Button::new(label).selected(on).frame(true))
            .accessible_name(name)
    })
}

/// A segmented control: one segment per option, the current one
/// accent-outlined. With `fill`, the segments share the width. Returns
/// true when the value changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, Option<&str>, &str)],
    fill: bool,
) -> bool {
    let p = palette(ui);
    let mut changed = false;
    Frame::new()
        .fill(p.inset)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(CONTROL_RADIUS + 2))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let count = options.len().max(1) as f32;
            let width = if fill {
                (ui.available_width() - 2.0 * (count - 1.0)) / count
            } else {
                0.0
            };
            ui.horizontal(|ui| {
                for &(option, icon, text) in options {
                    let on = *value == option;
                    let label = button_text(icon, text);
                    let response = with_selection_outline(ui, on, |ui| {
                        ui.add(
                            egui::Button::new(label)
                                .selected(on)
                                .frame_when_inactive(on)
                                .min_size(Vec2::new(width, ui.spacing().interact_size.y - 2.0)),
                        )
                        .accessible_name(text)
                    });
                    if response.clicked() && !on {
                        *value = option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

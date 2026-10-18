//! The look of the window: a palette taken from the application icon,
//! Ubuntu Regular and Medium on a clear type scale, and the spacing and
//! rounding of the cards.
//!
//! The icon's glowing cyan is the one accent: primary buttons, selection
//! and focus. Its amber marks search matches and warnings. Green, amber
//! and red appear only as status. Surfaces are charcoal in three steps
//! (chrome, canvas, card), with thin borders. The light variant keeps the
//! same roles. The window follows the system's light or dark setting
//! (egui's default `ThemePreference::System`); [`install`] sets both styles.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, Style, TextStyle, Theme, Visuals,
    style::{ScrollStyle, Selection, WidgetVisuals},
    vec2,
};

/// The colours of one theme; see the module docs for their roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The header, the sidebar and the action bar.
    pub chrome: Color32,
    /// The main area, behind its cards.
    pub canvas: Color32,
    /// Cards, windows and menus.
    pub card: Color32,
    /// Buttons and other controls, at rest, hovered and pressed.
    pub raised: Color32,
    pub raised_hover: Color32,
    pub raised_active: Color32,
    /// Text fields, progress tracks and image backgrounds, sunk into a card.
    pub inset: Color32,
    /// Every other table row.
    pub stripe: Color32,
    pub border: Color32,
    /// Borders of hovered controls.
    pub border_strong: Color32,
    pub text: Color32,
    /// Titles and hovered text.
    pub text_strong: Color32,
    /// Secondary text: labels, counts, hints.
    pub text_muted: Color32,
    /// The fill of the primary action, and the text on it.
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    /// Accent-coloured text and outlines on a surface.
    pub accent_text: Color32,
    /// Behind selected items and the context banner.
    pub accent_tint: Color32,
    /// The context banner's outline.
    pub accent_border: Color32,
    /// Search matches and warnings.
    pub amber: Color32,
    /// Success.
    pub green: Color32,
    /// Errors.
    pub red: Color32,
}

/// The dark palette, the default. Text contrast (WCAG): body 13.9:1 and
/// muted 6.6:1 on a card, accent text 9:1, text on the accent 8:1.
pub const DARK: Palette = Palette {
    chrome: Color32::from_rgb(0x15, 0x18, 0x1c),
    canvas: Color32::from_rgb(0x1b, 0x1f, 0x24),
    card: Color32::from_rgb(0x22, 0x27, 0x2e),
    raised: Color32::from_rgb(0x2b, 0x31, 0x39),
    raised_hover: Color32::from_rgb(0x33, 0x3a, 0x44),
    raised_active: Color32::from_rgb(0x3b, 0x43, 0x4e),
    inset: Color32::from_rgb(0x14, 0x17, 0x1b),
    stripe: Color32::from_rgb(0x26, 0x2b, 0x33),
    border: Color32::from_rgb(0x30, 0x37, 0x41),
    border_strong: Color32::from_rgb(0x48, 0x52, 0x5f),
    text: Color32::from_rgb(0xdc, 0xe1, 0xe7),
    text_strong: Color32::from_rgb(0xf3, 0xf5, 0xf8),
    text_muted: Color32::from_rgb(0x9a, 0xa4, 0xb1),
    accent: Color32::from_rgb(0x2b, 0xb8, 0xe3),
    accent_hover: Color32::from_rgb(0x4c, 0xc8, 0xee),
    on_accent: Color32::from_rgb(0x06, 0x14, 0x1a),
    accent_text: Color32::from_rgb(0x5c, 0xd3, 0xf5),
    accent_tint: Color32::from_rgb(0x16, 0x37, 0x46),
    accent_border: Color32::from_rgb(0x1f, 0x56, 0x6c),
    amber: Color32::from_rgb(0xf2, 0xb6, 0x50),
    green: Color32::from_rgb(0x5c, 0xc9, 0x8a),
    red: Color32::from_rgb(0xf0, 0x70, 0x70),
};

/// The light palette. Text contrast (WCAG): body 16.6:1 and muted 6:1 on
/// a card, accent text 5.6:1, text on the accent 4.9:1.
pub const LIGHT: Palette = Palette {
    chrome: Color32::from_rgb(0xe8, 0xec, 0xf0),
    canvas: Color32::from_rgb(0xf2, 0xf4, 0xf7),
    card: Color32::from_rgb(0xff, 0xff, 0xff),
    raised: Color32::from_rgb(0xec, 0xef, 0xf3),
    raised_hover: Color32::from_rgb(0xe1, 0xe6, 0xec),
    raised_active: Color32::from_rgb(0xd5, 0xdc, 0xe4),
    inset: Color32::from_rgb(0xf6, 0xf8, 0xfa),
    stripe: Color32::from_rgb(0xf5, 0xf7, 0xf9),
    border: Color32::from_rgb(0xd2, 0xd8, 0xdf),
    border_strong: Color32::from_rgb(0xa9, 0xb3, 0xbf),
    text: Color32::from_rgb(0x1a, 0x1f, 0x26),
    text_strong: Color32::from_rgb(0x0b, 0x0e, 0x12),
    text_muted: Color32::from_rgb(0x5a, 0x64, 0x72),
    accent: Color32::from_rgb(0x0b, 0x79, 0xa6),
    accent_hover: Color32::from_rgb(0x09, 0x6a, 0x92),
    on_accent: Color32::from_rgb(0xff, 0xff, 0xff),
    accent_text: Color32::from_rgb(0x0a, 0x6f, 0x99),
    accent_tint: Color32::from_rgb(0xd7, 0xee, 0xf8),
    accent_border: Color32::from_rgb(0x9c, 0xd2, 0xea),
    amber: Color32::from_rgb(0x8f, 0x53, 0x00),
    green: Color32::from_rgb(0x15, 0x70, 0x38),
    red: Color32::from_rgb(0xc0, 0x32, 0x2b),
};

impl Palette {
    /// The palette of `theme`.
    #[must_use]
    pub const fn of(theme: Theme) -> &'static Self {
        match theme {
            Theme::Dark => &DARK,
            Theme::Light => &LIGHT,
        }
    }
}

/// The palette `ui` is drawn with.
#[must_use]
pub fn palette(ui: &egui::Ui) -> &'static Palette {
    palette_of(ui.visuals())
}

/// The palette that `visuals` were made from.
#[must_use]
pub fn palette_of(visuals: &Visuals) -> &'static Palette {
    Palette::of(if visuals.dark_mode {
        Theme::Dark
    } else {
        Theme::Light
    })
}

// ---- Type ------------------------------------------------------------------

/// Point sizes of the type scale.
pub mod size {
    /// Captions, group labels, hints.
    pub const SMALL: f32 = 12.0;
    /// Body text and buttons.
    pub const BODY: f32 = 14.0;
    /// Paths and code.
    pub const MONO: f32 = 13.0;
    /// Card titles.
    pub const SECTION: f32 = 15.5;
    /// Headings: the preview's Package, window titles.
    pub const HEADING: f32 = 18.0;
    /// The application name in the header.
    pub const BRAND: f32 = 17.0;
}

/// The name of the card-title text style.
pub const SECTION: &str = "section";

/// The card-title text style.
#[must_use]
pub fn section_style() -> TextStyle {
    TextStyle::Name(SECTION.into())
}

const UBUNTU_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Ubuntu-Regular.ttf");
const UBUNTU_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Ubuntu-Medium.ttf");
const REGULAR: &str = "Ubuntu-Regular";
const MEDIUM: &str = "Ubuntu-Medium";
const HACK: &str = "Hack";
const ICONS: &str = super::icons::FONT_NAME;

/// The family of Ubuntu Medium, for headings, buttons and emphasis.
#[must_use]
pub fn medium() -> FontFamily {
    FontFamily::Name(MEDIUM.into())
}

/// Ubuntu Medium at `size` points.
#[must_use]
pub fn medium_font(size: f32) -> FontId {
    FontId::new(size, medium())
}

/// The fonts the window draws with: Ubuntu Regular for proportional text,
/// Ubuntu Medium as a named family, Hack for monospace. The Phosphor icons
/// ([`super::icons`]) follow Ubuntu in each family, ahead of Hack, whose
/// Powerline symbols share codepoints with some icons. Hack is the last
/// fallback, for the arrows and geometric shapes Ubuntu lacks.
#[must_use]
pub fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    for (name, bytes) in [
        (REGULAR, UBUNTU_REGULAR),
        (MEDIUM, UBUNTU_MEDIUM),
        (HACK, epaint_default_fonts::HACK_REGULAR),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    fonts
        .font_data
        .insert(ICONS.to_owned(), Arc::new(super::icons::font()));
    let family = |names: &[&str]| names.iter().map(|&n| n.to_owned()).collect::<Vec<_>>();
    fonts
        .families
        .insert(FontFamily::Proportional, family(&[REGULAR, ICONS, HACK]));
    fonts
        .families
        .insert(medium(), family(&[MEDIUM, ICONS, HACK]));
    fonts
        .families
        .insert(FontFamily::Monospace, family(&[HACK, REGULAR, ICONS]));
    fonts
}

// ---- Style -----------------------------------------------------------------

/// Corner radius of buttons and fields.
pub const CONTROL_RADIUS: u8 = 6;
/// Corner radius of cards.
pub const CARD_RADIUS: u8 = 8;
/// Corner radius of windows.
pub const WINDOW_RADIUS: u8 = 10;

fn widget(fill: Color32, border: Color32, text: Color32) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(1.0, border),
        corner_radius: CornerRadius::same(CONTROL_RADIUS),
        fg_stroke: Stroke::new(1.0, text),
        expansion: 0.0,
    }
}

/// The visuals of `theme`.
#[must_use]
pub fn visuals(theme: Theme) -> Visuals {
    let p = Palette::of(theme);
    let mut v = match theme {
        Theme::Dark => Visuals::dark(),
        Theme::Light => Visuals::light(),
    };
    v.widgets.noninteractive = WidgetVisuals {
        bg_fill: p.card,
        weak_bg_fill: p.card,
        ..widget(p.card, p.border, p.text)
    };
    v.widgets.inactive = widget(p.raised, p.border, p.text);
    v.widgets.hovered = widget(p.raised_hover, p.border_strong, p.text_strong);
    v.widgets.active = widget(p.raised_active, p.accent_text, p.text_strong);
    v.widgets.open = widget(p.raised_hover, p.border_strong, p.text_strong);
    v.selection = Selection {
        bg_fill: p.accent_tint,
        stroke: Stroke::new(1.0, p.accent_text),
    };
    v.weak_text_color = Some(p.text_muted);
    v.hyperlink_color = p.accent_text;
    v.faint_bg_color = p.stripe;
    v.extreme_bg_color = p.inset;
    v.text_edit_bg_color = Some(p.inset);
    v.code_bg_color = p.inset;
    v.warn_fg_color = p.amber;
    v.error_fg_color = p.red;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(WINDOW_RADIUS);
    v.menu_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.panel_fill = p.canvas;
    let shadow = match theme {
        Theme::Dark => Color32::from_black_alpha(110),
        Theme::Light => Color32::from_black_alpha(40),
    };
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: shadow,
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: shadow,
    };
    v.window_highlight_topmost = false;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = true;
    v.striped = true;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent_text);
    v.disabled_alpha = 0.45;
    v
}

/// The whole style of `theme`: visuals, type scale and spacing.
#[must_use]
pub fn style(theme: Theme) -> Style {
    let mut style = Style {
        visuals: visuals(theme),
        ..Style::default()
    };
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(size::SMALL)),
        (TextStyle::Body, FontId::proportional(size::BODY)),
        (TextStyle::Button, FontId::new(size::BODY, medium())),
        (TextStyle::Monospace, FontId::monospace(size::MONO)),
        (TextStyle::Heading, FontId::new(size::HEADING, medium())),
        (section_style(), FontId::new(size::SECTION, medium())),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = vec2(8.0, 6.0);
    s.button_padding = vec2(10.0, 4.0);
    s.interact_size = vec2(36.0, 26.0);
    s.window_margin = Margin::same(16);
    s.menu_margin = Margin::same(6);
    s.icon_width = 16.0;
    s.icon_width_inner = 9.0;
    s.icon_spacing = 6.0;
    s.combo_height = 320.0;
    s.tooltip_width = 360.0;
    s.scroll = ScrollStyle::thin();
    style.interaction.tooltip_delay = 0.4;
    style
}

/// Installs the fonts and both themes' styles in `ctx`. Safe to call more
/// than once: everything is set, not adjusted. Leaves the theme preference
/// alone, so the window keeps following the system.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    for theme in [Theme::Dark, Theme::Light] {
        ctx.set_style_of(theme, style(theme));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn luminance(c: Color32) -> f32 {
        let channel = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Text meets WCAG AA (4.5:1) on every surface it is drawn on.
    #[test]
    fn text_is_readable_in_both_themes() {
        for p in [&DARK, &LIGHT] {
            for surface in [p.chrome, p.canvas, p.card, p.raised, p.inset] {
                assert!(contrast(p.text, surface) >= 7.0, "{p:?}");
                assert!(contrast(p.text_muted, surface) >= 4.5, "{p:?}");
            }
            for surface in [p.card, p.canvas, p.chrome] {
                for color in [p.accent_text, p.amber, p.green, p.red] {
                    assert!(contrast(color, surface) >= 4.5, "{color:?} on {surface:?}");
                }
            }
            assert!(contrast(p.on_accent, p.accent) >= 4.5);
            assert!(contrast(p.on_accent, p.accent_hover) >= 4.5);
            assert!(contrast(p.accent_text, p.accent_tint) >= 4.5);
            assert!(contrast(p.text, p.accent_tint) >= 7.0);
        }
    }

    #[test]
    fn install_sets_the_type_scale_once() {
        let ctx = egui::Context::default();
        install(&ctx);
        install(&ctx);
        for theme in [Theme::Dark, Theme::Light] {
            let style = ctx.style_of(theme);
            let body = &style.text_styles[&TextStyle::Body];
            assert!((body.size - size::BODY).abs() < f32::EPSILON);
            assert_eq!(body.family, FontFamily::Proportional);
            assert_eq!(style.text_styles[&TextStyle::Heading].family, medium());
            assert_eq!(style.visuals.dark_mode, theme == Theme::Dark);
            assert_eq!(*palette_of(&style.visuals), *Palette::of(theme));
        }
    }
}

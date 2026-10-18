//! Icons from Phosphor (regular weight), embedded as a subset holding only
//! the icons below. egui-phosphor builds the subset while compiling, so the
//! rest of the 490 KB font never reaches the binary.
//!
//! egui-phosphor 0.14 is written against egui 0.36. Only its codepoints,
//! font bytes and const subsetter are used here, none of its egui types, so
//! no egui 0.36 code is linked. Icons are ordinary characters in the
//! proportional fonts ([`super::theme::fonts`]), after Ubuntu: the subset
//! maps a-z to blank glyphs, which the text font must win over.

use ae3_core::{AssetKind, EntityKind, Platform};
use eframe::egui::FontData;
use egui_phosphor::variants::{bytes, regular as ph};

macro_rules! icons {
    ($($(#[$doc:meta])* $name:ident = $icon:ident,)*) => {
        $($(#[$doc])* pub const $name: &str = ph::$icon;)*

        /// Every icon in the subset.
        pub const ALL: &[&str] = &[$(ph::$icon),*];
    };
}

icons! {
    // Asset kinds
    TEXTURE = IMAGE,
    ATLAS = SQUARES_FOUR,
    SPRITE = STICKER,
    TEXT = FILE_TEXT,
    STRUCTURED = BRACKETS_CURLY,
    MESH = CUBE,
    AUDIO = SPEAKER_HIGH,
    OTHER = CIRCLE_DASHED,
    // Game Entity kinds
    UNIT = USER,
    ABILITY = LIGHTNING,
    GEAR = SHIELD,
    MATERIAL = DIAMOND,
    PORTRAIT = USER_SQUARE,
    // Platforms
    WINDOWS = WINDOWS_LOGO,
    ANDROID = ANDROID_LOGO,
    APPLE = APPLE_LOGO,
    // Actions
    /// Fetch the latest Asset Version.
    FETCH = CLOUD_ARROW_DOWN,
    /// Load a Manifest.
    LOAD = DOWNLOAD_SIMPLE,
    /// Choose or reveal a folder.
    FOLDER = FOLDER_OPEN,
    SETTINGS = GEAR_SIX,
    ABOUT = INFO,
    EXTRACT = EXPORT,
    PAUSE = PAUSE,
    PLAY = PLAY,
    STOP = STOP,
    SEARCH = MAGNIFYING_GLASS,
    /// Clear, dismiss, cancel.
    CLOSE = X,
    RETRY = ARROW_CLOCKWISE,
    /// Reset a view.
    RESET = ARROW_COUNTER_CLOCKWISE,
    /// Fit an image to its view.
    FIT = CORNERS_OUT,
    /// Check or select everything.
    SELECT_ALL = CHECKS,
    SORT_UP = CARET_UP,
    SORT_DOWN = CARET_DOWN,
    /// The Bundles tab.
    BUNDLES = PACKAGE,
    /// The Diff tab and comparing.
    DIFF = GIT_DIFF,
    /// The Asset Index.
    INDEX = DATABASE,
    /// Display names (the Name Index).
    NAMES = TEXT_AA,
    /// A newer release.
    UPDATE = ARROW_CIRCLE_UP,
    // Status
    OK = CHECK_CIRCLE,
    WARNING = WARNING,
    ERROR = WARNING_CIRCLE,
}

const SOURCE: &[u8] = bytes::regular::FONT;
const LEN: usize = egui_phosphor::subset::subset_len(SOURCE, ALL);
/// The subset font: a complete TTF with only [`ALL`].
static FONT: [u8; LEN] = egui_phosphor::subset::subset_into::<LEN>(SOURCE, ALL);

/// The name the icon font is registered under.
pub const FONT_NAME: &str = "phosphor-regular";

/// The icon font.
#[must_use]
pub fn font() -> FontData {
    FontData::from_static(&FONT)
}

/// The icon of an Asset kind.
#[must_use]
pub fn kind(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Texture => TEXTURE,
        AssetKind::Atlas => ATLAS,
        AssetKind::Sprite => SPRITE,
        AssetKind::Text => TEXT,
        AssetKind::Structured => STRUCTURED,
        AssetKind::Mesh => MESH,
        AssetKind::Audio => AUDIO,
        AssetKind::Other => OTHER,
    }
}

/// The icon of a kind of Game Entity.
#[must_use]
pub fn entity(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Unit => UNIT,
        EntityKind::Ability => ABILITY,
        EntityKind::Gear => GEAR,
        EntityKind::Material => MATERIAL,
        EntityKind::PlayerPortrait => PORTRAIT,
    }
}

/// The icon of a Platform.
#[must_use]
pub fn platform(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => WINDOWS,
        Platform::Android => ANDROID,
        Platform::Ios => APPLE,
    }
}

/// `icon` and `text` as one label.
#[must_use]
pub fn with(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, FontFamily, FontId, RawInput};

    use super::*;
    use crate::ui::theme;

    /// Whether every character of `text` renders in `family` with the
    /// fonts of `ctx`.
    fn renders(ctx: &egui::Context, family: &FontFamily, text: &str) -> bool {
        ctx.fonts_mut(|f| f.has_glyphs(&FontId::new(14.0, family.clone()), text))
    }

    fn context(fonts: egui::FontDefinitions) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        let mut output = ctx.run_ui(RawInput::default(), |_| {});
        output.textures_delta.clear();
        ctx
    }

    /// Every icon renders in the text families; a missing one would show as
    /// a box.
    #[test]
    fn every_icon_renders() {
        let ctx = context(theme::fonts());
        for family in [FontFamily::Proportional, theme::medium()] {
            for icon in ALL {
                assert!(renders(&ctx, &family, icon), "{icon:?} in {family:?}");
            }
            // Symbols the window uses in text besides the icons.
            for symbol in ["·", "—", "…", "→", "×", "%"] {
                assert!(renders(&ctx, &family, symbol), "{symbol} in {family:?}");
            }
        }
        for kind in AssetKind::ALL {
            assert!(ALL.contains(&super::kind(kind)));
        }
        for kind in EntityKind::ALL {
            assert!(ALL.contains(&entity(kind)));
        }
        for p in Platform::ALL {
            assert!(ALL.contains(&platform(p)));
        }
    }

    /// No text font has a glyph at an icon's codepoint, so the icons always
    /// come from Phosphor, whatever the fallback order.
    #[test]
    fn no_text_font_shadows_an_icon() {
        let mut fonts = theme::fonts();
        for family in fonts.families.values_mut() {
            family.retain(|name| name != FONT_NAME);
        }
        let ctx = context(fonts);
        for family in [
            FontFamily::Proportional,
            theme::medium(),
            FontFamily::Monospace,
        ] {
            for icon in ALL {
                assert!(
                    !renders(&ctx, &family, icon),
                    "{family:?} has its own glyph at {icon:?}"
                );
            }
        }
    }
}

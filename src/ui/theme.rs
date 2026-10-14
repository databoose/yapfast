//! The window's colours: a full set of what `Theme` in `ui/theme.slint` holds,
//! which Talk sets on the window to change them all at once.

use std::cell::Cell;

use slint::{Color, ComponentHandle};
use talk_ui::TalkWindow;

/// Opaque `#rrggbb`.
const fn rgb(hex: u32) -> Color {
    Color::from_argb_encoded(0xff00_0000 | hex)
}

/// `#rrggbbaa`, as Slint writes a colour with alpha.
const fn rgba(hex: u32) -> Color {
    Color::from_argb_encoded(hex.rotate_right(8))
}

/// Declares `Theme` with one field per colour of the Slint global, and the
/// window's getter and setter for each.
macro_rules! theme {
    ($($name:ident ($get:ident, $set:ident),)*) => {
        /// A full set of the window's colours, and whether they make a dark
        /// theme. `ui/theme.slint` describes each.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Theme {
            pub dark: bool,
            $(pub $name: Color,)*
        }

        impl Theme {
            /// Sets only the colours that differ from those the window
            /// shows: nothing when it starts in Yap, its defaults.
            fn set(&self, tokens: &talk_ui::Theme<'_>) {
                if tokens.get_dark() != self.dark {
                    tokens.set_dark(self.dark);
                }
                $(if tokens.$get() != self.$name {
                    tokens.$set(self.$name);
                })*
            }

            /// The colours the window shows now.
            #[cfg(test)]
            pub fn shown(window: &TalkWindow) -> Self {
                let tokens = window.global::<talk_ui::Theme>();
                Self { dark: tokens.get_dark(), $($name: tokens.$get(),)* }
            }
        }
    };
}

theme! {
    rail(get_rail, set_rail),
    sidebar(get_sidebar, set_sidebar),
    panel(get_panel, set_panel),
    chat(get_chat, set_chat),
    stage(get_stage, set_stage),
    raised(get_raised, set_raised),
    field(get_field, set_field),
    sunken(get_sunken, set_sunken),
    hover(get_hover, set_hover),
    active(get_active, set_active),
    row_hover(get_row_hover, set_row_hover),
    line(get_line, set_line),
    edge(get_edge, set_edge),
    text(get_text, set_text),
    secondary(get_secondary, set_secondary),
    muted(get_muted, set_muted),
    faint(get_faint, set_faint),
    accent(get_accent, set_accent),
    accent_hover(get_accent_hover, set_accent_hover),
    on_accent(get_on_accent, set_on_accent),
    accent_soft(get_accent_soft, set_accent_soft),
    danger(get_danger, set_danger),
    danger_hover(get_danger_hover, set_danger_hover),
    on_danger(get_on_danger, set_on_danger),
    live(get_live, set_live),
    loud(get_loud, set_loud),
    link(get_link, set_link),
    button_default(get_button_default, set_button_default),
    button_default_hover(get_button_default_hover, set_button_default_hover),
    overlay(get_overlay, set_overlay),
    shimmer(get_shimmer, set_shimmer),
    reply_spine(get_reply_spine, set_reply_spine),
    tooltip(get_tooltip, set_tooltip),
    avatar(get_avatar, set_avatar),
}

/// Talk's own colours: layered graphite with a marigold accent. The defaults
/// in `ui/theme.slint` are these, so the first frame shows them.
pub const YAP: Theme = Theme {
    dark: true,
    rail: rgb(0x111215),
    sidebar: rgb(0x17181c),
    panel: rgb(0x121316),
    chat: rgb(0x1e1f24),
    stage: rgb(0x1e1f24),
    raised: rgb(0x26272e),
    field: rgb(0x2a2b32),
    sunken: rgb(0x0e0f12),
    hover: rgb(0x222329),
    active: rgb(0x2c2d35),
    row_hover: rgb(0x1a1b20),
    line: rgb(0x2c2d34),
    edge: rgb(0x0c0d0f),
    text: rgb(0xeef0f4),
    secondary: rgb(0xb5b8c1),
    muted: rgb(0x8d919c),
    faint: rgb(0x5c606a),
    accent: rgb(0xffbf47),
    accent_hover: rgb(0xffcd70),
    on_accent: rgb(0x1d1607),
    accent_soft: rgba(0xffbf4726),
    danger: rgb(0xf0545e),
    danger_hover: rgb(0xf46f77),
    on_danger: rgb(0xffffff),
    live: rgb(0x3ecf7e),
    loud: rgb(0xff8c42),
    link: rgb(0x7cc4ff),
    button_default: rgb(0x33343c),
    button_default_hover: rgb(0x3c3d46),
    overlay: rgb(0xffffff),
    shimmer: rgba(0xffffff10),
    reply_spine: rgb(0x4a4c56),
    tooltip: rgb(0x0b0c0e),
    avatar: rgb(0x6b7280),
};

/// The themes Talk comes with, by the id a layout stores.
pub const BUILT_IN: [(&str, &Theme); 1] = [("yap", &YAP)];

thread_local! {
    /// The colour of a mention without one of its own, as `0xrrggbb`: the
    /// theme's link colour. Rows build their markup with it, outside the
    /// window, on the window's thread.
    static MENTION: Cell<u32> = const { Cell::new(0x7cc4ff) };
}

pub fn mention_color() -> u32 {
    MENTION.get()
}

impl Theme {
    /// Shows the window in these colours. Returns whether mentions change
    /// colour, as rows built before carry the old one in their markup.
    pub fn apply(&self, window: &TalkWindow) -> bool {
        self.set(&window.global::<talk_ui::Theme>());
        let mention = self.link.as_argb_encoded() & 0xff_ffff;
        MENTION.replace(mention) != mention
    }
}

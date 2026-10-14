//! The window's colours: a full set of what `Theme` in `ui/theme.slint` holds,
//! which Talk sets on the window to change them all at once.

use std::cell::Cell;

use slint::{Color, ComponentHandle, ModelRc, VecModel};
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

/// Discord's own dark theme (Dark, `.theme-dark` with `.theme-darker` in its
/// web client's CSS) on Talk's surfaces, with blurple for the accent.
const BONFIRE: Theme = Theme {
    dark: true,
    rail: rgb(0x121214),
    sidebar: rgb(0x121214),
    panel: rgb(0x202024),
    chat: rgb(0x1a1a1e),
    stage: rgb(0x1a1a1e),
    raised: rgb(0x242429),
    field: rgb(0x202024),
    sunken: rgb(0x121214),
    hover: rgb(0x222225),
    active: rgb(0x2d2d30),
    row_hover: rgb(0x242428),
    line: rgb(0x29292e),
    edge: rgb(0x222225),
    text: rgb(0xefeff1),
    secondary: rgb(0xabacb2),
    muted: rgb(0x96979e),
    faint: rgb(0x6c6f7c),
    accent: rgb(0x5865f2),
    accent_hover: rgb(0x4452bb),
    on_accent: rgb(0xffffff),
    accent_soft: rgba(0x5865f226),
    danger: rgb(0xd22d39),
    danger_hover: rgb(0xa9232e),
    on_danger: rgb(0xffffff),
    live: rgb(0x23a55a),
    loud: rgb(0xf0b232),
    link: rgb(0x4d96ee),
    button_default: rgb(0x323237),
    button_default_hover: rgb(0x36363c),
    overlay: rgb(0xffffff),
    shimmer: rgba(0xffffff10),
    reply_spine: rgb(0x474851),
    tooltip: rgb(0x242429),
    avatar: rgb(0x5c5e66),
};

/// Catppuccin's darkest flavour, with mauve for the accent.
const CATPPUCCIN_MOCHA: Theme = Theme {
    dark: true,
    rail: rgb(0x11111b),
    sidebar: rgb(0x181825),
    panel: rgb(0x11111b),
    chat: rgb(0x1e1e2e),
    stage: rgb(0x1e1e2e),
    raised: rgb(0x313244),
    field: rgb(0x313244),
    sunken: rgb(0x11111b),
    hover: rgb(0x282839),
    active: rgb(0x45475a),
    row_hover: rgb(0x1a1a29),
    line: rgb(0x313244),
    edge: rgb(0x11111b),
    text: rgb(0xcdd6f4),
    secondary: rgb(0xbac2de),
    muted: rgb(0x9399b2),
    faint: rgb(0x6c7086),
    accent: rgb(0xcba6f7),
    accent_hover: rgb(0xccb4f6),
    on_accent: rgb(0x11111b),
    accent_soft: rgba(0xcba6f726),
    danger: rgb(0xf38ba8),
    danger_hover: rgb(0xeba0ac),
    on_danger: rgb(0x11111b),
    live: rgb(0xa6e3a1),
    loud: rgb(0xfab387),
    link: rgb(0x89b4fa),
    button_default: rgb(0x45475a),
    button_default_hover: rgb(0x585b70),
    overlay: rgb(0xffffff),
    shimmer: rgba(0xffffff10),
    reply_spine: rgb(0x585b70),
    tooltip: rgb(0x11111b),
    avatar: rgb(0x6c7086),
};

/// Catppuccin's light flavour, with mauve for the accent.
const CATPPUCCIN_LATTE: Theme = Theme {
    dark: false,
    rail: rgb(0xdce0e8),
    sidebar: rgb(0xe6e9ef),
    panel: rgb(0xdce0e8),
    chat: rgb(0xeff1f5),
    stage: rgb(0xeff1f5),
    raised: rgb(0xe6e9ef),
    field: rgb(0xdce0e8),
    sunken: rgb(0xdce0e8),
    hover: rgb(0xe0e4eb),
    active: rgb(0xccd0da),
    row_hover: rgb(0xeaedf2),
    line: rgb(0xccd0da),
    edge: rgb(0xbcc0cc),
    text: rgb(0x4c4f69),
    secondary: rgb(0x5c5f77),
    muted: rgb(0x7c7f93),
    faint: rgb(0x8c8fa1),
    accent: rgb(0x8839ef),
    accent_hover: rgb(0x793ece),
    on_accent: rgb(0xeff1f5),
    accent_soft: rgba(0x8839ef26),
    danger: rgb(0xd20f39),
    danger_hover: rgb(0xb01f45),
    on_danger: rgb(0xeff1f5),
    live: rgb(0x40a02b),
    loud: rgb(0xfe640b),
    link: rgb(0x1e66f5),
    button_default: rgb(0xccd0da),
    button_default_hover: rgb(0xbcc0cc),
    overlay: rgb(0x000000),
    shimmer: rgba(0xffffff80),
    reply_spine: rgb(0xacb0be),
    tooltip: rgb(0xeff1f5),
    avatar: rgb(0x7c7f93),
};

/// Dracula, with purple for the accent.
const DRACULA: Theme = Theme {
    dark: true,
    rail: rgb(0x191a21),
    sidebar: rgb(0x21222c),
    panel: rgb(0x191a21),
    chat: rgb(0x282a36),
    stage: rgb(0x282a36),
    raised: rgb(0x343746),
    field: rgb(0x343746),
    sunken: rgb(0x191a21),
    hover: rgb(0x2e303e),
    active: rgb(0x44475a),
    row_hover: rgb(0x242631),
    line: rgb(0x343746),
    edge: rgb(0x191a21),
    text: rgb(0xf8f8f2),
    secondary: rgb(0xcbd0db),
    muted: rgb(0x9ea8c3),
    faint: rgb(0x6272a4),
    accent: rgb(0xbd93f9),
    accent_hover: rgb(0xccacf7),
    on_accent: rgb(0x191a21),
    accent_soft: rgba(0xbd93f926),
    danger: rgb(0xff5555),
    danger_hover: rgb(0xfe7674),
    on_danger: rgb(0x191a21),
    live: rgb(0x50fa7b),
    loud: rgb(0xffb86c),
    link: rgb(0x8be9fd),
    button_default: rgb(0x44475a),
    button_default_hover: rgb(0x4e5674),
    overlay: rgb(0xffffff),
    shimmer: rgba(0xffffff10),
    reply_spine: rgb(0x44475a),
    tooltip: rgb(0x191a21),
    avatar: rgb(0x6272a4),
};

/// Nord's Polar Night and Snow Storm, with frost for the accent. Polar
/// Night has no shade below nord0, so the rail and sidebar darken it.
const NORD: Theme = Theme {
    dark: true,
    rail: rgb(0x242932),
    sidebar: rgb(0x292e39),
    panel: rgb(0x242932),
    chat: rgb(0x2e3440),
    stage: rgb(0x2e3440),
    raised: rgb(0x3b4252),
    field: rgb(0x3b4252),
    sunken: rgb(0x21252e),
    hover: rgb(0x343b49),
    active: rgb(0x434c5e),
    row_hover: rgb(0x2b313c),
    line: rgb(0x3b4252),
    edge: rgb(0x1f232c),
    text: rgb(0xeceff4),
    secondary: rgb(0xd8dee9),
    muted: rgb(0x99a1b0),
    faint: rgb(0x616e88),
    accent: rgb(0x88c0d0),
    accent_hover: rgb(0xa6cedb),
    on_accent: rgb(0x2e3440),
    accent_soft: rgba(0x88c0d026),
    danger: rgb(0xbf616a),
    danger_hover: rgb(0xc6767f),
    on_danger: rgb(0xeceff4),
    live: rgb(0xa3be8c),
    loud: rgb(0xd08770),
    link: rgb(0x81a1c1),
    button_default: rgb(0x434c5e),
    button_default_hover: rgb(0x4c566a),
    overlay: rgb(0xffffff),
    shimmer: rgba(0xffffff10),
    reply_spine: rgb(0x4c566a),
    tooltip: rgb(0x1f232c),
    avatar: rgb(0x616e88),
};

/// Gruvbox's dark palette at medium contrast, with orange for the accent.
const GRUVBOX_DARK: Theme = Theme {
    dark: true,
    rail: rgb(0x1d2021),
    sidebar: rgb(0x222424),
    panel: rgb(0x1d2021),
    chat: rgb(0x282828),
    stage: rgb(0x282828),
    raised: rgb(0x3c3836),
    field: rgb(0x32302f),
    sunken: rgb(0x171a1a),
    hover: rgb(0x32302f),
    active: rgb(0x504945),
    row_hover: rgb(0x222424),
    line: rgb(0x3c3836),
    edge: rgb(0x141617),
    text: rgb(0xebdbb2),
    secondary: rgb(0xd5c4a1),
    muted: rgb(0xa89984),
    faint: rgb(0x7c6f64),
    accent: rgb(0xfe8019),
    accent_hover: rgb(0xfc9822),
    on_accent: rgb(0x1d2021),
    accent_soft: rgba(0xfe801926),
    danger: rgb(0xfb4934),
    danger_hover: rgb(0xfb6b51),
    on_danger: rgb(0x1d2021),
    live: rgb(0xb8bb26),
    loud: rgb(0xfabd2f),
    link: rgb(0x83a598),
    button_default: rgb(0x504945),
    button_default_hover: rgb(0x665c54),
    overlay: rgb(0xffffff),
    shimmer: rgba(0xffffff10),
    reply_spine: rgb(0x665c54),
    tooltip: rgb(0x141617),
    avatar: rgb(0x7c6f64),
};

/// Tokyo Night, the Night style, with blue for the accent.
const TOKYO_NIGHT: Theme = Theme {
    dark: true,
    rail: rgb(0x13131a),
    sidebar: rgb(0x16161e),
    panel: rgb(0x13131a),
    chat: rgb(0x1a1b26),
    stage: rgb(0x1a1b26),
    raised: rgb(0x1f2335),
    field: rgb(0x1f2335),
    sunken: rgb(0x121218),
    hover: rgb(0x1c1f2e),
    active: rgb(0x292e42),
    row_hover: rgb(0x181822),
    line: rgb(0x292e42),
    edge: rgb(0x0f0f15),
    text: rgb(0xc0caf5),
    secondary: rgb(0xa9b1d6),
    muted: rgb(0x737aa2),
    faint: rgb(0x545c7e),
    accent: rgb(0x7aa2f7),
    accent_hover: rgb(0x80bafa),
    on_accent: rgb(0x16161e),
    accent_soft: rgba(0x7aa2f726),
    danger: rgb(0xf7768e),
    danger_hover: rgb(0xec87a3),
    on_danger: rgb(0x16161e),
    live: rgb(0x9ece6a),
    loud: rgb(0xff9e64),
    link: rgb(0x7dcfff),
    button_default: rgb(0x292e42),
    button_default_hover: rgb(0x3b4261),
    overlay: rgb(0xffffff),
    shimmer: rgba(0xffffff10),
    reply_spine: rgb(0x3b4261),
    tooltip: rgb(0x0f0f15),
    avatar: rgb(0x565f89),
};

/// Rosé Pine's light variant, with rose for the accent.
const ROSE_PINE_DAWN: Theme = Theme {
    dark: false,
    rail: rgb(0xdfdad9),
    sidebar: rgb(0xf2e9e1),
    panel: rgb(0xdfdad9),
    chat: rgb(0xfaf4ed),
    stage: rgb(0xfaf4ed),
    raised: rgb(0xfffaf3),
    field: rgb(0xf2e9e1),
    sunken: rgb(0xf4ede8),
    hover: rgb(0xeae3de),
    active: rgb(0xdfdad9),
    row_hover: rgb(0xf4ede8),
    line: rgb(0xdfdad9),
    edge: rgb(0xcecacd),
    text: rgb(0x575279),
    secondary: rgb(0x797593),
    muted: rgb(0x87829b),
    faint: rgb(0x9893a5),
    accent: rgb(0xd7827e),
    accent_hover: rgb(0xc9767c),
    on_accent: rgb(0x39354f),
    accent_soft: rgba(0xd7827e2e),
    danger: rgb(0xb4637a),
    danger_hover: rgb(0x9d5f7a),
    on_danger: rgb(0xfffaf3),
    live: rgb(0x56949f),
    loud: rgb(0xea9d34),
    link: rgb(0x286983),
    button_default: rgb(0xdfdad9),
    button_default_hover: rgb(0xcecacd),
    overlay: rgb(0x575279),
    shimmer: rgba(0xffffff99),
    reply_spine: rgb(0xcecacd),
    tooltip: rgb(0xfffaf3),
    avatar: rgb(0x9893a5),
};

/// Talk's own: cherry blossom on rice paper, pink on a pale pink ground.
const SAKURA: Theme = Theme {
    dark: false,
    rail: rgb(0xf6dbe5),
    sidebar: rgb(0xfcecf2),
    panel: rgb(0xf6dbe5),
    chat: rgb(0xfff8fb),
    stage: rgb(0xfff8fb),
    raised: rgb(0xfbe9f0),
    field: rgb(0xfbe5ee),
    sunken: rgb(0xf8e1ea),
    hover: rgb(0xf8e2eb),
    active: rgb(0xf2d1de),
    row_hover: rgb(0xfdf1f6),
    line: rgb(0xf2d6e1),
    edge: rgb(0xe9c6d3),
    text: rgb(0x4a3541),
    secondary: rgb(0x6d5361),
    muted: rgb(0x957685),
    faint: rgb(0xa8899a),
    accent: rgb(0xd5588a),
    accent_hover: rgb(0xc44a7b),
    on_accent: rgb(0xffffff),
    accent_soft: rgba(0xd5588a24),
    danger: rgb(0xd2404f),
    danger_hover: rgb(0xbd3242),
    on_danger: rgb(0xffffff),
    live: rgb(0x2f9e6e),
    loud: rgb(0xee8a45),
    link: rgb(0x6a5ad0),
    button_default: rgb(0xf4d6e1),
    button_default_hover: rgb(0xedc6d4),
    overlay: rgb(0x6b2d4a),
    shimmer: rgba(0xffffffa0),
    reply_spine: rgb(0xe2bccb),
    tooltip: rgb(0xffffff),
    avatar: rgb(0xc08aa2),
};

/// A theme Talk comes with: the id a layout stores and the name Appearance
/// shows.
pub struct BuiltIn {
    pub id: &'static str,
    pub name: &'static str,
    pub theme: Theme,
}

/// The themes Talk comes with, as Appearance lists them: the dark ones, then
/// the light ones. The palettes credited in docs/themes.md are MIT licensed.
pub const BUILT_IN: [BuiltIn; 10] = [
    BuiltIn {
        id: "yap",
        name: "Yap",
        theme: YAP,
    },
    BuiltIn {
        id: "bonfire",
        name: "Bonfire",
        theme: BONFIRE,
    },
    BuiltIn {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        theme: CATPPUCCIN_MOCHA,
    },
    BuiltIn {
        id: "dracula",
        name: "Dracula",
        theme: DRACULA,
    },
    BuiltIn {
        id: "nord",
        name: "Nord",
        theme: NORD,
    },
    BuiltIn {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        theme: GRUVBOX_DARK,
    },
    BuiltIn {
        id: "tokyo-night",
        name: "Tokyo Night",
        theme: TOKYO_NIGHT,
    },
    BuiltIn {
        id: "catppuccin-latte",
        name: "Catppuccin Latte",
        theme: CATPPUCCIN_LATTE,
    },
    BuiltIn {
        id: "rose-pine-dawn",
        name: "Rosé Pine Dawn",
        theme: ROSE_PINE_DAWN,
    },
    BuiltIn {
        id: "sakura",
        name: "Sakura",
        theme: SAKURA,
    },
];

pub fn find(id: &str) -> Option<&'static BuiltIn> {
    BUILT_IN.iter().find(|theme| theme.id == id)
}

/// The theme a layout chose: Yap when it chose none, or one this version
/// doesn't have.
pub fn chosen(id: Option<&str>) -> &'static BuiltIn {
    id.and_then(find).unwrap_or(&BUILT_IN[0])
}

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

/// Appearance's cards: each built-in theme's name and the colours its
/// preview draws with.
pub fn cards() -> ModelRc<talk_ui::ThemeCard> {
    let cards: Vec<_> = BUILT_IN
        .iter()
        .map(|built_in| {
            let theme = &built_in.theme;
            talk_ui::ThemeCard {
                id: built_in.id.into(),
                name: built_in.name.into(),
                rail: theme.rail,
                sidebar: theme.sidebar,
                chat: theme.chat,
                raised: theme.raised,
                accent: theme.accent,
                text: theme.text,
                muted: theme.muted,
            }
        })
        .collect();
    ModelRc::new(VecModel::from(cards))
}

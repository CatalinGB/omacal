//! The theme vocabulary the service layer needs, kept Tauri-free. The
//! *resolution* — reading an Omarchy theme off disk and turning a choice into
//! a palette — stays in `src-tauri`; here are only the choice itself (an
//! enum, no I/O) and the one filesystem probe the settings read depends on.

use serde::{Deserialize, Serialize};

/// Which palette the app wears.
///
/// `Auto` stays the default: the Omarchy theme if there is one, and
/// otherwise the system's own light-or-dark — which until 2026-09-24 it never
/// asked, so every Mac on `Auto` was dark for good.
///
/// `Light` and `Dark` are the user overruling that, and they take the built-in
/// palette **whole** — including on Omarchy, where the theme's own accent is
/// deliberately *not* kept. An accent picked to sit on a dark terminal has no
/// obligation to be legible on white (Omarchy ships pale yellows and washed
/// greens), and the entire point of choosing a palette explicitly is getting
/// one that works. Someone who wants their theme's colours wants `Auto`.
///
/// The four named themes are the same kind of choice — `Palette::named` on the
/// desktop side — spelled as Omarchy spells its themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Auto,
    Light,
    Dark,
    #[serde(rename = "tokyo-night")]
    TokyoNight,
    #[serde(rename = "catppuccin-mocha")]
    CatppuccinMocha,
    #[serde(rename = "catppuccin-latte")]
    CatppuccinLatte,
    #[serde(rename = "rose-pine-dawn")]
    RosePineDawn,
}

impl Appearance {
    /// The stored spelling — the same string the wire uses, so a row read by
    /// eye in `sqlite3` says what the settings modal says.
    pub fn as_str(self) -> &'static str {
        match self {
            Appearance::Auto => "auto",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
            Appearance::TokyoNight => "tokyo-night",
            Appearance::CatppuccinMocha => "catppuccin-mocha",
            Appearance::CatppuccinLatte => "catppuccin-latte",
            Appearance::RosePineDawn => "rose-pine-dawn",
        }
    }

    /// The stored spelling read back: `as_str`'s inverse, and `None` for
    /// anything this version did not write.
    pub fn parse(stored: &str) -> Option<Self> {
        [
            Appearance::Auto, Appearance::Light, Appearance::Dark, Appearance::TokyoNight,
            Appearance::CatppuccinMocha, Appearance::CatppuccinLatte, Appearance::RosePineDawn,
        ]
        .into_iter()
        .find(|a| a.as_str() == stored)
    }

    /// Whether this choice is the user's own rather than the desktop's — the
    /// question `theme_watch` asks before repainting on a theme file it has
    /// been told not to follow.
    pub fn is_pinned(self) -> bool {
        !matches!(self, Appearance::Auto)
    }
}

/// The conventional Omarchy location. Returns `None` off Linux or when absent.
///
/// Omarchy 4 keeps the live theme under the state dir (a hardcoded
/// `~/.local/state` in Omarchy's own scripts, so no XDG lookup here); the
/// `~/.config` path is where every earlier Omarchy put it.
pub fn omarchy_theme_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let home = std::path::Path::new(&home);
    [
        ".local/state/omarchy/current/theme",
        ".config/omarchy/current/theme",
    ]
    .iter()
    .map(|rel| home.join(rel))
    .find(|p| p.exists())
}

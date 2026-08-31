//! Named theme resolution and window chrome.
//!
//! Boundary. Config owns parse and keys. Settings persist strings. Palette
//! owns RGB constructors. This module maps those plus `WindowAppearance` and
//! `AccessibilityPolicy` into a runtime `ResolvedChrome`. It never calls
//! `AccessibilityPolicy::detect()`.

use gpui::WindowAppearance;
use mr_crabs_element::TerminalPalette;

use crate::accessibility_policy::AccessibilityPolicy;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeId {
    Auto,
    Ink,
    Paper,
    Harbor,
    Ember,
}

impl ThemeId {
    pub fn parse(value: &str) -> Self {
        match value {
            "ink" | "dark" => Self::Ink,
            "paper" | "light" => Self::Paper,
            "harbor" => Self::Harbor,
            "ember" => Self::Ember,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ink => "ink",
            Self::Paper => "paper",
            Self::Harbor => "harbor",
            Self::Ember => "ember",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowMaterial {
    Opaque,
    Blurred { radius: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedChrome {
    pub theme: ThemeId,
    pub palette: TerminalPalette,
    pub material: WindowMaterial,
}

pub fn resolve_chrome(
    theme: &str,
    background_opacity: f32,
    background_blur: u16,
    appearance: WindowAppearance,
    policy: AccessibilityPolicy,
) -> ResolvedChrome {
    let requested = ThemeId::parse(theme);
    let resolved = match requested {
        ThemeId::Auto => match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeId::Paper,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeId::Ink,
        },
        named => named,
    };
    let effective_opacity = if policy.allows_transparency() {
        background_opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let effective_blur = if policy.allows_transparency() && effective_opacity < 1.0 {
        background_blur
    } else {
        0
    };
    let palette = match resolved {
        ThemeId::Auto => unreachable!("auto is resolved before palette construction"),
        ThemeId::Ink => TerminalPalette::ink(effective_opacity),
        ThemeId::Paper => TerminalPalette::paper(effective_opacity),
        ThemeId::Harbor => TerminalPalette::harbor(effective_opacity),
        ThemeId::Ember => TerminalPalette::ember(effective_opacity),
    };
    let material = if effective_blur == 0 {
        WindowMaterial::Opaque
    } else {
        WindowMaterial::Blurred {
            radius: effective_blur,
        }
    };
    ResolvedChrome {
        theme: resolved,
        palette,
        material,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_light_keeps_translucency_when_allowed() {
        let chrome = resolve_chrome(
            "auto",
            0.8,
            20,
            WindowAppearance::Light,
            AccessibilityPolicy::from_flags(false, false),
        );
        assert_eq!(chrome.theme, ThemeId::Paper);
        assert_eq!(chrome.palette, TerminalPalette::paper(0.8));
        assert_eq!(chrome.material, WindowMaterial::Blurred { radius: 20 });
    }

    #[test]
    fn reduce_transparency_forces_opaque_paper() {
        let chrome = resolve_chrome(
            "auto",
            0.8,
            20,
            WindowAppearance::Light,
            AccessibilityPolicy::from_flags(false, true),
        );
        assert_eq!(chrome.theme, ThemeId::Paper);
        assert_eq!(chrome.palette, TerminalPalette::paper(1.0));
        assert_eq!(chrome.material, WindowMaterial::Opaque);
    }

    #[test]
    fn named_harbor_ignores_light_desktop() {
        let chrome = resolve_chrome(
            "harbor",
            0.8,
            20,
            WindowAppearance::Light,
            AccessibilityPolicy::from_flags(false, false),
        );
        assert_eq!(chrome.theme, ThemeId::Harbor);
        assert_eq!(chrome.palette.background, [0x12, 0x16, 0x1c]);
        assert_eq!(chrome.palette.foreground, [0xd7, 0xe0, 0xea]);
        assert!((chrome.palette.background_opacity - 0.8).abs() < f32::EPSILON);
        assert_eq!(chrome.material, WindowMaterial::Blurred { radius: 20 });
    }

    #[test]
    fn aliases_map_to_ink_and_paper() {
        assert_eq!(ThemeId::parse("dark"), ThemeId::Ink);
        assert_eq!(ThemeId::parse("light"), ThemeId::Paper);
    }

    #[test]
    fn opaque_user_opacity_drops_blur() {
        let chrome = resolve_chrome(
            "ink",
            1.0,
            20,
            WindowAppearance::Dark,
            AccessibilityPolicy::from_flags(false, false),
        );
        assert_eq!(chrome.material, WindowMaterial::Opaque);
    }
}

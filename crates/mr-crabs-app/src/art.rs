//! Startup art registry: named text logos plus on-disk custom files.

use std::path::Path;
use std::sync::LazyLock;

use mr_crabs_config::{StartupArt, StaticStartupArt};

const APPLE_ART: &str = include_str!("../../../resources/art/apple.txt");

/// One named block of startup art.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Art {
    pub name: String,
    pub rows: Vec<String>,
    pub width: usize,
    pub height: usize,
}

impl Art {
    fn from_text(name: impl Into<String>, text: &str) -> Option<Self> {
        let rows: Vec<String> = text
            .lines()
            .map(|line| line.trim_end().to_string())
            .collect();
        let rows: Vec<String> = {
            let start = rows.iter().position(|r| !r.is_empty()).unwrap_or(0);
            let end = rows
                .iter()
                .rposition(|r| !r.is_empty())
                .map(|i| i + 1)
                .unwrap_or(0);
            if start >= end {
                Vec::new()
            } else {
                rows[start..end].to_vec()
            }
        };
        if rows.is_empty() {
            return None;
        }
        let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        if width == 0 {
            return None;
        }
        let height = rows.len();
        Some(Self {
            name: name.into(),
            rows,
            width,
            height,
        })
    }

    /// Top-left of centred art in a viewport, in pixels.
    pub fn centered_origin(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> (f32, f32) {
        let art_w = self.width as f32 * cell_width;
        let art_h = self.height as f32 * cell_height;
        (
            (viewport_width - art_w) * 0.5,
            (viewport_height - art_h) * 0.5,
        )
    }
}

struct Builtin {
    name: &'static str,
    text: &'static str,
}

const BUILTINS: &[Builtin] = &[Builtin {
    name: "apple",
    text: APPLE_ART,
}];

fn builtins() -> &'static [Art] {
    static CACHE: LazyLock<Vec<Art>> = LazyLock::new(|| {
        BUILTINS
            .iter()
            .filter_map(|entry| Art::from_text(entry.name, entry.text))
            .collect()
    });
    CACHE.as_slice()
}
fn builtin(name: &str) -> Option<Art> {
    builtins().iter().find(|art| art.name == name).cloned()
}

fn load_custom(path: &Path) -> Option<Art> {
    let text = std::fs::read_to_string(path).ok()?;
    Art::from_text(path.display().to_string(), &text)
}

/// Resolve selectable startup art. Missing custom files fall back to none.
pub fn art_for_startup(art: &StartupArt) -> Option<Art> {
    match art {
        StartupArt::None => None,
        StartupArt::Native | StartupArt::Apple => builtin("apple"),
        StartupArt::Custom(path) => load_custom(path).or_else(|| builtin("apple")),
    }
}

/// Resolve molt-only art. Native is already folded to [`StaticStartupArt::None`].
pub fn art_for_static(art: &StaticStartupArt) -> Option<Art> {
    match art {
        StaticStartupArt::None => None,
        StaticStartupArt::Apple => builtin("apple"),
        StaticStartupArt::Custom(path) => load_custom(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn apple_art_has_nonzero_dimensions() {
        let art = builtin("apple").expect("apple");
        assert_eq!(art.name, "apple");
        assert!(art.width > 0, "width {}", art.width);
        assert!(art.height > 0, "height {}", art.height);
        assert_eq!(art.height, art.rows.len());
        assert!(art.rows.iter().any(|row| !row.is_empty()));
    }

    #[test]
    fn missing_custom_path_falls_back() {
        let missing = PathBuf::from("/definitely/missing/mr-crabs-art-does-not-exist.txt");
        assert!(load_custom(&missing).is_none());
        assert!(
            art_for_static(&StaticStartupArt::Custom(missing.clone())).is_none(),
            "static custom must not error"
        );
        let startup = art_for_startup(&StartupArt::Custom(missing));
        let apple = builtin("apple");
        assert_eq!(startup, apple, "startup custom falls back to apple");
        assert!(art_for_static(&StaticStartupArt::None).is_none());
        assert!(art_for_startup(&StartupArt::None).is_none());
        assert!(
            art_for_startup(&StartupArt::Native).is_some(),
            "native is built-in art now that no external program supplies a logo"
        );
    }

    #[test]
    fn art_is_centred_for_viewport_and_cell_size() {
        let art = builtin("apple").expect("apple");
        let cell_w = 8.0_f32;
        let cell_h = 16.0_f32;
        let vw = 800.0_f32;
        let vh = 600.0_f32;
        let (left, top) = art.centered_origin(vw, vh, cell_w, cell_h);
        let expected_left = (vw - art.width as f32 * cell_w) * 0.5;
        let expected_top = (vh - art.height as f32 * cell_h) * 0.5;
        assert!((left - expected_left).abs() < f32::EPSILON);
        assert!((top - expected_top).abs() < f32::EPSILON);
        assert!(left > 0.0 && top > 0.0);
        assert!(left + art.width as f32 * cell_w <= vw);
        assert!(top + art.height as f32 * cell_h <= vh);
    }
}

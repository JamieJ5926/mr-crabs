use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde_json::{Map, Value};

/// Config key holding the text-animation choice.
pub const KEY_TEXT_ANIMATION: &str = "text_animation";
/// Config key holding the cursor-trail flag.
pub const KEY_CURSOR_TRAIL: &str = "cursor_trail";

/// Save verb: disable text animation.
pub const TEXT_ANIMATION_NONE: &str = "none";
/// Save verb: left-to-right streaming reveal.
pub const TEXT_ANIMATION_STREAMING: &str = "streaming";
/// Save verb: row-staggered typewriter reveal.
pub const TEXT_ANIMATION_TYPEWRITER: &str = "typewriter";

/// The exact save verbs accepted by [`save_animation_config`].
/// (`inherit` is restore-only and is never persisted.)
pub const TEXT_ANIMATION_CHOICES: [&str; 3] = [
    TEXT_ANIMATION_NONE,
    TEXT_ANIMATION_STREAMING,
    TEXT_ANIMATION_TYPEWRITER,
];

/// Typed failure of a config save, one variant per OSC save reply code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnimationConfigError {
    /// Bad save request or a config document that cannot host a config
    /// object (reply code `invalid`).
    Invalid(String),
    /// No config file path is available (reply code `no-path`).
    NoPath,
    /// Filesystem failure: read, mkdir, temp write, or rename (code `io`).
    Io(String),
    /// Config document could not be parsed or serialized (code `json`).
    Json(String),
}

impl AnimationConfigError {
    /// OSC save reply code: `invalid`, `no-path`, `io`, or `json`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid",
            Self::NoPath => "no-path",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
        }
    }
}

impl Display for AnimationConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(e) => write!(f, "invalid animation config: {e}"),
            Self::NoPath => write!(f, "no animation config path"),
            Self::Io(e) => write!(f, "animation config io error: {e}"),
            Self::Json(e) => write!(f, "animation config json error: {e}"),
        }
    }
}

impl std::error::Error for AnimationConfigError {}

/// Merge `text_animation = <text>` and `cursor_trail = <cursor_trail>` into
/// the JSON config at `path`, preserving all unrelated and unknown keys.
///
/// A missing file starts from `{}`; malformed JSON is rejected with
/// [`AnimationConfigError::Json`] and valid non-object documents with
/// [`AnimationConfigError::Invalid`] — in both cases the file is left
/// untouched. The parent directory is created if needed and the write lands
/// via a sibling temp file renamed over the target, so readers never observe
/// a partial document.
pub fn save_animation_config(
    path: &Path,
    text: &str,
    cursor_trail: bool,
) -> Result<(), AnimationConfigError> {
    if path.as_os_str().is_empty() {
        return Err(AnimationConfigError::NoPath);
    }
    if !TEXT_ANIMATION_CHOICES.contains(&text) {
        return Err(AnimationConfigError::Invalid(format!(
            "text choice must be one of {}, got {text:?}",
            TEXT_ANIMATION_CHOICES.join(", ")
        )));
    }

    let mut config = read_config(path)?;
    let object = config.as_object_mut().ok_or_else(|| {
        AnimationConfigError::Invalid(format!("config {} is not a JSON object", path.display()))
    })?;
    object.insert(
        KEY_TEXT_ANIMATION.to_string(),
        Value::String(text.to_string()),
    );
    object.insert(KEY_CURSOR_TRAIL.to_string(), Value::Bool(cursor_trail));

    write_config(path, &config)
}

fn read_config(path: &Path) -> Result<Value, AnimationConfigError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| AnimationConfigError::Json(format!("parse {}: {e}", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(e) => Err(AnimationConfigError::Io(format!(
            "read {}: {e}",
            path.display()
        ))),
    }
}

fn write_config(path: &Path, config: &Value) -> Result<(), AnimationConfigError> {
    let mut bytes = serde_json::to_string_pretty(config)
        .map_err(|e| AnimationConfigError::Json(format!("serialize {}: {e}", path.display())))?;
    bytes.push('\n');

    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent).map_err(|e| {
            AnimationConfigError::Io(format!("create_dir_all {}: {e}", parent.display()))
        })?;
    }
    let dir = parent.unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| AnimationConfigError::Io(format!("no file name in {}", path.display())))?;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp = dir.join(format!(
        ".{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        unique,
    ));

    let result = (|| {
        let mut file = fs::File::create(&temp)
            .map_err(|e| AnimationConfigError::Io(format!("create {}: {e}", temp.display())))?;
        file.write_all(bytes.as_bytes())
            .map_err(|e| AnimationConfigError::Io(format!("write {}: {e}", temp.display())))?;
        file.sync_all()
            .map_err(|e| AnimationConfigError::Io(format!("sync {}: {e}", temp.display())))?;
        // Keep the target's permissions (e.g. 0600) through the rename.
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&temp, meta.permissions())
                .map_err(|e| AnimationConfigError::Io(format!("chmod {}: {e}", temp.display())))?;
        }
        fs::rename(&temp, path).map_err(|e| {
            AnimationConfigError::Io(format!(
                "rename {} -> {}: {e}",
                temp.display(),
                path.display()
            ))
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "mr-crabs-animation-config-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("create test dir");
            Self(path)
        }

        fn path(&self, file: &str) -> PathBuf {
            self.0.join(file)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn leftover_temps(dir: &Path) -> Vec<String> {
        let mut names = fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(".tmp"))
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).expect("read config")).expect("valid json")
    }

    #[test]
    fn missing_file_is_created_with_exact_keys_under_new_parents() {
        let dir = TestDir::new("missing");
        let path = dir.path("a/b/c/config.json");
        save_animation_config(&path, TEXT_ANIMATION_TYPEWRITER, true).expect("save");

        let doc = read_json(&path);
        assert_eq!(
            doc,
            serde_json::json!({
                KEY_TEXT_ANIMATION: "typewriter",
                KEY_CURSOR_TRAIL: true,
            })
        );
        assert!(leftover_temps(&dir.0).is_empty());
    }

    #[test]
    fn unrelated_and_unknown_keys_are_preserved_verbatim() {
        let dir = TestDir::new("preserve");
        let path = dir.path("config.json");
        fs::write(
            &path,
            r#"{"theme":"dark","keybindings":[{"action":"quit"}],"unknown_x":42,"text_animation":"streaming","nested":{"a":[1,true,null]}}"#,
        )
        .expect("seed config");

        save_animation_config(&path, TEXT_ANIMATION_NONE, false).expect("save");

        let doc = read_json(&path);
        assert_eq!(doc["theme"], "dark");
        assert_eq!(doc["keybindings"], serde_json::json!([{"action": "quit"}]));
        assert_eq!(doc["unknown_x"], 42);
        assert_eq!(doc["nested"], serde_json::json!({"a": [1, true, null]}));
        assert_eq!(doc[KEY_TEXT_ANIMATION], "none");
        assert_eq!(doc[KEY_CURSOR_TRAIL], false);
        assert_eq!(doc.as_object().expect("object").len(), 6);
        assert!(leftover_temps(&dir.0).is_empty());
    }

    #[test]
    fn malformed_json_is_rejected_without_overwriting() {
        let dir = TestDir::new("malformed");
        let path = dir.path("config.json");
        let original = "{ not json !!!";
        fs::write(&path, original).expect("seed config");

        let err = save_animation_config(&path, TEXT_ANIMATION_STREAMING, true).unwrap_err();
        assert_eq!(err.code(), "json");
        assert_eq!(fs::read_to_string(&path).expect("read back"), original);
        assert!(leftover_temps(&dir.0).is_empty());
    }

    #[test]
    fn non_object_json_is_rejected_without_overwriting() {
        for original in ["[1,2,3]", "\"streaming\"", "42", "null"] {
            let dir = TestDir::new("non-object");
            let path = dir.path("config.json");
            fs::write(&path, original).expect("seed config");

            let err = save_animation_config(&path, TEXT_ANIMATION_STREAMING, true).unwrap_err();
            assert_eq!(err.code(), "invalid");
            assert_eq!(fs::read_to_string(&path).expect("read back"), original);
            assert!(leftover_temps(&dir.0).is_empty());
        }
    }

    #[test]
    fn invalid_text_choice_is_rejected_without_writing() {
        let dir = TestDir::new("invalid-choice");
        let path = dir.path("config.json");
        let original = r#"{"theme":"dark"}"#;
        fs::write(&path, original).expect("seed config");

        let err = save_animation_config(&path, "inherit", false).unwrap_err();
        assert_eq!(err.code(), "invalid");
        assert_eq!(fs::read_to_string(&path).expect("read back"), original);
        assert!(leftover_temps(&dir.0).is_empty());
    }

    #[test]
    fn empty_path_reports_no_path() {
        assert_eq!(
            save_animation_config(Path::new(""), TEXT_ANIMATION_NONE, false).unwrap_err(),
            AnimationConfigError::NoPath
        );
    }

    #[test]
    fn unreachable_parent_reports_io() {
        let dir = TestDir::new("io");
        let blocker = dir.path("blocker");
        fs::write(&blocker, "i am a file").expect("seed blocker");
        let path = blocker.join("config.json");

        let err = save_animation_config(&path, TEXT_ANIMATION_NONE, true).unwrap_err();
        assert_eq!(err.code(), "io");
        assert!(leftover_temps(&dir.0).is_empty());
    }

    #[test]
    fn error_codes_match_osc_save_reply_codes() {
        assert_eq!(AnimationConfigError::Invalid("x".into()).code(), "invalid");
        assert_eq!(AnimationConfigError::NoPath.code(), "no-path");
        assert_eq!(AnimationConfigError::Io("x".into()).code(), "io");
        assert_eq!(AnimationConfigError::Json("x".into()).code(), "json");
    }
}

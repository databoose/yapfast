//! Installed Steam games supply native Linux/macOS names even when Discord's
//! catalogue only has a Windows executable. Manifests are read at most once a
//! minute, never by walking installed game files.

use super::catalog::{Catalog, GameInfo, normalize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Default, PartialEq, Eq)]
pub(super) struct Steam {
    installed: Vec<(String, Arc<GameInfo>)>,
}

fn read_small(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > 256 * 1024 {
        return None;
    }
    let mut content = String::new();
    file.take(256 * 1024 + 1)
        .read_to_string(&mut content)
        .ok()?;
    (content.len() <= 256 * 1024).then_some(content)
}

/// Valve's text KeyValues: consume quoted key/value pairs without interpreting
/// strings as code, and handle the escaped paths Windows manifests use.
fn pairs(text: &str) -> Vec<(String, String)> {
    let mut chars = text.chars().peekable();
    let mut key = None;
    let mut result = Vec::new();
    while let Some(ch) = chars.next() {
        match ch {
            '{' | '}' => key = None,
            '/' if chars.peek() == Some(&'/') => {
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
            }
            '"' => {
                let mut value = String::new();
                while let Some(ch) = chars.next() {
                    match ch {
                        '"' => break,
                        '\\' => {
                            if let Some(next) = chars.next() {
                                if next != '\\' && next != '"' {
                                    value.push('\\');
                                }
                                value.push(next);
                            }
                        }
                        _ => value.push(ch),
                    }
                }
                if let Some(key) = key.take() {
                    result.push((key, value));
                } else {
                    key = Some(value);
                }
            }
            _ => {}
        }
    }
    result
}

pub(super) fn roots() -> Vec<PathBuf> {
    if let Some(root) = std::env::var_os("YAPFAST_STEAM_DIR") {
        return vec![root.into()];
    }
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    if cfg!(target_os = "windows") {
        [
            std::env::var_os("ProgramFiles(x86)"),
            std::env::var_os("ProgramFiles"),
        ]
        .into_iter()
        .flatten()
        .map(|path| PathBuf::from(path).join("Steam"))
        .collect()
    } else if cfg!(target_os = "macos") {
        vec![home.join("Library/Application Support/Steam")]
    } else {
        vec![
            home.join(".local/share/Steam"),
            home.join(".steam/steam"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        ]
    }
}

impl Steam {
    pub(super) fn load(roots: &[PathBuf], catalog: &Catalog) -> Self {
        let mut libraries = Vec::new();
        for root in roots {
            libraries.push(root.clone());
            if let Some(vdf) = read_small(&root.join("steamapps/libraryfolders.vdf")) {
                for (key, value) in pairs(&vdf) {
                    if key == "path" {
                        libraries.push(value.into());
                    }
                }
            }
        }
        libraries = libraries
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        libraries.sort();
        libraries.dedup();
        let mut installed = Vec::new();
        for library in libraries {
            let Ok(files) = fs::read_dir(library.join("steamapps")) else {
                continue;
            };
            for file in files.flatten().take(10_000) {
                let name = file.file_name();
                let name = name.to_string_lossy();
                let Some(id) = name
                    .strip_prefix("appmanifest_")
                    .and_then(|s| s.strip_suffix(".acf"))
                else {
                    continue;
                };
                if !id.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                let Some(content) = read_small(&file.path()) else {
                    continue;
                };
                let pairs = pairs(&content);
                let Some((_, directory)) = pairs.iter().find(|(key, _)| key == "installdir") else {
                    continue;
                };
                // A manifest must not turn an arbitrary parent directory into
                // a game match. Install directories are single path components.
                if directory.is_empty()
                    || directory == "."
                    || directory == ".."
                    || directory.contains(['/', '\\'])
                {
                    continue;
                }
                let Some((_, name)) = pairs.iter().find(|(key, _)| key == "name") else {
                    continue;
                };
                if name.trim().is_empty() || name.chars().count() > 128 {
                    continue;
                }
                let game = catalog.steam_game(id).unwrap_or_else(|| {
                    Arc::new(GameInfo {
                        id: None,
                        name: name.as_str().into(),
                    })
                });
                let path = library.join("steamapps/common").join(directory);
                let path = path.canonicalize().unwrap_or(path);
                installed.push((
                    normalize(&path.to_string_lossy(), cfg!(target_os = "windows")),
                    game,
                ));
            }
        }
        // If a library has nested installation directories, the most specific
        // matching installation wins; directories are still few and bounded.
        installed.sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        Self { installed }
    }

    pub(super) fn matches(&self, path: &str) -> Option<Arc<GameInfo>> {
        let path = normalize(path, cfg!(target_os = "windows"));
        let path = if cfg!(target_os = "linux") {
            path.strip_prefix("Z:")
                .or_else(|| path.strip_prefix("z:"))
                .unwrap_or(&path)
        } else {
            &path
        };
        self.installed
            .iter()
            .find(|(root, _)| {
                path == root
                    || path
                        .strip_prefix(root)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|(_, game)| game.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valve_paths_and_ignores_commented_entries() {
        assert_eq!(
            pairs(
                r#""libraryfolders" { "0" { "path" "C:\\Games\\Steam" } // "path" "wrong"
        "1" { "path" "/mnt/Games" } }"#
            ),
            vec![
                ("path".into(), "C:\\Games\\Steam".into()),
                ("path".into(), "/mnt/Games".into())
            ]
        );
    }

    #[test]
    fn steam_matches_only_inside_an_installation_and_uses_catalog_identity() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("steamapps")).unwrap();
        fs::write(
            dir.path().join("steamapps/appmanifest_42.acf"),
            r#""AppState" { "name" "Unknown Steam Game" "installdir" "Fixture" }"#,
        )
        .unwrap();
        let steam = Steam::load(&[dir.path().into()], &Catalog::default());
        let path = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("steamapps/common/Fixture/game");
        assert_eq!(
            &*steam.matches(&path.to_string_lossy()).unwrap().name,
            "Unknown Steam Game"
        );
        assert!(
            steam
                .matches(
                    &path
                        .to_string_lossy()
                        .replace("Fixture/game", "FixtureOther/game")
                )
                .is_none()
        );
        assert!(
            steam
                .matches(
                    &dir.path()
                        .join("steamapps/common/Proton/wine")
                        .to_string_lossy()
                )
                .is_none()
        );
    }
}

//! A compact, platform-filtered version of Discord's public /games/detectable.
//! Only game identity, executable rules and Steam ids survive deserialization.

use crate::api::Api;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

const MAX_FILE: u64 = 32 * 1024 * 1024;
pub(super) const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Deserialize, Serialize)]
struct Game {
    #[serde(default)]
    id: Option<String>,
    name: Box<str>,
    #[serde(default)]
    executables: Vec<Executable>,
    #[serde(default)]
    third_party_skus: Vec<Sku>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Executable {
    name: String,
    os: String,
    #[serde(default)]
    is_launcher: bool,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Sku {
    distributor: String,
    id: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct GameInfo {
    pub(super) id: Option<u64>,
    pub(super) name: Box<str>,
}

struct Rule {
    suffix: Box<str>,
    windows: bool,
    arguments: Option<Box<str>>,
    game: Arc<GameInfo>,
}

#[derive(Default)]
pub(super) struct Catalog {
    rules: HashMap<Box<str>, Vec<Rule>>,
    steam: HashMap<u64, Arc<GameInfo>>,
}

pub(super) fn host_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

fn read_games(path: &Path, visit: impl FnMut(Game) -> Result<()>) -> Result<usize> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_FILE,
        "game catalogue exceeds 32 MiB"
    );
    // Filter each entry as it is read. Keeping all platforms/SKUs until the
    // complete 13+ MiB response is parsed needlessly doubles peak allocations.
    struct Games<F>(F);
    impl<'de, F: FnMut(Game) -> Result<()>> serde::de::Visitor<'de> for Games<F> {
        type Value = usize;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a game catalogue array")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            mut self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut count = 0;
            while let Some(mut game) = seq.next_element::<Game>()? {
                game.executables.retain(|exe| {
                    !exe.is_launcher
                        && !exe.name.is_empty()
                        && (exe.os == host_os() || (cfg!(target_os = "linux") && exe.os == "win32"))
                });
                game.third_party_skus
                    .retain(|sku| sku.distributor == "steam" && sku.id.is_some());
                if !game.name.trim().is_empty()
                    && game.name.chars().count() <= 128
                    && game
                        .id
                        .as_deref()
                        .is_none_or(|id| id.parse::<u64>().is_ok_and(|n| n > 0))
                    && (!game.executables.is_empty() || !game.third_party_skus.is_empty())
                {
                    (self.0)(game).map_err(serde::de::Error::custom)?;
                    count += 1;
                }
            }
            Ok(count)
        }
    }
    use serde::Deserializer;
    let mut decoder = serde_json::Deserializer::from_reader(BufReader::new(file));
    let count = (&mut decoder).deserialize_seq(Games(visit))?;
    decoder.end()?;
    Ok(count)
}

impl Catalog {
    pub(super) fn read(path: &Path) -> Result<Self> {
        let mut catalog = Self::default();
        read_games(path, |game| {
            catalog.add_game(game);
            Ok(())
        })?;
        Ok(catalog)
    }

    #[cfg(test)]
    fn from_games(games: Vec<Game>) -> Self {
        let mut catalog = Self::default();
        for game in games {
            catalog.add_game(game);
        }
        catalog
    }

    fn add_game(&mut self, game: Game) {
        let info = Arc::new(GameInfo {
            id: game.id.as_deref().and_then(|id| id.parse().ok()),
            name: game.name,
        });
        for sku in game.third_party_skus {
            if let Some(id) = sku.id.and_then(|id| id.parse::<u64>().ok()) {
                self.steam.entry(id).or_insert_with(|| info.clone());
            }
        }
        for exe in game.executables {
            let windows = exe.os == "win32";
            let suffix = normalize(
                exe.name.trim_start_matches('>'),
                windows || exe.os == "darwin",
            );
            let Some(file) = suffix.rsplit('/').next().filter(|s| !s.is_empty()) else {
                continue;
            };
            self.rules
                .entry(file.to_lowercase().into_boxed_str())
                // Most executable names have one rule. Vec's default first
                // allocation reserves four, wasting space in thousands of buckets.
                .or_insert_with(|| Vec::with_capacity(1))
                .push(Rule {
                    suffix: suffix.into_boxed_str(),
                    windows,
                    arguments: exe.arguments.map(String::into_boxed_str),
                    game: info.clone(),
                });
        }
    }

    pub(super) fn matches(
        &self,
        path: &str,
        args: &[std::ffi::OsString],
        windows: bool,
    ) -> Option<Arc<GameInfo>> {
        if cfg!(target_os = "macos") && !windows {
            return self.matches_macos(path, args);
        }
        let path = normalize(path, windows);
        self.match_path(&path, args, windows)
    }

    fn matches_macos(&self, path: &str, args: &[std::ffi::OsString]) -> Option<Arc<GameInfo>> {
        let path = normalize(path, true);
        self.match_path(&path, args, false).or_else(|| {
            // Discord names macOS app bundles, while sysinfo supplies the
            // executable inside App.app/Contents/MacOS/. Match the outer app.
            let end = path.find(".app/")? + 4;
            self.match_path(&path[..end], args, false)
        })
    }

    fn match_path(
        &self,
        path: &str,
        args: &[std::ffi::OsString],
        windows: bool,
    ) -> Option<Arc<GameInfo>> {
        let file = path.rsplit('/').next()?;
        self.rules
            .get(file.to_lowercase().as_str())?
            .iter()
            .find(|rule| {
                rule.windows == windows
                    && suffix_matches(path, &rule.suffix)
                    && rule
                        .arguments
                        .as_ref()
                        .is_none_or(|expected| arguments_match(args, expected))
            })
            .map(|rule| rule.game.clone())
    }

    pub(super) fn steam_game(&self, id: &str) -> Option<Arc<GameInfo>> {
        self.steam.get(&id.parse::<u64>().ok()?).cloned()
    }

    pub(super) fn needs_arguments(&self, path: &str) -> bool {
        let file = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path)
            .to_lowercase();
        self.rules
            .get(file.as_str())
            .is_some_and(|rules| rules.iter().any(|rule| rule.arguments.is_some()))
    }

    pub(super) fn rule_count(&self) -> usize {
        self.rules.values().map(Vec::len).sum()
    }
}

pub(super) fn normalize(path: &str, windows: bool) -> String {
    let mut path = path.replace('\\', "/");
    if windows {
        if let Some(rest) = path.strip_prefix("//?/UNC/") {
            path = format!("//{rest}");
        } else if path.starts_with("//?/") {
            path.drain(..4);
        }
        path.to_lowercase()
    } else {
        path
    }
}

fn suffix_matches(path: &str, suffix: &str) -> bool {
    path == suffix
        || path
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('/'))
}

fn arguments_match(args: &[std::ffi::OsString], expected: &str) -> bool {
    if args
        .iter()
        .any(|arg| arg.to_string_lossy().contains(expected))
    {
        return true;
    }
    let mut command = String::new();
    for arg in args {
        if !command.is_empty() {
            command.push(' ');
        }
        command.push_str(&arg.to_string_lossy());
    }
    command.contains(expected)
}

pub(super) fn fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < MAX_AGE)
}

/// Download and parse off the runtime; replace the cache only after validating
/// the complete response. A failed refresh leaves the earlier catalogue intact.
pub(super) async fn refresh(api: &Api, cache: Option<PathBuf>) -> Result<Arc<Catalog>> {
    let temp = tokio::task::spawn_blocking(move || -> Result<_> {
        let temp = match cache.as_ref().and_then(|p| p.parent()) {
            Some(parent) => {
                std::fs::create_dir_all(parent)?;
                tempfile::NamedTempFile::new_in(parent)?
            }
            None => tempfile::NamedTempFile::new()?,
        };
        Ok((temp, cache))
    })
    .await??;
    tokio::time::timeout(
        Duration::from_secs(30),
        api.detectable_games(temp.0.reopen()?),
    )
    .await??;
    tokio::task::spawn_blocking(move || -> Result<_> {
        let (temp, cache) = temp;
        Ok(Arc::new(compact(temp.path(), cache.as_deref())?))
    })
    .await?
}

/// Build indexes and the compact cache one game at a time. The download stays
/// readable until validation finishes, and a bad response never replaces cache.
fn compact(source: &Path, cache: Option<&Path>) -> Result<Catalog> {
    let mut output = cache
        .map(|path| {
            tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
        })
        .transpose()?;
    let mut writer = output
        .as_mut()
        .map(|file| BufWriter::new(file.as_file_mut()));
    if let Some(writer) = &mut writer {
        writer.write_all(b"[")?;
    }
    let mut catalog = Catalog::default();
    let mut first = true;
    let count = read_games(source, |game| {
        if let Some(writer) = &mut writer {
            if !first {
                writer.write_all(b",")?;
            }
            serde_json::to_writer(writer, &game)?;
        }
        first = false;
        catalog.add_game(game);
        Ok(())
    })
    .context("invalid game catalogue")?;
    ensure!(count > 0, "game catalogue contains no supported games");
    if let Some(writer) = &mut writer {
        writer.write_all(b"]")?;
        writer.flush()?;
    }
    drop(writer);
    if let (Some(file), Some(path)) = (output, cache) {
        file.persist(path)?;
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_cache_streams_supported_entries_and_preserves_old_cache_on_error() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("download.json");
        let cache = directory.path().join("cache.json");
        let game = json!({"id":"42","name":"Game","description":"discard this", "executables":[{"os":host_os(),"name":"game"}]});
        std::fs::write(&source, serde_json::to_vec(&[game]).unwrap()).unwrap();
        let indexed = compact(&source, Some(&cache)).unwrap();
        assert_eq!(indexed.rule_count(), 1);
        assert_eq!(Catalog::read(&cache).unwrap().rule_count(), 1);
        let saved = std::fs::read(&cache).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        assert!(value[0].get("description").is_none());
        let mut invalid = std::fs::read(&source).unwrap();
        invalid.extend_from_slice(b" trailing invalid data");
        std::fs::write(&source, invalid).unwrap();
        assert!(compact(&source, Some(&cache)).is_err());
        assert_eq!(std::fs::read(&cache).unwrap(), saved);
        std::fs::write(&source, b"[]").unwrap();
        assert!(compact(&source, Some(&cache)).is_err());
        assert_eq!(std::fs::read(&cache).unwrap(), saved);
    }

    #[test]
    fn reads_public_nullable_skus_and_discards_launchers_and_other_platforms() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            serde_json::to_vec(&json!([
                {"id":"42", "name":"Verified Steam Game", "third_party_skus":[
                    {"distributor":"battlenet", "id":null}, {"distributor":"steam", "id":"123"}],
                    "executables":[{"name":"launcher", "os":host_os(), "is_launcher":true}]},
                {"id":"43", "name":"Game", "executables":[{"name":"native_game", "os":host_os()}]}
            ]))
            .unwrap(),
        )
        .unwrap();
        let catalog = Catalog::read(file.path()).unwrap();
        assert_eq!(
            &*catalog.steam_game("123").unwrap().name,
            "Verified Steam Game"
        );
        assert!(
            catalog
                .matches("/games/launcher", &[], cfg!(target_os = "windows"))
                .is_none()
        );
        assert!(
            catalog
                .matches("/games/native_game", &[], cfg!(target_os = "windows"))
                .is_some()
        );
        std::fs::write(file.path(), "[] trailing invalid data").unwrap();
        assert!(Catalog::read(file.path()).is_err());
    }

    #[test]
    fn matches_paths_on_component_boundaries_and_windows_case() {
        let games = serde_json::from_value(json!([{"id":"42","name":"Game","executables":[
            {"os":"win32","name":"Game/Game.EXE"},
            {"os":"linux","name":"game"}]}]))
        .unwrap();
        let catalog = Catalog::from_games(games);
        assert!(
            catalog
                .matches("C:\\Games\\GAME\\game.exe", &[], true)
                .is_some()
        );
        assert!(
            catalog
                .matches("C:/AnotherGame/game.exe", &[], true)
                .is_none()
        );
        assert!(catalog.matches("/opt/game", &[], false).is_some());
        assert!(catalog.matches("/opt/notgame", &[], false).is_none());
        assert!(catalog.match_path("/opt/GAME", &[], false).is_none());
    }

    #[test]
    fn macos_recognizes_the_app_bundle_instead_of_the_inner_executable() {
        let catalog = Catalog::from_games(
            serde_json::from_value(json!([{
                "id":"42", "name":"Mac Game", "executables":[{"os":"darwin", "name":"game.app"}]
            }]))
            .unwrap(),
        );
        assert_eq!(
            &*catalog
                .matches_macos("/Applications/Game.app/Contents/MacOS/Game", &[])
                .unwrap()
                .name,
            "Mac Game"
        );
        assert!(
            catalog
                .matches_macos("/Applications/OtherGame.app/Contents/MacOS/Game", &[])
                .is_none()
        );
    }

    #[test]
    fn windows_handles_verbatim_paths_and_rules_with_multiple_arguments() {
        assert_eq!(normalize(r"\\?\C:\Games\Fixture", true), "c:/games/fixture");
        assert_eq!(normalize(r"\\?\UNC\server\Games", true), "//server/games");
        let catalog = Catalog::from_games(serde_json::from_value(json!([{
            "name":"Runtime Game", "executables":[{"os":"win32", "name":"java.exe", "arguments":"--game fixture"}]
        }])).unwrap());
        assert!(catalog.matches("C:/Runtime/java.exe", &[], true).is_none());
        let args = ["java.exe", "--game", "fixture"].map(std::ffi::OsString::from);
        assert!(
            catalog
                .matches("C:/Runtime/java.exe", &args, true)
                .is_some()
        );
    }
}

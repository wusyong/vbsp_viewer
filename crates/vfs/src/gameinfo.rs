//! `gameinfo.txt` — the game's declaration of its search path stack.
//!
//! Each entry under `FileSystem/SearchPaths` is `<types> <location>`, where the
//! key is a `+`-joined list of *path types* and the value is a location:
//!
//! ```text
//! game+mod+custom_mod  tf/custom/*
//! game_lv              tf/tf2_lv.vpk
//! game+mod             tf/tf2_textures.vpk
//! game                 |all_source_engine_paths|hl2/hl2_textures.vpk
//! mod+mod_write+...    |gameinfo_path|.
//! gamebin              tf/bin
//! platform             |all_source_engine_paths|platform
//! ```
//!
//! Four details decide whether the resulting order matches the engine's:
//!
//! - **Locations are relative to the install root**, the directory holding
//!   `hl2.exe` — *not* to the directory holding `gameinfo.txt`. That is why the
//!   entries say `tf/...` even though the file itself lives in `tf/`.
//! - `|gameinfo_path|` is the directory holding `gameinfo.txt`;
//!   `|all_source_engine_paths|` is the install root.
//! - Only entries typed `game` or `mod` hold game content. `gamebin` is DLLs,
//!   `platform` is engine UI resources, and **`game_lv` is the low-violence
//!   override**, which must stay unmounted or a normal install would load
//!   censored assets where they exist.
//! - A `.vpk` reference names the *set*, so `tf2_textures.vpk` means
//!   `tf2_textures_dir.vpk`. The plain name is tried first for the rare
//!   single-file archive.
//! - `tf/custom/*` is a wildcard: every VPK and subdirectory inside, in
//!   alphabetical order, and the engine only scans it at boot.

use crate::keyvalues::{KeyValues, Result as KvResult, Value};
use std::path::{Path, PathBuf};

/// Path types that hold game content, as opposed to binaries or engine UI.
const CONTENT_TYPES: [&str; 2] = ["game", "mod"];

/// One `SearchPaths` entry, before resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPath {
    /// The `+`-joined type list, verbatim, e.g. `game+mod+custom_mod`.
    pub types: String,
    /// The location, verbatim, e.g. `|all_source_engine_paths|hl2/hl2_misc.vpk`.
    pub location: String,
}

impl SearchPath {
    /// Whether this entry holds game content the renderer should search.
    ///
    /// Type matching is exact per token: `game_lv` is not `game`.
    pub fn is_content(&self) -> bool {
        self.types
            .split('+')
            .any(|t| CONTENT_TYPES.contains(&t.trim().to_ascii_lowercase().as_str()))
    }
}

/// The parts of `gameinfo.txt` this crate needs.
#[derive(Clone, Debug, Default)]
pub struct GameInfo {
    pub game: String,
    pub steam_app_id: Option<u32>,
    /// Every `SearchPaths` entry in file order, unfiltered.
    pub search_paths: Vec<SearchPath>,
}

impl GameInfo {
    pub fn parse(text: &str) -> KvResult<GameInfo> {
        let kv = KeyValues::parse(text)?;
        // The root block is named "GameInfo", but a mod could name it anything,
        // so fall back to the first block rather than requiring the name.
        let root = kv
            .block("GameInfo")
            .or_else(|| {
                kv.entries()
                    .iter()
                    .find_map(|(_, v)| v.as_block())
            })
            .unwrap_or(&kv);

        let mut info = GameInfo {
            game: root.string("game").unwrap_or_default().to_string(),
            steam_app_id: root
                .path(["FileSystem"])
                .and_then(|fs| fs.string("SteamAppId"))
                .and_then(|s| s.trim().parse().ok()),
            search_paths: Vec::new(),
        };

        if let Some(paths) = root.path(["FileSystem", "SearchPaths"]) {
            for (types, value) in paths.entries() {
                if let Value::String(location) = value {
                    info.search_paths.push(SearchPath {
                        types: types.clone(),
                        location: location.clone(),
                    });
                }
            }
        }
        Ok(info)
    }

    /// Resolve the content search paths to absolute paths, in mount order.
    ///
    /// `game_dir` is the directory holding `gameinfo.txt`; the install root is
    /// its parent. Paths that do not exist are still returned — the caller
    /// records them, because a missing entry is normal (`tf2_lv.vpk`) and a
    /// silently dropped one is impossible to debug.
    pub fn resolve(&self, game_dir: &Path) -> Vec<PathBuf> {
        let root = game_dir.parent().unwrap_or(game_dir).to_path_buf();
        let mut out = Vec::new();

        for entry in self.search_paths.iter().filter(|e| e.is_content()) {
            let location = expand_tokens(&entry.location, game_dir, &root);

            if let Some(parent) = location.strip_suffix('*') {
                out.extend(expand_wildcard(&resolve_relative(parent, &root)));
                continue;
            }
            let path = resolve_relative(&location, &root);
            if let Some(vpk) = vpk_set_path(&path) {
                out.push(vpk);
            } else {
                out.push(path);
            }
        }
        out
    }
}

/// Replace `|gameinfo_path|` and `|all_source_engine_paths|`.
///
/// Both expand to absolute paths, so the result is absolute and
/// [`resolve_relative`] leaves it alone.
fn expand_tokens(location: &str, game_dir: &Path, root: &Path) -> String {
    let mut out = location.trim().to_string();
    for (token, value) in [
        ("|gameinfo_path|", game_dir),
        ("|all_source_engine_paths|", root),
    ] {
        if let Some(rest) = out.strip_prefix(token) {
            let rest = rest.trim_start_matches(['/', '\\']);
            let joined = value.join(rest);
            out = joined.to_string_lossy().into_owned();
        }
    }
    out
}

/// Anchor a relative location at the install root, and drop a trailing `.`.
fn resolve_relative(location: &str, root: &Path) -> PathBuf {
    let trimmed = location.trim().trim_end_matches(['/', '\\']);
    // `|gameinfo_path|.` expands to `<...>/tf/.`; normalising it makes the
    // duplicate-mount check work.
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    let trimmed = trimmed.trim_end_matches(['/', '\\']);
    let path = Path::new(trimmed);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// `tf2_textures.vpk` names an archive *set*; the file on disk is
/// `tf2_textures_dir.vpk`. Returns `None` when the location is not a VPK.
fn vpk_set_path(path: &Path) -> Option<PathBuf> {
    if !path.extension().and_then(|e| e.to_str())?.eq_ignore_ascii_case("vpk") {
        return None;
    }
    // A literal single-file archive wins if it exists; otherwise the `_dir`
    // member of the set.
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let stem = path.file_stem()?.to_string_lossy().to_string();
    if stem.ends_with("_dir") {
        return Some(path.to_path_buf());
    }
    Some(path.with_file_name(format!("{stem}_dir.vpk")))
}

/// Expand a `dir/*` search path: VPKs first, then subdirectories, each
/// alphabetically, mirroring the engine's boot-time scan.
///
/// A missing `custom/` directory is normal, so an unreadable path yields
/// nothing rather than an error.
fn expand_wildcard(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let (mut vpks, mut dirs): (Vec<PathBuf>, Vec<PathBuf>) = (Vec::new(), Vec::new());
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => dirs.push(path),
            Ok(t) if t.is_file() => {
                let is_vpk = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("vpk"));
                // Only the `_dir` member of a set is mountable; the numbered
                // archives are its payload, not separate search paths.
                let is_member = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| !s.ends_with("_dir") && has_numeric_suffix(s));
                if is_vpk && !is_member {
                    vpks.push(path);
                }
            }
            _ => {}
        }
    }
    vpks.sort();
    dirs.sort();
    vpks.extend(dirs);
    vpks
}

/// Whether a VPK stem ends in `_NNN`, marking it a payload archive.
fn has_numeric_suffix(stem: &str) -> bool {
    match stem.rsplit_once('_') {
        Some((_, suffix)) => {
            suffix.len() == 3 && suffix.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real TF2 `gameinfo.txt`, trimmed to the parts that matter. Kept
    /// verbatim — tabs, comments, and all — because the point is to parse what
    /// Valve actually ships.
    const TF2_GAMEINFO: &str = r#"
"GameInfo"
{
	game	"Team Fortress 2"
	type multiplayer_only
	FileSystem
	{
		SteamAppId				440

		//
		// Setup engine search paths.
		//
		SearchPaths
		{
			game+mod+custom_mod	tf/custom/*

			game_lv				tf/tf2_lv.vpk
			game+mod			tf/tf2_textures.vpk
			game+mod			tf/tf2_sound_vo_english.vpk
			game+mod			tf/tf2_sound_misc.vpk
			game+mod+vgui		tf/tf2_misc.vpk
			game				|all_source_engine_paths|hl2/hl2_textures.vpk
			game+vgui			|all_source_engine_paths|hl2/hl2_misc.vpk
			platform+vgui			|all_source_engine_paths|platform/platform_misc.vpk

			mod+mod_write+default_write_path		|gameinfo_path|.
			game+game_write		tf
			gamebin				tf/bin
			game				|all_source_engine_paths|hl2
			platform			|all_source_engine_paths|platform
			game+download	tf/download
		}
	}
}
"#;

    #[test]
    fn parses_the_shipped_gameinfo() {
        let info = GameInfo::parse(TF2_GAMEINFO).expect("parse");
        assert_eq!(info.game, "Team Fortress 2");
        assert_eq!(info.steam_app_id, Some(440));
        assert_eq!(info.search_paths.len(), 15, "{:#?}", info.search_paths);
        assert_eq!(info.search_paths[0].types, "game+mod+custom_mod");
        assert_eq!(info.search_paths[0].location, "tf/custom/*");
    }

    #[test]
    fn only_game_and_mod_paths_hold_content() {
        let info = GameInfo::parse(TF2_GAMEINFO).expect("parse");
        let kept: Vec<&str> = info
            .search_paths
            .iter()
            .filter(|p| p.is_content())
            .map(|p| p.location.as_str())
            .collect();

        // `game_lv` must not be mounted: it is the low-violence override, and
        // mounting it on a normal install swaps in censored assets.
        assert!(!kept.contains(&"tf/tf2_lv.vpk"), "low-violence VPK mounted");
        // Binaries and engine UI are not content either.
        assert!(!kept.contains(&"tf/bin"));
        assert!(!kept.iter().any(|l| l.ends_with("platform")));
        assert!(!kept.iter().any(|l| l.contains("platform_misc")));
        // ...and everything that is content survived, in order.
        assert_eq!(
            kept,
            [
                "tf/custom/*",
                "tf/tf2_textures.vpk",
                "tf/tf2_sound_vo_english.vpk",
                "tf/tf2_sound_misc.vpk",
                "tf/tf2_misc.vpk",
                "|all_source_engine_paths|hl2/hl2_textures.vpk",
                "|all_source_engine_paths|hl2/hl2_misc.vpk",
                "|gameinfo_path|.",
                "tf",
                "|all_source_engine_paths|hl2",
                "tf/download",
            ]
        );
    }

    #[test]
    fn resolution_anchors_relative_paths_at_the_install_root_not_the_game_dir() {
        // This is the detail that silently mounts nothing if you get it wrong:
        // `tf/tf2_textures.vpk` is relative to the *parent* of `tf/`.
        let info = GameInfo::parse(TF2_GAMEINFO).expect("parse");
        let game_dir = Path::new("/game/tf");
        let resolved = info.resolve(game_dir);

        let names: Vec<String> = resolved
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(
            names.contains(&"/game/tf/tf2_textures_dir.vpk".to_string()),
            "{names:#?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("/tf/tf/")),
            "anchored at the game dir instead of the root: {names:#?}"
        );
        assert!(names.contains(&"/game/hl2/hl2_textures_dir.vpk".to_string()));
        // `|gameinfo_path|.` and the bare `tf` both mean `/game/tf`.
        assert_eq!(
            names.iter().filter(|n| *n == "/game/tf").count(),
            2,
            "both spellings must resolve to the same directory: {names:#?}"
        );
        assert!(names.contains(&"/game/hl2".to_string()));
        assert!(names.last().is_some_and(|n| n.ends_with("tf/download")));
    }

    #[test]
    fn vpk_references_name_the_dir_member_of_the_set() {
        assert_eq!(
            vpk_set_path(Path::new("/g/tf/tf2_textures.vpk")),
            Some(PathBuf::from("/g/tf/tf2_textures_dir.vpk"))
        );
        // Already a `_dir` name: left alone.
        assert_eq!(
            vpk_set_path(Path::new("/g/tf/x_dir.vpk")),
            Some(PathBuf::from("/g/tf/x_dir.vpk"))
        );
        // Not a VPK at all.
        assert_eq!(vpk_set_path(Path::new("/g/tf")), None);
    }

    #[test]
    fn payload_archives_are_not_mounted_as_search_paths() {
        // `custom/mymod_000.vpk` is the payload of `custom/mymod_dir.vpk`;
        // mounting it as its own archive would fail to parse a VPK header.
        assert!(has_numeric_suffix("mymod_000"));
        assert!(has_numeric_suffix("tf2_misc_027"));
        assert!(!has_numeric_suffix("mymod_dir"));
        assert!(!has_numeric_suffix("mymod"));
        assert!(!has_numeric_suffix("pack_01"), "only three digits count");
    }

    #[test]
    fn a_mod_root_block_need_not_be_called_gameinfo() {
        let info = GameInfo::parse(
            r#""SomeMod" { FileSystem { SearchPaths { game somemod } } }"#,
        )
        .expect("parse");
        assert_eq!(info.search_paths.len(), 1);
    }
}

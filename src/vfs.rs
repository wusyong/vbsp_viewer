//! The search path.
//!
//! `tf-asset-loader` does the heavy lifting — locating the install via
//! `steamlocate`, mounting `tf/`, `hl2/`, `tf/download/` and every `*_dir.vpk`.
//! This wrapper exists for two things it does not do.
//!
//! **Pakfile precedence.** Source resolves the BSP's own embedded pakfile
//! *first*, which is how custom maps override stock content. `Loader::add_source`
//! appends, so mounting the pakfile through it puts it dead last — the opposite.
//! `vbspview` mounts it exactly that way, so it has the precedence backwards.
//! Here the pakfile is a separate field consulted before the loader, which also
//! sidesteps the version split: `tf-asset-loader`'s `bsp` feature pins vbsp
//! 0.8.2, and linking that beside our 0.9 would give two incompatible copies of
//! the crate.
//!
//! **Separator normalisation.** `tf-asset-loader` retries lookups lowercased,
//! but its `clean_path` only resolves `../` — backslashes are left alone. VMTs
//! reference textures with either separator, so normalise before asking.

use tf_asset_loader::Loader;
use vbsp::Packfile;

pub struct Vfs {
    /// Searched first. `None` for a map with no embedded content.
    pack: Option<Packfile>,
    /// `None` if the TF2 install could not be found, which is survivable: a
    /// custom map may carry everything it needs in its pakfile.
    loader: Option<Loader>,
}

impl Vfs {
    pub fn new(pack: Option<Packfile>) -> (Self, Option<String>) {
        let (loader, error) = match Loader::new() {
            Ok(loader) => (Some(loader), None),
            Err(e) => (None, Some(format!("no TF2 install found: {e}"))),
        };
        (Vfs { pack, loader }, error)
    }

    /// Source paths are case-insensitive and mixed-separator. Everything below
    /// goes through here first.
    fn normalise(path: &str) -> String {
        path.replace('\\', "/").to_ascii_lowercase()
    }

    pub fn load(&self, path: &str) -> Option<Vec<u8>> {
        let path = Self::normalise(path);
        if let Some(pack) = &self.pack
            && let Ok(Some(data)) = pack.get(&path)
        {
            return Some(data);
        }
        self.loader.as_ref()?.load(&path).ok().flatten()
    }

}

// Phase 2 will want `exists` and `find_in_paths(name, dirs)` here — an MDL names
// its texture and the directories to search for it separately. Left out until
// there is a caller: `Loader` has its own versions, but they would not consult
// the pakfile, so ours will have to shadow them the way `load` does.

# Vendored crates

| crate | upstream | revision |
|---|---|---|
| `vbsp` | https://github.com/eira-fransham/vbsp | `cd1711720be854654be7708fec44b2b11fc08424` |
| `qbsp` | https://github.com/eira-fransham/qbsp.git | `ca4e05b4ece63cba883615144e31286c01222672` |
| `vmt-parser` | https://codeberg.org/icewind/vmt-parser | `bf47f537940c26d9571fc2f8761d1e9edbbc7acc` |

## Local changes

Manifests only — no `.rs` file has been touched, so a diff against upstream
stays readable.

* **`vbsp`, `vbsp/common`, `qbsp`** — `glam` 0.30 → **0.32**, and qbsp's
  optional `bevy_reflect` 0.18 → **0.19**, to match bevy 0.19. This is the
  point of vendoring: on 0.30 vbsp's `Vec3` was a *different type* from bevy's
  and everything crossing the boundary had to be rebuilt componentwise. Both
  crates compiled against 0.32 unchanged, and ctf_2fort renders identically
  (232 batches, 63,783 tris, 14,879 faces, 49 normal mismatches — same as
  before the bump).
* **`vbsp`** — dropped `[workspace] members = ["common"]` and `[profile.dev]`
  (profiles are only honoured in a workspace root), and repointed `qbsp` from
  its git URL to `path = "../qbsp"`. That last one matters: as git deps, vbsp
  declared `qbsp` with no `rev` while we declared it *with* one, and Cargo
  treats those as different sources — two `qbsp` packages, and
  `compute_lightmap_atlas_rgb32f` rejecting our packer with a type error that
  reads like nonsense.
* **`qbsp`** — upstream is its own workspace root; dropped `[workspace]` and
  `[workspace.package]` and inlined the six inherited fields here and in
  `qbsp_macros/` and `tools/write-lightmaps/`.

`vmt-parser` is unmodified. Its GitHub mirror is stale at 0.2.0 (with the wrong
repo URL in its own manifest) — Codeberg is the real home, and the revision
above is byte-identical to the crates.io 0.2.1 tarball's `src/`.

## The .bsp fixtures

Upstream vbsp ships `koth_bagel_rc2a.bsp` (22 MB) and `test.bsp`, which its
bench `include_bytes!`s and one of its unit tests reads. They are gitignored on
the same footing as `assets/maps/` — it is map data, and redistributing map
data is not ours to do. Nothing in the viewer's build needs them; fetch them if
you want `cargo test -p vbsp@0.9.1` and `cargo bench` to pass:

```sh
for f in koth_bagel_rc2a.bsp test.bsp; do
  curl -sSL -o "crates/vbsp/$f" \
    "https://raw.githubusercontent.com/eira-fransham/vbsp/cd1711720be854654be7708fec44b2b11fc08424/$f"
done
```

## Updating one

Re-clone at the new revision, re-apply the changes above, update the table.
Keep the manifests-only rule: when a real source change is finally needed,
commit it separately with a comment saying why, so it survives the next update
instead of being silently reverted.

# Vendored crates

| crate | upstream | revision |
|---|---|---|
| `vbsp` | https://github.com/eira-fransham/vbsp | `cd1711720be854654be7708fec44b2b11fc08424` |
| `qbsp` | https://github.com/eira-fransham/qbsp.git | `ca4e05b4ece63cba883615144e31286c01222672` |
| `vmt-parser` | https://codeberg.org/icewind/vmt-parser | `bf47f537940c26d9571fc2f8761d1e9edbbc7acc` |
| `vtf` | https://github.com/roman901/vtf-rs | `4f8a4fec41b4d2b3c6b51933370c8cebfb4bba2f` (0.4.1) |

## Local changes

Manifest-only, **except `vtf`** — see the last entry. Everywhere else no `.rs`
file has been touched, so a diff against upstream stays readable.

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
* **`vtf`** — **the one source change.** Added `get_mip`, `mip_count` and
  `mip_size` to `VTFImage` in `src/image.rs`, inside a `LOCAL ADDITION` comment
  fence. Upstream exposes mip 0 and nothing else: `get_offset` takes a mip index
  but lives in a private `mod utils`, and `VTFImage`'s `bytes`/`offset` fields
  are private too, so the chain is unreachable from outside the crate. 0.4.1
  does not help — its only changes are on the writer side. Without this,
  everything renders unmipped and shimmers at distance (milestone 1.5b).

  Worth knowing when reading that code: **VTF stores mips smallest-first**, so
  mip 0 sits last in the file and `get_offset` sums the sizes of every *smaller*
  mip to find it. wgpu wants the opposite order, so the caller walks
  `0..mip_count` and the reversal happens on the way out.

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

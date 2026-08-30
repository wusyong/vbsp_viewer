# tf2-bevy-viewer

Phase 1 of [the roadmap](../tf2-bevy-phase1-roadmap.md). Two ways to look at the
same map, toggled at runtime with `G`:

- **Reference** — `ctf_2fort.glb` from `vbsp-to-gltf`. Known-good, textured,
  static. This is what milestone 1.0 established.
- **BSP** — our own `vbsp` → `Mesh` path, textured from the VPKs (1.1 + 1.2).
  No lightmaps, no props, no sky yet.

Keeping both in one scene is the point. A geometry bug in the BSP path shows up
as a difference from the reference, which beats deciding in the abstract whether
a wall is in the right place.

## Running

```sh
cargo run                              # interactive, BSP view
cargo run -- --shot                    # capture shot.png after 8 s, then quit
TF2_BSP=/path/to/map.bsp cargo run     # a different map
TF2_CAM=0,16,45 cargo run              # start position, metres, Bevy axes
TF2_CULL=1 TF2_PLAIN=1 cargo run       # initial toggle state, for --shot
```

`G` cycles BSP / reference / both · `C` cycles culling per-material / forced on /
forced off · `T` cycles textured / per-texture colour / plain · RMB look ·
WASD/QE move · Shift sprint · `[` `]` speed.

The BSP is read from the TF2 install directly, so there is no asset step for the
BSP view. The reference view needs the glb:

```sh
cargo install --git https://github.com/icewind1991/vbsp-to-gltf
vbsp-to-gltf "$TF2/tf/maps/ctf_2fort.bsp" assets/maps/ctf_2fort.glb
```

`assets/maps/` is gitignored — Valve's map data isn't redistributable, and the
glb is ~170 MB.

## Milestone 1.0 — glTF smoke test

Established that the assets, the parser and the scale factor all work.

- **Coordinates need no fixing.** `vbsp-to-gltf` maps Source `(x, y, z)` to
  `(y, z, x)`, a cyclic permutation: Z-up becomes Y-up and handedness is
  preserved, so winding survives. The roadmap's `(x, z, -y)` mirrors instead and
  would need every index list reversed.
- **Scale is applied on our side.** The exporter writes raw Hammer units (its own
  `UNIT_SCALE` is `#[allow(dead_code)]`). 1 unit = 1 inch = 0.0254 m puts 2fort
  at ~120 m end to end. Both of icewind's tools use 1.905 cm instead, which makes
  the player 1.37 m tall.
- **The reference renders almost black out of the box.** It writes no PBR
  factors, so glTF's defaults apply — `metallic: 1.0`, meaning no diffuse
  response. `fix_reference_materials` corrects this on load.

## Milestone 1.1 — geometry from the BSP

`232 batches · 63,783 triangles · 14,879 faces · 6 ms` on ctf_2fort.

Structure follows `vbspview`'s `bsp.rs` (group faces by texture, one mesh per
group) rather than `vbsp-to-gltf`'s, which emits one primitive per face — 14,574
of them. Three deliberate departures from both:

**Brush entities are found by their `*N` model reference**, not by matching a
list of classnames. Both of icewind's tools match exactly four
(`func_brush`, `func_illusionary`, `func_wall`, `func_wall_toggle`), which on
this map reaches 64 of 148 models — the glb has exactly 64 nodes named `bsp`.
Keying on the model reference reaches all 148, picking up `func_door` ×14,
`func_regenerate` ×8, `func_respawnroomvisualizer` ×8 and `func_rotating` ×3
among others: the spawn doors and resupply cabinets. Worth 305 extra visible
faces.

**Normals come from the face's plane**, not from recomputing them off the
triangles as vbspview does. Exact, and free.

**`dface_t.side` must be ignored.** However much the field name suggests
otherwise, it records which side of the plane the face sits on for the BSP
tree's benefit — the surfedge walk is already wound to the face's true facing.
Measured here: negating the normal when `side` is set disagrees with the winding
on 5,508 of 14,879 faces; leaving it alone disagrees on 49.

The build carries a standing cross-check of the plane normal against the winding
of each face's first triangle, reported in the HUD. It is what caught the `side`
mistake. **Known residual: 45 faces are genuinely backwards** (0.3%), not
numerical noise — unexplained, and not chased down at this milestone.

Displacements appear to come free: `vbsp`'s `vertex_positions()` already returns
displaced, triangulated vertices, and 232 faces take that path. Worth confirming
against the reference before calling 1.3 done.

## Milestone 1.2 — textures

`226 materials, 0 failed · 225 BC + 1 RGBA = 88 MB · 120 ms`

Three modules: [vfs.rs](src/vfs.rs) (search path), [vmt.rs](src/vmt.rs)
(materials), [vtf.rs](src/vtf.rs) (textures).

**DXT blocks go to the GPU untouched.** DXT1/3/5 are bit-identical to BC1/2/3, so
`get_frame(0)` is uploaded as-is wherever the adapter reports
`TEXTURE_COMPRESSION_BC`, with a CPU decode as fallback. This is not a
micro-optimisation: 88 MB against roughly 900 MB if everything were decoded to
RGBA8. Both reference tools decode, and `vbsp-to-gltf` then re-encodes to PNG,
which is how 22 MB of BSP becomes a 172 MB glb.

**The pakfile is searched first**, ahead of the install, which is how a custom map
overrides stock content. `tf-asset-loader`'s `add_source` *appends*, so mounting
it that way puts it last — vbspview has this backwards. Keeping it as a separate
field also sidesteps a version split: `tf-asset-loader`'s `bsp` feature pins vbsp
0.8.2, which cannot coexist with our 0.9.

**Two things the search path needed that `tf-asset-loader` does not do.** Its
`clean_path` only resolves `../`; the lowercase retry in `load` handles case, but
backslashes are left alone, and VMTs use both separators. Normalising both before
the lookup is the whole fix.

**`ImageAddressMode::Repeat` is mandatory.** Source computes UVs as a dot product
of world position against texture size, so a long wall runs to `u = 40`. Bevy
defaults to `ClampToEdge`, which turns every surface into one smeared edge pixel.

Materials are **unlit** on purpose. Without lightmaps the only light available is
a single directional, which reads worse than flat albedo and hides texture
problems in shadow. Flat is the honest intermediate state; 1.4 is what makes it
look like TF2.

`$bumpmap` is deliberately *not* collected. A normal map needs per-vertex
tangents and the BSP has none, so storing it would mean decoding a texture that
is never read — exactly what `vbsp-to-gltf` does. It goes in when tangent
generation does.

**Known gap: no mipmaps.** Only mip 0 is uploaded, so distant surfaces will
shimmer in motion. VTFs carry the full chain, and `get_offset` takes a mip index,
so this is a contained fix rather than a design problem.

## Notes for later milestones

**1.4 (lightmaps) is blocked on `vbsp` and has no reference implementation.**
`Bsp` has no `lighting` field; `LumpType::Lighting` and `LightingHdr` exist but
are never read; `mod bspfile` is private, so `BspFile`, `LumpType` and
`get_lump` are unreachable; and `Bsp::header` is only `{ version }`, with no
lump table. Face-side data is all public (`light_offset`,
`light_map_texture_min`/`_size`, `styles`) — everything except the bytes.
Neither vbspview nor vbsp-to-gltf touches lightmaps at all.

The workaround is a side-parser: read the 8 + 64×16 lump table off the raw bytes
and LZMA-decompress lump 8. TF2 compresses lumps — the models and entity lumps of
ctf_2fort both start with `LZMA` magic, and the lump header's `fourCC` field
holds the uncompressed size rather than a code. Worth an upstream PR.

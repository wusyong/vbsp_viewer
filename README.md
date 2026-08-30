# tf2-bevy-viewer

Phase 1 of [the roadmap](../tf2-bevy-phase1-roadmap.md). Two ways to look at the
same map, toggled at runtime with `G`:

- **Reference** — `ctf_2fort.glb` from `vbsp-to-gltf`. Known-good, textured,
  static. This is what milestone 1.0 established.
- **BSP** — our own `vbsp` → `Mesh` path: textured from the VPKs and lit by the
  map's baked lightmaps (1.1–1.4). No props and no sky yet.

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
TF2_LIGHTMAP_EXPOSURE=8 cargo run      # lightmap brightness (default 20)
```

`G` cycles BSP / reference / both · `C` cycles culling per-material / forced on /
forced off · `T` cycles textured / per-texture colour / plain · `-` `=` lightmap
exposure · RMB look · WASD/QE move · Shift sprint · `[` `]` speed.

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

Materials were **unlit** at this milestone, on purpose: with no lightmaps the
only light available was a single directional, which reads worse than flat albedo
and hides texture problems in shadow. 1.4 turned that off — see below, and note
that Bevy silently ignores a `Lightmap` on an unlit material.

`$bumpmap` is deliberately *not* collected. A normal map needs per-vertex
tangents and the BSP has none, so storing it would mean decoding a texture that
is never read — exactly what `vbsp-to-gltf` does. It goes in when tangent
generation does.

**Known gap: no mipmaps.** Only mip 0 is uploaded, so distant surfaces will
shimmer in motion. VTFs carry the full chain, and `get_offset` takes a mip index,
so this is a contained fix rather than a design problem.

## Milestone 1.3 — displacements

Free, as suspected. 232 faces take the displacement path through
`vertex_positions()`, and after the fork migration they are indexed with
smoothed vertex normals (visible as a smoothly shaded cliff face in the `plain`
view) and they lightmap correctly.

One open question, deferred to 1.5: base-texture UVs on displacements are
projected from the *displaced* position, where Source interpolates the base
face's corner UVs across the grid. On steep displacements that shows as slight
stretching.

## Milestone 1.4 — lightmaps

`2044x1393 atlas · 1 style · 14,662 patches, 1,129 without · 43 MB · 95 ms`

[lightmap.rs](src/bsp/lightmap.rs) is the shortest module in the project and the
longest task in the roadmap, because the vbsp fork does the hard part:
`compute_lightmap_atlas_rgb32f` decodes every face's RGBE patch and packs them
into one atlas, returning a per-face pixel rect.

**The atlas remap is baked into `UV_1`, not left to Bevy.** `Lightmap.uv_rect`
exists for exactly this, but it is per-*entity* and we batch hundreds of faces
per entity, so one rect cannot describe them. Each face's `lightmap_uvs()` (0..1
across its own patch) is scaled into its atlas slot at mesh-build time and
`uv_rect` stays the full 0..1.

**The atlas is `Rgba32Float`.** Source stores RGBE and the shared exponent
routinely pushes values past 1.0, so an 8-bit target would clip every bright
surface. That costs 43 MB, which is worth it; `Rgba16Float` would halve it if
that ever matters.

**Materials had to stop being `unlit`.** They were unlit through 1.2 because
there was nothing to light them with. The failure mode if you forget is "nothing
changed on screen" rather than an error — Bevy silently ignores the `Lightmap`
component on an unlit material.

**Exposure is eyeballed and has no principled value.** Source bakes against its
own light units; Bevy's PBR expects something else. `20.0` was found by sweeping
— 1 is dim but legible, 200 washes out, 4000 is pure white. `-`/`=` adjust it
live and `TF2_LIGHTMAP_EXPOSURE` overrides it, because a hand-tuned constant
deserves a knob.

**Known gaps.** Only style 0 is used: `LightmapStyle` keys one atlas per
switchable-light state, and compositing them at runtime is deferred (neither
existing Source viewer does it either). 1,129 faces have no patch and sample the
atlas's neutral texel, rendering at full albedo. Indoor lighting reads better
than outdoor at a single global exposure, which is inherent to one constant over
HDR data.

## The vbsp fork

We depend on [`eira-fransham/vbsp`](https://github.com/eira-fransham/vbsp),
pinned to a rev, rather than crates.io `vbsp` 0.9.1. It is MIT, forked from
exactly that tag.

**Upstream is dormant** — its last code change was 2025-04-19, with only workflow
commits since. So this is not "fork risk versus a maintained upstream"; upstream
is stale either way, and the fork is the living line.

It supplies what upstream withholds, all of which is 1.4's input:
`Bsp::lightmap_data`, `Face::lightmap_uvs()`,
`Bsp::compute_lightmap_atlas_rgb32f()`, HDR lighting, leaf ambient lighting (for
props in Phase 2) and displacement lightmap sample positions. Upstream exposes
every *index* into the lightmap and none of the bytes: `mod bspfile` is private,
so `BspFile`/`LumpType`/`get_lump` are unreachable, and `Bsp::header` is only
`{ version }` with no lump table.

**It also unified triangulation**, which is why `push_face` is one path rather
than two. Upstream's `vertex_positions()` returns a pre-triangulated soup; the
fork's returns the face's own vertices (polygon corners, or the displaced grid)
with `triangulate_indices()` giving indices into them. Migrating dropped vertex
count from 135,597 to 85,597 — a 37% saving, all of it displacements that were
previously duplicated per triangle.

**The one real wart**: it tracks glam 0.30 while Bevy 0.19 is on 0.32. Both get
linked, and its `Vec3`/`Vec2` are distinct types from Bevy's, so everything
crossing the boundary converts componentwise. `SourceVec3` is aliased in
`geometry.rs` so this reads as a deliberate boundary rather than a baffling
"expected `Vec3`, found `Vec3`".

### What the migration did and didn't change

Identical: 232 batches, 63,783 triangles, 14,879 faces, 232 displaced, and the
cross-check still at 49 mismatches / 45 strong — which is the evidence the
triangulation swap was clean.

Changed: vertices 135,597 → 85,597, and `side set on` 5,795 → 5,830. The latter
is not a regression — the counter used to sit after an early `return` in the
displacement branch, so it never counted displaced faces. 35 of the 232 have
`side` set.

## Notes for later milestones

`bevy_bsp`/`bevy_vpk` ([eira-fransham](https://github.com/eira-fransham/bevy_vbsp),
and [kristoff3r](https://github.com/kristoff3r/bevy_vbsp), which is a fork of it
and further along) are the reason the vbsp fork and qbsp's lightmap packer exist.
**Neither repo carries a license** — no LICENSE file, no `license` field on
either crate — so they are all-rights-reserved and cannot be copied from. They
remain useful as a design reference for the Bevy side of 1.4: the
`bevy::pbr::Lightmap` component, per-`LightmapStyle` atlases, and ASTC
compression of the atlas.

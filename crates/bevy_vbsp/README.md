# bevy_vbsp

Phase 1 of [the roadmap](../../tf2-bevy-phase1-roadmap.md). Two ways to look at the
same map, toggled at runtime with `G`:

- **Reference** — `ctf_2fort.glb` from `vbsp-to-gltf`. Known-good, textured,
  static. This is what milestone 1.0 established.
- **BSP** — our own `vbsp` → `Mesh` path: textured and mipmapped from the VPKs,
  lit by the map's baked lightmaps, with the 2D sky as a cubemap and the 3D
  skybox placed where it belongs (1.1–1.5). No props yet — those are Phase 2.

Keeping both in one scene is the point. A geometry bug in the BSP path shows up
as a difference from the reference, which beats deciding in the abstract whether
a wall is in the right place.

## Running

```sh
cargo run                              # interactive, BSP view
cargo run -- --shot                    # capture shot.png after 8 s, then quit
cargo run -- --probe                   # load, render one frame, quit (~15 s)
TF2_BSP=/path/to/map.bsp cargo run     # a different map
TF2_CAM=0,16,45 cargo run              # start position, metres, Bevy axes
TF2_CULL=1 TF2_PLAIN=1 cargo run       # initial toggle state, for --shot
TF2_LIGHTMAP_EXPOSURE=8 cargo run      # lightmap brightness (default 20)
TF2_NO_SKY3D=1 cargo run               # start with the 3D skybox hidden
```

`G` cycles BSP / reference / both · `C` cycles culling per-material / forced on /
forced off · `T` cycles textured / per-texture colour / plain · `K` toggles the
3D skybox · `-` `=` lightmap exposure · RMB look · WASD/QE move · Shift sprint ·
`[` `]` speed.

`--probe` exists to sweep the map library: it loads, renders one frame so
anything the GPU would reject is actually submitted, and quits. That is how the
`Rgb888Bluescreen` and `cp_cloak` failures below were found, rather than by
assuming ctf_2fort generalises.

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

`236 batches · 63,564 triangles · 14,787 faces · 7 ms` on ctf_2fort. (The 1.1
figures were 232 / 63,783 / 14,879; 1.5a dropped 92 tool faces and 1.5d split
five shared materials into separate skybox batches.)

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
on 5,788 of 14,787 faces; leaving it alone disagrees on **zero**.

The build carries a standing cross-check of the plane normal against the winding
of every face, reported in the HUD. It is what caught the `side` mistake, and it
matters beyond diagnostics: backfaces are culled unless the VMT says `$nocull`,
so a backwards face is not mis-shaded but *invisible* — a hole you see through.

> This section used to read `5,508 / 49`, with a standing note that 45 of the 49
> were "genuinely backwards, unexplained". **All 45 were the check's own noise**
> — see milestone 1.6a below. The numbers above are the re-measured ones.

## Milestone 1.2 — textures

`229 materials, 0 failed · 228 BC + 1 RGBA = 120 MB · 114 ms` (1.2 measured 226 /
88 MB / 120 ms; mipmaps added the volume, 1.5d the extra batches)

Three modules: [vfs.rs](src/vfs.rs) (search path), [vmt.rs](src/vmt.rs)
(materials), [vtf.rs](src/vtf.rs) (textures).

**DXT blocks go to the GPU untouched.** DXT1/3/5 are bit-identical to BC1/2/3, so
`get_frame(0)` is uploaded as-is wherever the adapter reports
`TEXTURE_COMPRESSION_BC`, with a CPU decode as fallback. This is not a
micro-optimisation: 120 MB with full mip chains, against roughly 1.2 GB if the
same were decoded to RGBA8. Both reference tools decode, and `vbsp-to-gltf` then
re-encodes to PNG, which is how 22 MB of BSP becomes a 172 MB glb.

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

~~Known gap: no mipmaps.~~ Fixed in 1.5b — and it was not the contained fix this
predicted. See below.

## Milestone 1.3 — displacements

Free, as suspected. 232 faces take the displacement path through
`vertex_positions()`, and after the fork migration they are indexed with
smoothed vertex normals (visible as a smoothly shaded cliff face in the `plain`
view) and they lightmap correctly.

~~One open question, deferred to 1.5:~~ base-texture UVs were projected from the
*displaced* position where Source interpolates the base face's corner UVs across
the grid. Fixed in 1.5e, and measured before being fixed.

## Milestone 1.4 — lightmaps

`2044x1393 atlas · 1 style · 14,662 patches, 1,129 without · 43 MB · 93 ms`

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

~~Known gap: only style 0 is used.~~ Fixed in 1.5e. 1,129 faces still have no
patch and sample the atlas's neutral texel, rendering at full albedo; and indoor
lighting reads better than outdoor at a single global exposure, which is inherent
to one constant over HDR data.

## Milestone 1.5 — tools, mips, sky

**1.5a — tool textures.** `is_visible()` rejects the flagged ones (NODRAW, SKIP,
HINT, TRIGGER, SKY, SKY2D) — 912 faces here. But the flags live on the *texture*
and a few tool materials ship without them: `tools/toolsblack` was drawing 92
faces of opaque black. Every tool material lives under `tools/`, so a name check
closes it. The HUD splits the two so it stays obvious which is a format guarantee
and which is a convention we chose to trust.

**1.5b — mipmaps.** `120 MB, ×1.34 of mip 0 alone` — textbook for a full chain,
which is how you know it uploaded. This cost a fourth vendored crate: `vtf` hides
`get_offset` behind a private `mod utils`, so mip 1+ is unreachable from outside,
and 0.4.1 does not change that. See [VENDORING.md](../VENDORING.md). Note **VTF
stores mips smallest-first**, so mip 0 is last in the file and the chain has to be
reversed on the way to wgpu. `mipmap_filter: Linear` is required too — the
`linear()` preset leaves it at `Nearest`, which pops visibly.

**1.5c — the 2D sky.** `sky_tf2_04 · 512×512 cube from 3 textures · 6.3 MB · 12 ms`

Three things the documented "six textures named `<skyname>{rt,lf,bk,ft,up,dn}`"
gets wrong on this map, each a different failure:

- **There are no side VTFs.** Every face is a *VMT*, and all four sides resolve to
  one shared `sky_tf2_04side`. Going through the VMT is the only thing that works.
- **The faces differ in size** — sides 512×256, `up` 512×512, `dn` **one pixel**.
  wgpu needs all six cube layers identical, so everything resamples to one square
  size, which also rules out the BC pass-through used everywhere else.
- **`$basetexturetransform` is load-bearing.** The sides carry `scale 1 2`; with a
  clamped lookup that puts the gradient in the upper half and holds the horizon
  colour below. Ignore it and the gradient tiles twice.

Two Bevy notes: `Skybox::brightness` is **not** a 0..1 multiplier (the shader does
`colour * brightness` with camera exposure folded in, so `1.0` renders black — it
is set to the same `lux::OVERCAST_DAY` as the scene light), and six layers alone
give an array texture, so `TextureViewDimension::Cube` is what makes it a cubemap.

*Unverified:* face **rotation**. The axis mapping follows from the world
permutation, but Source also rotates some faces in-plane and ctf_2fort's gradient
sky cannot test it — a wrong rotation looks exactly like a right one.

**1.5d — the 3D skybox.** `127 faces · 4,708 tris · origin (-4544, 256, 48) · scale 16`

Distant scenery built at 1/16 size off to one side, which a loader walking model 0
picks up as ordinary brush geometry. Three plausible ways to identify it that do
not work — bounds (it has its own `toolsskybox` shell, so they overlap), texture
name (a convention, and five of its seven materials are *also* used in the
playable map), and BSP areas (2fort has no areaportals; all 7,971 leaves are
area 0).

What works is visibility: `sky_camera` → `leaf_at` → PVS, which is **exactly one
cluster of 2,489**, the signature of a sealed room. Union those 48 leaves' bounds
for the region, then classify faces geometrically — because **displacements are
absent from `leaf_faces`**, so attributing through the lump alone misses 4,544 of
the 4,708 triangles. Apply the brush model's origin first, or the skybox's own
cloud brushes test at the world origin.

Rendering is a single `Transform` on a separate root: scale `s`, translation
`-to_bevy(origin) * s`. That works as one transform because `to_bevy` is a
permutation plus a uniform scale, so the mapping commutes with it. **The parallax
is a lie** — Source uses a second camera at `sky_origin + (player)/scale`; this is
right standing still and slides as you move. `K` toggles it.

**1.5e — displacement UVs and lightmap styles.** Both smaller than billed, and
both measured before being built.

Displacement UVs project from the *undisplaced* base grid — Source interpolates
the base face's corner UVs, and since `TextureInfo::uv` is affine that is the same
thing. Mean error was 0.024 of a tile, but 7.2% of vertices exceeded 0.1 and the
worst was **0.77 of a tile on chicken wire**.

Styles composite **per face, never per pixel**. A style atlas is created filled
with `default_color`, which is white here so unlit faces render at full albedo —
so summing the atlases adds white everywhere a style does not reach. Walking faces
and reading only the styles each one declares never samples the fill. Worth
knowing this does *nothing* on ctf_2fort: 0 of 14,787 faces reference a switchable
light. It earns itself on koth_harvest_event, 4,200 of 8,831.

## Milestone 1.6a — the cross-check was wrong, not the map

The standing `45 faces genuinely backwards` note from 1.1 turned out to be an
artifact of the check itself.

Two flaws. It **returned early for displacements**, so 232 faces per map were
never checked at all. And it read only the **first emitted triangle** — but a face
is a polygon fanned from vertex 0, and one triangle at a concave or
near-collinear corner can wind against the face as a whole while the surface is
perfectly correct.

Summing the cross product over every triangle (area-weighted — Newell's method,
so slivers count for as little as they deserve) takes it to **0 of 14,787**, with
displacements included and also clean.

The instrument was then verified by breaking it deliberately: negating the plane
normal makes the fixed check flag 14,776 of 14,787. That is what makes a reported
zero mean "clean" rather than "not looking".

Worth keeping as a lesson — **a check reporting a small residual is not thereby
trustworthy.** 45 of 14,879 reads like the honest remainder of a good measurement.
It was a systematic flaw that happened to be small, and it sat in the plan as a
known defect for the whole of Phase 1.

## The vbsp fork

We build [`eira-fransham/vbsp`](https://github.com/eira-fransham/vbsp) rather
than crates.io `vbsp` 0.9.1 — MIT, forked from exactly that tag. It started as a
pinned git dependency and is now **vendored** at `crates/vbsp`, along with
`qbsp`, `vmt-parser` and `vtf`; see [VENDORING.md](../VENDORING.md) for why and
for the two local source changes.

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

**Its one real wart is now fixed.** It tracked glam 0.30 while Bevy 0.19 is on
0.32, so both got linked and its `Vec3`/`Vec2` were distinct types from Bevy's —
everything crossing the boundary had to be rebuilt componentwise. Vendoring made
that a version number rather than a fact of life: `crates/vbsp` and `crates/qbsp`
are on glam 0.32, they compiled against it unchanged, and the map renders
identically. There is now one glam in the workspace, `glam` is not a direct
dependency of this crate at all, and `Vec3` is just `Vec3`.

The cost is that the *coordinate* boundary lost its type-level guard. Z-up
Hammer units and Y-up metres are the same type now, so `to_bevy` is a
convention, not a checked conversion — see its doc comment.

### What the migration did and didn't change

Identical: 232 batches, 63,783 triangles, 14,879 faces, 232 displaced, and the
cross-check unchanged at 49 mismatches / 45 strong — which is what showed the
triangulation swap was clean. (Those were the naive check's figures; it now reads
0, but "unchanged across the swap" is the claim that mattered here.)

Changed: vertices 135,597 → 85,597, and `side set on` 5,795 → 5,830. The latter
is not a regression — the counter used to sit after an early `return` in the
displacement branch, so it never counted displaced faces. 35 of the 232 have
`side` set.

## Known gaps

Found by sweeping the map library with `--probe`, not by the plan. 91 of the
first 92 maps load; the details are in the roadmap's 1.6 table.

- **`cp_cloak` will not open.** `vbsp` rejects an empty displacement lump
  (`lump size 0 is not a multiple of the element size 0`) and the map simply has
  no displacements. Same BSP version 20 as every map that works. **Won't fix** —
  one map out of 233.
- **Five maps load with no sky**, all `Rgb888Bluescreen`, which `vtf` cannot
  decode. It is RGB888 plus a keyed-transparent blue, so it is the existing
  RGB888 path with an alpha test.
- **Nine maps have a few unresolved materials**, 1–2 each and `cp_carrier` at 14.
  They render magenta rather than failing; cause not investigated.
- **Sky face rotation is unverified** — see 1.5c.
- **1,129 faces have no lightmap patch** and render at full albedo.

## Notes for later milestones

`bevy_bsp`/`bevy_vpk` ([eira-fransham](https://github.com/eira-fransham/bevy_vbsp),
and [kristoff3r](https://github.com/kristoff3r/bevy_vbsp), which is a fork of it
and further along) are the reason the vbsp fork and qbsp's lightmap packer exist.
**Neither repo carries a license** — no LICENSE file, no `license` field on
either crate — so they are all-rights-reserved and cannot be copied from. They
were a useful design reference for 1.4's Bevy side: the `bevy::pbr::Lightmap`
component, per-`LightmapStyle` atlases, and ASTC compression of the atlas, which
is still the obvious lever if the 43 MB atlas ever matters.

The `AssetLoader` question is still open. Loading is synchronous in a startup
system and after five milestones that still looks right — the whole map is ~7 ms
of geometry and ~110 ms of materials. A VPK-backed `AssetSource` would buy
streaming and hot-reload, neither of which a viewer needs; `Vfs` already exposes
the `load(path) -> Option<Vec<u8>>` an `AssetReader` would be built on, so the
door stays open for Phase 2 to decide deliberately.

# Phase 1 — TF2 Map Viewer in Bevy

## Context

`bevy-tf2/` is currently empty scaffolding (a hello-world `main.rs` and a workspace whose only member, `crates/bevy_sbsp`, is commented out — renamed `crates/bevy_bsp` here, see *Architecture*). The long-term goal is porting TF2 to Bevy; phase 1 establishes the foundation that every later phase depends on: **reading Valve's binary asset formats correctly**.

Phase 1 is deliberately scoped to *rendering*, not gameplay. But the parsers written here (BSP lumps, VPK, VTF, VMT, KeyValues) are the same ones phase 2+ needs for collision, entities, and physics — so they are built as standalone, testable crates with no Bevy dependency, and the Bevy layer sits on top.

**Definition of done:** `cargo run -- --map cp_badlands` opens a Bevy window showing Badlands with brush geometry, displacement terrain, VTF textures, and baked lightmaps, navigable with a free-fly camera. All 233 shipped TF2 maps parse without error.

### Reference material

| What | Where |
|---|---|
| Authoritative BSP struct/lump definitions | [public/bspfile.h](../../source-sdk-2013/src/public/bspfile.h) (1157 lines) |
| `SURF_*` / `CONTENTS_*` flags | [public/bspflags.h](../../source-sdk-2013/src/public/bspflags.h) |
| Reference BSP reader/writer | [utils/common/bsplib.cpp](../../source-sdk-2013/src/utils/common/bsplib.cpp) (5257 lines) |
| Displacement tessellation rules | [public/builddisp.cpp](../../source-sdk-2013/src/public/builddisp.cpp) (3116 lines) |
| Lightmap layout in `LUMP_LIGHTING` | [utils/vrad/lightmap.cpp:3370-3400](../../source-sdk-2013/src/utils/vrad/lightmap.cpp#L3370-L3400), [utils/vrad/radial.cpp:737](../../source-sdk-2013/src/utils/vrad/radial.cpp#L737) |
| Valve LZMA lump header | [public/tier1/lzmaDecoder.h:28-35](../../source-sdk-2013/src/public/tier1/lzmaDecoder.h#L28-L35), [tier1/lzmaDecoder.cpp:128](../../source-sdk-2013/src/tier1/lzmaDecoder.cpp#L128) |
| Image formats | [public/bitmap/imageformat.h:33](../../source-sdk-2013/src/public/bitmap/imageformat.h#L33) |
| VTF header | [public/vtf/vtf.h](../../source-sdk-2013/src/public/vtf/vtf.h) |
| Game lumps (static props) — phase 2 | [public/gamebspfile.h](../../source-sdk-2013/src/public/gamebspfile.h) |

Note: `src/engine/` contains only `audio/` — there is **no** engine renderer source in this SDK. `vbsp`/`vrad`/`vvis` and the `public/` headers are the substitutes.

---

## Verified facts about the real data

Measured directly from `C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\`, not assumed:

- **233 loose `.bsp` files** in `tf/maps/`. All 233 are **`VBSP` version 20** (verified exhaustively by the M1 sweep) — matches `BSPVERSION 20` in bspfile.h. No VPK extraction needed for maps.
- **⚠️ 48 of 64 lumps are LZMA-compressed** in `cp_badlands.bsp`. `lump_t.uncompressedSize != 0` signals compression. This is the single most common thing naive readers get wrong — it is mandatory, not optional.
  - Verified lump body begins with the 17-byte Valve header: magic `"LZMA"`, `actualSize: u32`, `lzmaSize: u32`, `properties: [u8; 5]` (observed `5d 00 00 00 01` = lc/lp/pb + 16 MB dict), then a **raw LZMA1 stream with no end marker** — output size must be supplied externally.
  - `LUMP_GAME_LUMP` (35) and `LUMP_PAKFILE` (40) are **not** compressed.
- `LUMP_PAKFILE` starts with `PK\x03\x04` — a plain zip.
- `LUMP_GAME_LUMP` reports 3 game lumps.
- `tf2_textures_dir.vpk`: signature `0x55aa1234`, **version 2**, tree size 1 586 352.
- `tf/gameinfo.txt` gives the exact search-path order (transcribed in M6 below).

### Struct sizes — validated against cp_badlands lump lengths

Every fixed-stride lump divides exactly. This table is the acceptance test for M1.

| Struct | Size | Lump | Uncompressed len | ÷ size |
|---|---|---|---|---|
| `lump_t` | 16 | — | — | — |
| `dheader_t` | **1036** | — | — | first lump ofs = 1036 ✓ |
| `dplane_t` | 20 | PLANES | 408 440 | 20 422 ✓ |
| `dvertex_t` | 12 | VERTEXES | 352 692 | 29 391 ✓ |
| `dedge_t` | 4 | EDGES | 316 844 | 79 211 ✓ |
| `i32` (surfedge) | 4 | SURFEDGES | 468 936 | 117 234 ✓ |
| `dface_t` | **56** | FACES | 775 320 | 13 845 ✓ |
| `texinfo_t` | 72 | TEXINFO | 434 160 | 6 030 ✓ |
| `dtexdata_t` | 32 | TEXDATA | 4 832 | 151 ✓ |
| `dnode_t` | 32 | NODES | 172 032 | 5 376 ✓ |
| `dleaf_t` (v1) | 32 | LEAFS | 176 512 | 5 516 ✓ |
| `dmodel_t` | 48 | MODELS | 6 672 | 139 ✓ |
| `ddispinfo_t` | **176** | DISPINFO | 209 616 | 1 191 ✓ |
| `CDispVert` | 20 | DISP_VERTS | 848 300 | 42 415 ✓ |
| `CDispTri` | 2 | DISP_TRIS | 120 768 | 60 384 ✓ |
| `u16` (leafface) | 2 | LEAFFACES | 37 714 | 18 857 ✓ |
| `ColorRGBExp32` | 4 | LIGHTING | 10 351 660 | 2 587 915 ✓ |

`dleaf_t` is **version-dependent**: lump version 1 → 32 bytes (ambient cube moved out to `LUMP_LEAF_AMBIENT_LIGHTING`); version 0 → 56 bytes. Read `lump_t.version` and branch. Badlands is v1.

---

## Architecture

Four library crates with no Bevy dependency below the top layer, plus the viewer binary. This split is what makes phase 2+ cheap.

```
bevy-tf2/
  Cargo.toml            # workspace root + viewer binary
  src/main.rs           # viewer app: clap args, plugins, camera, HUD
  crates/
    bsp/                # BSP: header, LZMA lumps, typed structs, displacements, lightmaps
      examples/lumpdump.rs
    vfs/                # VFS: gameinfo search paths, VPK v2, pakfile zip, KeyValues, VMT
      examples/vfsls.rs
    vtf/                # VTF header + image decode / BCn passthrough
      examples/vtf2png.rs
    bevy_bsp/           # Bevy: AssetLoader<BspMap>, mesh+material build, lightmap atlas, shader
```

Crate names are unprefixed. `bsp`, `vfs` and `vtf` shadow same-named crates.io packages, which is harmless here because they are path dependencies and nothing depends on the published ones — but if the differential tests mentioned under *Dependencies* are ever added, alias them (`vbsp`, `vtf as vtf_ref`) rather than renaming ours.

**Build the `examples/` CLI tools.** Debugging a byte-offset bug through a renderer is 10× more expensive than through `lumpdump`. These pay for themselves within the first milestone.

### Coordinate system

Source is **Z-up, right-handed**; Bevy is **Y-up, right-handed, −Z forward**.

```rust
pub const SCALE: f32 = 0.0254; // 1 Source unit ≈ 1 inch; tune to taste
#[inline]
pub fn src_to_bevy(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[2], -v[1]) * SCALE
}
```

This basis map has **determinant +1**, so handedness and triangle winding are preserved — do **not** reverse index order. (Unit scale only affects camera speed and near/far planes; keep it a `const` so it's trivially adjustable.)

---

## Milestones

Each milestone ends in something observable. Do not proceed past a failing verification.

### M1 — Lump reader with LZMA (`crates/bsp`)

- `Bsp::open(path)` → `memmap2` the file, parse `dheader_t`: `ident == u32::from_le_bytes(*b"VBSP")`, `version` (accept 19–21, warn outside), `lumps: [lump_t; 64]`, `map_revision`.
- `LumpReader::raw(id) -> Cow<[u8]>`:
  - `uncompressedSize == 0` → slice `[fileofs .. fileofs+filelen]` directly.
  - else → parse the 17-byte Valve LZMA header, then synthesize a standard *alone*-LZMA 13-byte header (`props[5]` ++ `actual_size as u64 LE`) and feed the remainder to `lzma_rs::lzma_decompress`. Assert output length == `actual_size`.
  - Memoize per lump id (`OnceCell` per slot) — several lumps are read more than once.
- Typed access `LumpReader::slice::<T: Pod>(id)` via `bytemuck::cast_slice`, with `debug_assert!(bytes.len() % size_of::<T>() == 0)`.
- `#[repr(C)]` structs transcribed from bspfile.h, each with `const _: () = assert!(size_of::<X>() == N);`
- `dface_t`: the `m_NumPrims` field's top bit is `!AreDynamicShadowsEnabled` — mask with `0x7FFF` for the count ([bspfile.h:772](../../source-sdk-2013/src/public/bspfile.h#L772)).

**Verify:** `cargo test -p bsp` passes struct-size asserts. `cargo run -p bsp --example lumpdump -- --all-maps <tf dir>` walks all 233 maps, decompresses every lump, and asserts stride divisibility — zero failures.

### M1 findings

Implemented in `crates/bsp`. Confirmed against real data:

**Acceptance gate passed:** the all-maps sweep inflated all 64 lumps of all **233 maps — 0 failures, 14.8 GiB inflated**. It also settled three things the plan had only sampled:

- Every one of the 233 maps is **BSP version 20**, and every one has **LEAFS lump version 1**. The 56-byte `dleaf_t` v0 branch is therefore *not* reachable from TF2 content — `dm_lockdown.bsp` in the SDK is the only test case that covers it, which is why it is worth keeping in the loop.
- Displacement powers across all maps are exactly `{2, 3, 4}`, confirming the `MIN_MAP_DISP_POWER`/`MAX_MAP_DISP_POWER` range M4 relies on. No map violates it.
- `cp_badlands.bsp` inflates 26.0 MB on disk to 51.3 MB across 48 compressed lumps, and every stride in the table above divides exactly.
- **Trap: an absent lump is not a zero-length cast.** A map with no displacements has `filelen == 0` for DISPINFO, and an empty `&[u8]` carries a 1-aligned *dangling* pointer, so `bytemuck::cast_slice` to a 4-aligned struct fails on it. `cp_cloak.bsp` is the first map in the sweep that hits this. Return an empty slice before casting; `lump_slice` now does, with a regression test.
- **`dm_lockdown.bsp` from the SDK is the best second test case**: BSP v19, LEAFS lump **version 0** (56-byte leaves), and zero compressed lumps — so it covers the version-dependent stride branch and the uncompressed path that TF2 maps never exercise.
- Static prop game lump version varies by game: `sprp` v10 in TF2, **v5** in the HL2MP map. Phase 2 must branch on it rather than assuming `GAMELUMP_STATIC_PROPS_VERSION`.
- GAME_LUMP reports one more entry than there are real lumps: the last is the terminal null dictionary entry `bspfile.h` mentions. Ignore a zero id.
- Casts use `try_cast_slice`, not `cast_slice` — a malformed map should surface as a `BspError`, never a panic inside a library.

### M2 — Brush face geometry

- Read VERTEXES, EDGES, SURFEDGES, FACES, PLANES, TEXINFO, TEXDATA, TEXDATA_STRING_TABLE (`i32` offsets), TEXDATA_STRING_DATA (NUL-terminated names).
- Worldspawn is `models[0]`; its faces are `[firstface, firstface + numfaces)`.
- Face → polygon ring:
  ```rust
  for i in 0..face.numedges {
      let se = surfedges[(face.firstedge + i) as usize];
      let vi = if se >= 0 { edges[se as usize].v[0] } else { edges[(-se) as usize].v[1] };
  }
  ```
- Skip a face if `texinfo < 0`, if `dispinfo >= 0` (M4 owns those), or if flags hit `SURF_NODRAW | SURF_SKY | SURF_SKY2D | SURF_HINT | SURF_SKIP | SURF_TRIGGER` (values in [bspflags.h:80-97](../../source-sdk-2013/src/public/bspflags.h#L80-L97)).
- Triangulate as a fan `(0, i, i+1)` — Source faces are convex, so this is valid.
- Normal = `planes[face.planenum].normal`, negated when `face.side != 0`.
- Texture UV from `texinfo.textureVecsTexelsPerWorldUnits`:
  ```
  u = (dot(pos, tv[0].xyz) + tv[0][3]) / texdata.width
  v = (dot(pos, tv[1].xyz) + tv[1][3]) / texdata.height
  ```
  Compute in **Source space with unscaled coords**, before `src_to_bevy`.
- **Also render brush entities.** Parse the ENTITIES lump (KeyValues text, reuse `vfs`'s KV lexer) and for each entity with `model "*N"`, emit `models[N]`'s faces translated by its `origin`. Without this, doors, gates and moving platforms are missing holes in the world.

**Verify:** `geomdump --all-maps` builds every model of every map. Fail on out-of-range edge indices, lump errors, or a worldspawn that yields no faces. ~~no degenerate triangles~~ — see M2 findings; zero-area triangles are normal.

### M2 findings

**Acceptance gate passed:** built every model of all **233 maps — 0 failures**, 16.7 M verts / 9.34 M tris total. **Zero faces with out-of-range edge indices and zero faces with fewer than 3 edges across every map**, which is the real signal that the signed-surfedge walk is right. Heaviest map is `cp_fulgur` (89 820 tris); most materials is `pl_embargo` (586).

- **Zero-area fan triangles are expected map content, not a defect.** ~10% of fan triangles on `cp_badlands` come out *exactly* zero area. vbsp inserts extra vertices along straight edges to close t-junctions against neighbouring faces, so a face is frequently a triangle or quad carrying a long collinear run — one 21-vertex Badlands face is really a triangle with 18 collinear points on one edge. A fan from vertex 0 emits a zero-area triangle for every such pair, and **loses no surface area**, because those triangles lie on a line through the apex. The `|cross|` histogram is bimodal with a clean gap: 10.08% exactly `0`, then 0.01% below `1e-4`. Across all 233 maps this is 836 573 of 10.2 M fan triangles (8.2%). They are dropped (they render nothing and would give NaN tangents in M8) and counted, never treated as an error.
- Badlands worldspawn: 88 materials, 62 787 verts, 35 154 tris from 11 812/13 483 faces — 1191 faces deferred to M4 as displacements, 480 skipped as tool surfaces. The three heaviest materials are all wooden structure (`WOOD/WOOD_BRIDGE001` at 6519 tris).
- Grouping by texdata collapses 11 812 faces to **88 draw calls**, not the 151 the material count suggested — 63 of Badlands' materials are used only by displacements or tool brushes.
- Brush entities (models 1..138) total only 704 verts / 352 tris, so deferring their placement to M6 costs almost nothing visually.

### M3 — Bevy app + free camera

- bevy **0.19.1** (latest on crates.io as of writing; Rust 1.98 toolchain is present).
- `bevy_bsp::BspPlugin` registering an `AssetLoader` for `*.bsp` producing a `BspMap` asset; the viewer resolves `--map <name>` against the VFS.
- Free-fly camera: WASD + QE, mouse-look on right-drag or captured cursor, Shift to sprint. Far plane generous — TF2 maps span ~16 000 units (~400 m at `SCALE`).
- Debug HUD (F1): map name, tri/vert/drawcall counts, camera position in **Source units** (so it can be cross-checked against `cl_showpos` in the real game).
- Wireframe toggle (F2) and a "hide displacements / hide brushes" toggle — invaluable for M4/M5 debugging.

**Verify:** fly around a flat-shaded Badlands.

### M4 — Displacement surfaces

Most TF2 maps put all ground and cliffs in displacements; Badlands has 1191 of them.

- For each `ddispinfo_t`, the parent face is `m_iMapFace`. It must be a 4-edge quad — assert this.
- Find the quad corner nearest `startPosition` → that becomes index 0; rotate the corner ring so orientation matches vbsp's.
- Grid side = `2^power + 1` (`power ∈ [2,4]` → 5/9/17 per side = 25/81/289 verts), `NUM_DISP_POWER_VERTS` in [bspfile.h:53](../../source-sdk-2013/src/public/bspfile.h#L53).
- Vertex `(i, j)`, with `s = i/(side-1)`, `t = j/(side-1)`:
  ```
  base  = bilerp(c0, c1, c2, c3, s, t)
  final = base + disp_verts[m_iDispVertStart + j*side + i].m_vVector
                * disp_verts[...].m_flDist
  alpha = disp_verts[...].m_flAlpha   // 0..255 blend weight for $basetexture2
  ```
- Triangulate the grid following `builddisp.cpp` — **alternate the quad diagonal by `(i + j)` parity**, not a fixed diagonal, or terrain silhouettes will visibly differ from the real game.
- Texture and lightmap UVs come from the parent face's `texinfo`, evaluated at the **final displaced** position (this is what the engine does).
- Carry `alpha` into `ATTRIBUTE_COLOR` (or a spare UV channel) for the M8 two-texture blend.
- `CDispTri.m_uiTags` — `DISPTRI_TAG_REMOVE` triangles should be skipped.

**Verify:** Badlands terrain is watertight, no gaps at displacement seams, silhouettes match a TF2 screenshot from the same position.

### M5 — Baked lightmaps

- Source lump: `LUMP_LIGHTING` (8) for LDR. `LUMP_LIGHTING_HDR` (53) also present — start with LDR, keep the lump id a parameter.
- Samples are `ColorRGBExp32 { r: u8, g: u8, b: u8, exponent: i8 }` (4 bytes, [mathlib.h:990](../../source-sdk-2013/src/public/mathlib/mathlib.h#L990)).
- Per face: skip if `styles[0] == 255 || lightofs < 0`.
  ```
  w = m_LightmapTextureSizeInLuxels[0] + 1
  h = m_LightmapTextureSizeInLuxels[1] + 1
  numluxels = w * h
  ```
- **Layout subtlety (get this right or lighting will be offset by a few bytes per face):** `lightofs` already points *past* a `lightstyles * 4` byte block of average-face-colour data that vrad writes immediately before the samples ([lightmap.cpp:3395-3397](../../source-sdk-2013/src/utils/vrad/lightmap.cpp#L3395-L3397)). Samples are then ordered `[style][bumpSample][luxel]`, where `bumpSampleCount = 4` if `texinfo.flags & SURF_BUMPLIGHT` else `1` ([radial.cpp:737](../../source-sdk-2013/src/utils/vrad/radial.cpp#L737)). **For phase 1 read style 0, bumpSample 0 — the first `numluxels` entries at `lightofs`.**
- Decode: `linear_rgb = vec3(r, g, b) * exp2(exponent) / 255.0` — the bytes are **linear**, not sRGB.
- Pack every face's `w × h` rect into one atlas (skyline/shelf packer). **Add a 1px border by duplicating edge luxels** — without it, bilinear filtering bleeds neighbouring faces' lighting across seams. Emit as `Rgba16Float` to preserve the exponent range (`Rgba8UnormSrgb` after tonemap is an acceptable fallback but clips bright outdoor luxels).
- Lightmap UV per vertex, in Source space:
  ```
  lu = dot(pos, lmVecs[0].xyz) + lmVecs[0][3] - m_LightmapTextureMinsInLuxels[0]
  lv = dot(pos, lmVecs[1].xyz) + lmVecs[1][3] - m_LightmapTextureMinsInLuxels[1]
  u  = (lu + 0.5) / w      // half-texel centre
  v  = (lv + 0.5) / h
  ```
  then remap into the atlas rect. Store in `ATTRIBUTE_UV_1`.
- Faces with `texinfo.flags & (SURF_SKY | SURF_NOLIGHT)` (vrad's `TEX_SPECIAL`, [vrad.h:432](../../source-sdk-2013/src/utils/vrad/vrad.h#L432)) have no lightmap at all.

**Verify:** dump the atlas to PNG and inspect. In-window: interiors dark, open ground bright, shadow shapes matching the real game. This step is where a viewer starts to look like TF2 even with white albedo.

### M6 — VFS: gameinfo, VPK v2, pakfile, VMT

`crates/vfs`. **Note there is no VPK code in the SDK** (`src/filesystem/` is absent) — implement from the well-documented format, validated against the real files.

- **KeyValues lexer** first — `gameinfo.txt`, `.vmt`, and the ENTITIES lump are all the same format (quoted/unquoted tokens, `{}` blocks, `//` comments, duplicate keys allowed → keep an ordered multimap).
- **Search paths**, in this exact order (transcribed from the real `tf/gameinfo.txt`; `|all_source_engine_paths|` → `<install>/`, `|gameinfo_path|` → `<install>/tf/`, and a `tf2_textures.vpk` reference resolves to `tf2_textures_dir.vpk`):
  1. **BSP pakfile** (`LUMP_PAKFILE` zip) — mounted at the top, only while that map is loaded
  2. `tf/custom/*` (VPKs and subdirs, alphabetical)
  3. `tf/tf2_textures_dir.vpk`
  4. `tf/tf2_misc_dir.vpk`
  5. `tf/` (loose)
  6. `hl2/hl2_textures_dir.vpk`
  7. `hl2/hl2_misc_dir.vpk`
  8. `hl2/` (loose)

  Sound/VO VPKs are irrelevant to phase 1 — skip them. Config via `--game-dir` / `BEVY_TF2_GAME_DIR`, defaulting to the detected Steam path.
- **VPK v2 reader:** header `{ signature: u32 = 0x55aa1234, version: u32 = 2, tree_size: u32 }` then `{ file_data_section_size, archive_md5_section_size, other_md5_section_size, signature_section_size }: u32`. Directory tree is a three-level nesting of NUL-terminated strings (extension → path → filename, each level ended by an empty string); each leaf carries `{ crc: u32, preload_bytes: u16, archive_index: u16, entry_offset: u32, entry_length: u32 }` followed by `preload_bytes` of inline data. `archive_index == 0x7fff` means the payload lives in the `_dir` file itself, after the tree; otherwise it's in `tf2_textures_<NNN>.vpk`. A file can be preload-only, archive-only, or both concatenated. Build a `HashMap<String, VpkEntry>` with lowercased, `/`-normalised paths.
- **Pakfile zip:** use the `zip` crate over a `Cursor` on the decompressed lump.
- **VMT parse:** shader name (`LightmappedGeneric`, `WorldVertexTransition`, `UnlitGeneric`, `Water`, `SkyBox`), `$basetexture`, `$basetexture2`, `$translucent`, `$alphatest`, `$nocull`, `$surfaceprop`. Implement `Patch { include, replace }` inheritance — TF2 uses it heavily.

**Verify:** `cargo run -p vfs --example vfsls -- materials/concrete/` lists entries and their source path. Unit test resolving a handful of known VMTs and dumping keys. Cross-check a few resolutions against what the real game loads.

### M7 — VTF decode

- Header: `VTF\0`, `version: [u32; 2]`, `header_size`, `width: u16`, `height: u16`, `flags: u32`, `frames: u16`, `first_frame: u16`, pad, `reflectivity: [f32;3]`, pad, `bump_scale: f32`, `image_format: i32`, `mip_count: u8`, `low_res_format: i32`, `low_res_w: u8`, `low_res_h: u8`; 7.2+ adds `depth: u16`; 7.3+ adds `num_resources: u32` and a resource directory — locate the high-res image data via tag `[0x30, 0, 0]`. Always seek by `header_size`, never by a computed struct size.
- **Mips are stored smallest-first**; within a mip, iterate frames → faces → slices.
- Formats to support (indices from [imageformat.h:33](../../source-sdk-2013/src/public/bitmap/imageformat.h#L33)): `DXT1`, `DXT1_ONEBITALPHA`, `DXT3`, `DXT5`, `BGR888`, `BGRA8888`, `BGRX8888`, `RGBA8888`, `I8`, `IA88`, `A8`, `UV88`, `RGBA16161616F`.
- **Prefer BCn passthrough:** hand DXT1/3/5 blocks straight to wgpu as `Bc1RgbaUnormSrgb` / `Bc2` / `Bc3RgbaUnormSrgb` via Bevy's `Image` — no CPU decode, and it keeps VRAM at parity with the real game. Keep `texpresso` behind a feature flag as a CPU fallback for backends without BC support. CPU-convert only the uncompressed formats.
- Respect the `TEXTUREFLAGS_SRGB`-adjacent conventions: albedo → sRGB view, normal maps (`UV88`, `$bumpmap`) → linear.

**Verify:** `cargo run -p vtf --example vtf2png -- materials/concrete/concretewall001.vtf` and eyeball the output against the in-game texture.

### M8 — Material assembly and final render

- **Group faces by `texdata` index**, one mesh per material. Badlands has 151 texdatas vs 13 845 faces — this is the difference between ~150 and ~14 000 drawcalls.
- Custom `Material` + WGSL shader:
  - `LightmappedGeneric`: `albedo(uv0) * lightmap(uv1) * OVERBRIGHT` (Source's overbright factor is 2.0).
  - `WorldVertexTransition`: `mix(basetexture, basetexture2, vertex_alpha) * lightmap` — this is what makes displacement blend zones (dirt→grass) look right.
  - `$alphatest` → discard below `$alphatestreference` (default 0.5); `$translucent` → alpha blend, sorted back-to-front.
- Sky faces: skip geometry, use a flat clear colour (real skybox is phase 2).
- `Water` shader materials: render as a plain translucent surface with the `$basetexture` for now.

**Verify:** side-by-side screenshots vs the real game at matching `cl_showpos` coordinates for `cp_badlands`, `ctf_2fort`, `pl_upward`, `koth_harvest_final`.

---

## Dependencies

```toml
[workspace.dependencies]
bevy      = "0.19"
bytemuck  = { version = "1", features = ["derive"] }
memmap2   = "0.9"
lzma-rs   = "0.3"     # BSP lump decompression — mandatory, see M1
zip       = { version = "2", default-features = false, features = ["deflate"] }
thiserror = "2"
clap      = { version = "4", features = ["derive"] }
image     = "0.25"    # examples/vtf2png only
texpresso = "2"       # optional CPU BCn fallback
```

Available and confirmed on crates.io. The published `vbsp` 0.9.1 / `vtf` 0.4.1 / `vpk` 0.3.0 can be pulled in as **dev-dependencies** for differential testing if a lump's interpretation is ever in doubt — but the shipped code is hand-written, as chosen. Since our own crates are named `bsp`/`vfs`/`vtf`, add such a dev-dependency under an alias (`vtf_ref = { package = "vtf", version = "0.4" }`) to avoid the name clash.

---

## Verification

**Automated:**
```bash
cargo test --workspace                  # struct sizes, KV/VMT/VPK unit tests
cargo run -p bsp --example lumpdump -- --all-maps "<tf dir>"   # all 233 maps, 0 errors
```
The all-maps sweep is the highest-value test in phase 1 — it catches version drift, `dleaf_t` v0/v1 branching, unusual displacement powers, and community-map edge cases in one command.

**Manual:**
```bash
cargo run -- --map cp_badlands
cargo run -- --map ctf_2fort
cargo run -- --map pl_upward
```
For each: fly the map, confirm no missing terrain, no missing textures (log every unresolved material path — the log should be empty), lighting consistent with the real game, and the F1 HUD showing sane counts. Use F2 wireframe and the brush/displacement toggles to isolate anything that looks wrong.

---

## Out of scope — phase 2 and later

Called out explicitly so phase 1 doesn't creep:

- **Static props** — `LUMP_GAME_LUMP` `'sprp'` ([gamebspfile.h:28](../../source-sdk-2013/src/public/gamebspfile.h#L28), version 10) plus a full MDL/VVD/VTX loader. This is the single biggest visual gap after phase 1 and the natural start of phase 2.
- Detail props (`'dprp'`), overlays/decals, cubemap reflections, 3D skybox, real water shaders, `$bumpmap` / phong / self-illum.
- VIS/PVS culling — Bevy's frustum culling is sufficient for a viewer at these poly counts.
- Entity logic, brush-entity *motion*, collision, VPhysics, netcode, gameplay.

## Risks

| Risk | Mitigation |
|---|---|
| LZMA lump handling wrong → garbage everywhere | Already verified format and props bytes against real data; assert decompressed length == `actualSize` on every lump. |
| `lightofs` avg-colour prefix / bump-sample stride misread | Documented precisely in M5 with source citations; validate by dumping the atlas to PNG before wiring the shader. |
| Displacement diagonal parity wrong → subtly wrong terrain | Compare silhouettes against in-game screenshots; `builddisp.cpp` is the ground truth. |
| VPK v2 reader bugs (preload + archive split) | `vfsls` example over a known-good file list; optional differential test vs the `vpk` crate. |
| Bevy 0.19 API churn during the phase | Pin the exact patch version in the workspace; keep all Bevy contact inside `bevy_bsp` and `main.rs` so the format crates are unaffected. |

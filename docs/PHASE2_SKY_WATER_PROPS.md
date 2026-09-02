# Phase 2 — Sky, Water, and Static Props

> Companion to [`PHASE1_MAP_VIEWER.md`](PHASE1_MAP_VIEWER.md). Like that
> document, this one gains a `### Mn findings` section as each milestone lands,
> so the record of what the data actually turned out to be lives next to the
> plan that predicted it.

## Context

Phase 1 delivered a working map viewer: brush geometry, displacement terrain, VTF textures and baked lightmaps, verified across all 233 shipped TF2 maps. What it renders is *architecturally* complete and *visually* half-finished. Three gaps dominate a screenshot:

- **The sky is a flat clear colour.** Every one of the 233 maps names a real sky, and 215 additionally place a `sky_camera` for a 3D skybox.
- **Water is a provisional translucent teal.** 603 water surfaces across 144 maps, 464 distinct materials — and only 65 of those have a `$basetexture` to draw.
- **There are no props.** 353 116 static props across 8 584 unique models — the barrels, fences, pipes, signs and ammo crates that make a Source map read as populated rather than as bare architecture.

**Order: sky and water first, props second.** Sky and water reuse the VTF/VMT/KeyValues work already shipped and need no new binary format, so they land quickly and change screenshots immediately. Props are three new interlocking formats and deserve their own uninterrupted run of milestones.

**⚠️ Within that, water must precede the 3D skybox.** Not a preference — the sky pass draws its own room's water (`DF_RENDER_WATER` in `CSkyboxView::Setup`), and `ctf_2fort`'s sky room is 51 faces of which 22 are water. M11 is unverifiable on most maps until M10 works. Both were attempted in the other order, and both were reverted.

**Definition of done:** `cargo run -- --map cp_badlands` shows a textured sky, a scaled 3D skybox, fogged water, and populated props lit consistently with the baked world. All 233 maps load without error, and coverage-measured screenshots stay non-empty.

### Reference material

| What | Where |
|---|---|
| Game lump ids, `StaticPropLump_t` and its V4/V5/V6 predecessors | [public/gamebspfile.h](../../source-sdk-2013/src/public/gamebspfile.h) |
| `studiohdr_t`, `mstudiobodyparts_t`, `mstudiovertex_t`, `vertexFileHeader_t` | [public/studio.h](../../source-sdk-2013/src/public/studio.h) (3200 lines) |
| VTX strip / stripgroup / mesh hierarchy | [public/optimize.h](../../source-sdk-2013/src/public/optimize.h) |
| Leaf ambient cube layout | [public/bspfile.h](../../source-sdk-2013/src/public/bspfile.h) — `dleafambientlighting_t`, `dleafambientindex_t` |
| Overlay lump | [public/bspfile.h:1007](../../source-sdk-2013/src/public/bspfile.h#L1007) — `doverlay_t` |
| Static prop placement / fade / lighting flags | [public/gamebspfile.h](../../source-sdk-2013/src/public/gamebspfile.h) — `STATIC_PROP_*` |
| Displacement luxel rules (already used, still relevant) | [public/builddisp.cpp](../../source-sdk-2013/src/public/builddisp.cpp), [utils/vbsp/disp_vbsp.cpp](../../source-sdk-2013/src/utils/vbsp/disp_vbsp.cpp) |

As in Phase 1: there is **no engine renderer source** in this SDK. `vbsp`/`vrad`/`vvis` and the `public/` headers are the ground truth, and where they are silent — VPK, VMT semantics, sky face orientation — the shipped data is.

---

## Verified facts about the real data

Measured from the shipped game during planning, not assumed. These are the acceptance targets.

### ⚠️ Individual game lumps are LZMA-compressed, and Phase 1's note on this is wrong

M1 recorded that `LUMP_GAME_LUMP` "is not compressed". True at the *lump directory* level — its `lump_t.uncompressedSize` is 0 — and **wrong at the game-lump level**. On `cp_badlands` the `sprp` entry has `flags = 0x1` (`GAMELUMPFLAG_COMPRESSED`) and its body begins `4c 5a 4d 41` — the same 17-byte Valve LZMA header as any compressed lump.

Two consequences, either of which yields garbage:

- **`dgamelump_t.filelen` is the *uncompressed* size when the compressed flag is set.** The compressed byte span must come from the **next** directory entry's `fileofs` (and for the last entry, from the end of `LUMP_GAME_LUMP`). Reading `filelen` bytes reads far past the data. Verified on `cp_badlands`: `sprp` at 14 873 872 with `filelen` 112 396, next entry at 14 897 344 → a 23 472-byte compressed span, matching the header's `lzmaSize` of 23 455 plus 17.
- Both paths ship: **177 maps have a compressed `sprp`, 56 have it uncompressed.** A reader handling only one fails on a quarter to three quarters of the game. Same split for `dprp` (177/56), `dplt` and `dplh` (100/26).

### ⚠️ TF2 ships four `sprp` versions and five MDL versions

M1 sampled `sprp` v10 and the plan assumed it throughout. Across all 233 maps:

| `sprp` version | Bytes per prop | Maps | Props |
|---|---|---|---|
| 10 | **72** | 193 | 316 251 |
| 7 | **72** | 1 | 536 |
| 6 | **64** | 37 | 36 150 |
| 5 | **60** | 1 | 179 |

Every stride divides exactly with **0 leftover bytes** — this table is M12's acceptance test, the same way the struct-size table was M1's. Note v7 measures the same 72 bytes as v10; whether the *fields* match, or v7 is 68 bytes plus padding, must be checked against `StaticPropLumpV6_t`/V10 before its tail fields are trusted. It is one map and 0.15% of props, so failing closed there is acceptable.

**MDL versions of the models maps actually reference** — sampled over 20 maps, 1 236 unique models:

| MDL version | Count | Share |
|---|---|---|
| 48 | 467 | 50% |
| 44 | 257 | 28% |
| 45 | 181 | 20% |
| 46 | 19 | 2% |
| 47 | 1 | <1% |

**Half the props in use are pre-48.** Not a legacy corner to defer — a v48-only loader fails on half the content. `STUDIO_VERSION` in this SDK is 48, so the older layouts have to be handled by field presence rather than read off the header struct.

By contrast **VVD is version 4 and VTX version 7, uniformly** (14 065 and 14 064 files in `tf2_misc_dir.vpk`). The version risk lives entirely in the `.mdl`.

### Models on disk

- **`.vtx` is never the extension.** Files are `<model>.dx90.vtx`, `.dx80.vtx`, `.sw.vtx` — 48 648 VTX for 16 308 MDL. **Use `.dx90.vtx`** (16 215, matching the VVD count).
- **Every referenced model that exists has both a `.vvd` and a `.dx90.vtx`** — 0 missing across the sample. 92 MDL in the VPK have no VVD (animation-only), so absence must be tolerated but props never hit it.
- **25% of referenced models come from a map's own pakfile or `custom/`** — 311 of 1 236. M6's pakfile mounting is load-bearing here.
- MDL magic is always `IDST`. Heaviest map is `pl_patagonia` (4 383 props); exactly one map has none.
- `studiohdr_t` offsets needed for static props: `numtextures` 204, `textureindex` 208, `numcdtextures` 212, `cdtextureindex` 216 — all in the fixed header prefix, but **assert this on real files of all five versions** rather than assuming.

### Entities, and what is *not* an entity

- **`prop_static`, `env_cubemap`, `info_overlay` and `func_detail` appear in ENTITIES on zero maps.** vbsp compiles them into the `sprp` game lump, the CUBEMAPS lump, the OVERLAYS lump, and worldspawn respectively. Do not look for them as entities.
- `prop_dynamic` is on all 233 maps and `info_player_teamspawn` on all 233; `water_lod_control` on 183, `shadow_control` on 225, `env_sun` on 47.
- **CUBEMAPS is non-empty on all 233 maps, OVERLAYS on 232**, `LEAF_AMBIENT_LIGHTING` on all 233, `PHYSCOLLIDE` on all 233, `PHYSDISP` on 206.
- **`light_environment` is on 231 maps, and every one of the 265 instances carries a `pitch` key**, which overrides the pitch component of `angles` — a standard Source trap.
- **`sky_camera`: 211 maps have exactly 1, 18 have none, 4 have 2–3** (the engine uses the first). `scale` is 16 on 215 maps but also 18, 32, 64 and 128 — **read it, never hardcode**. `fogenable 1` on 201.

### Sky materials

- Shader is **`sky`**, not `SkyBox`. `sky_badlands_01up.vmt` is `$basetexture skybox/sky_badlands_01up`, `$hdrbasetexture` (same LDR path), `$nofog 1`, `$ignorez 1`.
- 964 files under `materials/skybox/`; **179 are `_hdr` variants** named `<name>_hdr<face>`, mostly HL2 — TF2's own skies point `$hdrbasetexture` at the LDR texture, so **LDR is the correct default**.
- **⚠️ Sky faces are neither square nor uniform.** `sky_badlands_01`: the four sides are **512 × 256**, `up` and `dn` are **512 × 512**, all `BGR888`, VTF 7.4, flagged `CLAMPS | CLAMPT | NOMIP | NOLOD`. A cubemap needs six equal square faces, which is why the engine draws the sky as six *2D* textures instead — see M9. `BGR888` also means CPU decode — no BC passthrough.
- **Sky formats vary.** Several skies (`sky_trainyard_01`, `sky_night_01`, `sky_alpinestorm_01`) are `DXT1`, and some sets are not named `<skyname><face>` uniformly (`sky_dustbowl_01rt.vtf` does not exist under that name). Our `vtf` crate already decodes every format present; a loader must resolve the face texture through the **VMT's `$basetexture`** rather than guessing the filename.
- **⚠️ There is no cubemap in Source's drawn sky.** The `Sky` shader is 2D throughout — one texcoord set, `tex2D`, `UnlitGeneric` as its dx6 fallback, and `texCUBE` in none of its files — and the engine's sky interface is `SkyBoxMaterials_t`, six *materials* ([public/cdll_int.h:123](../../source-sdk-2013/src/public/cdll_int.h#L123)). Each face is `CLAMPS | CLAMPT | NOMIP | NOLOD`, so nothing filters across a face join or leaves mip 0. Citations and consequences in M9.
- **⚠️ Valve ships two different face orderings, and `bk`/`lf` are swapped between them.** The **drawn sky** uses `rt, bk, lf, ft, up, dn` ([cdll_int.h:125](../../source-sdk-2013/src/public/cdll_int.h#L125), [skyboxswapper.cpp:60](../../source-sdk-2013/src/game/server/skyboxswapper.cpp#L60), [vscript_server.cpp:2190](../../source-sdk-2013/src/game/server/vscript_server.cpp#L2190)); the **reflection cubemap** uses `rt, lf, bk, ft, up, dn` ([cubemap.cpp:195](../../source-sdk-2013/src/utils/vbsp/cubemap.cpp#L195), matching `CubeMapFaceIndex_t`). Anything that indexes a six-element list *positionally* from the wrong file is silently wrong. Take directions from `CubeMapFaceIndex_t`'s per-face comments, never from slot position.

### Water materials

> **Rewritten after both M10 and M11 were reverted.** Every number here is
> measured from the shipped game or read out of the SDK; the paragraph this
> replaced was assembled from a shader-name census and got three things wrong.

- **603 water surfaces across 144 of the 233 maps, 464 distinct materials.** The
  earlier "547 `Water` references" counted something else — 403 of the 464
  materials use the `Water` shader, and the rest are `LightmappedGeneric` (56),
  `LightMappedGeneric` (4, note the case) and `UnlitGeneric` (1).
- **⚠️ Two of the three classifiers are redundant, and the plan's reason for
  using all three was wrong.** Measured over the 464:

  | test | catches |
  |---|---|
  | shader is `Water*` | 403 |
  | `%compilewater 1` | **464** |
  | `SURF_WARP` on the face | **464** |

  `%compilewater` and `SURF_WARP` agree on *every single material* — which is
  causation, not luck: vbsp reads `%compilewater` and sets `SURF_WARP` from it.
  The **shader name** is the one that genuinely falls short. Test all three
  anyway, but for robustness (a face whose VMT is missing still has its bit),
  not because no single test suffices.
- **⚠️ A colour's scale comes from its delimiter, and the two differ by 255×.**
  `{51 43 13}` is 0–255 integers; `[0.95 1.0 0.97]` is 0–1 floats. The same key
  takes either form: `$fogcolor` is brace in **all 446** materials that set it,
  while `$reflecttint` is 47 brace / 16 bracket and `$refracttint` 46 / 14. Read
  a brace value as floats and the clamp turns 2fort's dark olive into **pure
  white** — which, with a working depth gradient, still looks like water.
- **⚠️ The ripple key is `$normalmap`, not `$bumpmap`** — 459 materials against
  61 — and the SDK agrees:
  `SHADER_PARAM( NORMALMAP, ..., "dev/water_normal", "normal map" )`.
- **The ripple is a 60-frame animation, not a scroll.**
  `water/tfwater001_normal.vtf` carries 60 frames, played by an
  `AnimatedTexture` proxy at `animatedtextureframerate 30.00` — a two-second
  loop, and it *is* the motion. Two `Sine` proxies (periods 24 s and 16 s, ±0.5,
  the second with `sinemin`/`sinemax` swapped for phase inversion) drift its UVs
  on top via `$bumptransform`.
- **`$bottommaterial` is resolved at compile time**, on 444 materials. vbsp's
  `AssignBottomWaterMaterialToFace` (`utils/vbsp/faces.cpp:1255`) rewrites the
  **downward-facing** water face's texinfo, so the underside is already separate
  geometry in the BSP. There is nothing to swap at runtime.
- `$reflecttexture`/`$refracttexture` name **render targets**
  (`_rt_WaterReflection`), not files. But `WaterCheap` needs neither — see M10.
- `$envmap` is on **447** of the 464; `$fogcolor`/`$fogstart`/`$fogend`/
  `$fogenable` on 446; `$abovewater` on 459; `$scale` on 403; `$forcecheap` on
  33 and `$forceexpensive` on 9.
- **SDK defaults, which are not neutral:** `$cheapwaterstartdistance` 500,
  `$cheapwaterenddistance` 1000, `$forceexpensive` **1** on PC, and `$fogcolor`
  defaults to **`(1 0 0)` — red — with a `Warning`**
  (`materialsystem/stdshaders/water.cpp:61-129`). A missing `$fogcolor` is a loud
  authoring error; defaulting it to something tasteful hides exactly what the
  engine shouts about.
- **`$fogstart` is frequently negative** — `water/water_2fort.vmt` is
  `-100 .. 400` — meaning the fog has already begun at the waterline.
- Water is **lightmapped** (`MATERIAL_VAR2_LIGHTING_LIGHTMAP`) and needs a
  **tangent basis** (`MATERIAL_VAR2_NEEDS_TANGENT_SPACES`), both set in
  `water.cpp`'s `SHADER_INIT_PARAMS` (:61-129).

---

## Architecture

One new crate; the existing four grow. Phase 1's split holds — nothing below `bevy_bsp` gains a Bevy dependency.

```
crates/
  bsp/      + gamelump.rs   (M12: per-lump LZMA, sprp v5/6/7/10, dprp)
            + ambient.rs    (M15: leaf ambient cube lookup)
            + area.rs       (M11: BSP areas + point-to-leaf walk, for the sky room)
            examples/propdump.rs
  mdl/      NEW — studiohdr / VVD / VTX, engine-free
            examples/mdldump.rs
  vfs/      (unchanged — pakfile mounting already covers packed models)
  vtf/      (unchanged — cubemap faces already exposed via surface())
  bevy_bsp/ + sky.rs      (M9:  six quads, six native-size 2D faces — no cubemap)
            + water.rs    (M10: WaterCheap tier, then the fog volume)
            + skybox3d.rs (M11: sky-camera transform + the two-pass setup)
            + props.rs    (M14)
            examples/skydump.rs, waterdump.rs, skyroom.rs
```

`crates/mdl` returns plain vertex/index buffers plus material names, exactly as `bsp::geometry` does, so the Bevy layer stays a thin adapter. Static props are rigid, so **no skinning** in Phase 2 — but bone transforms are read, because a model's bind pose is not guaranteed to be identity.

---

## Milestones

Same rule as Phase 1: each ends in something observable, and a failing verification stops the milestone.

### M9 — 2D skybox, as six quads

> **This milestone was rewritten after the first attempt was reverted.** The
> original plan assembled one cubemap and searched for the face arrangement; that
> is retired, for the reasons under *M9 findings* below. The SDK does not build a
> cubemap for the drawn sky, and following it removes the entire seam class
> rather than tuning it.

**Source's 2D sky is six independent quads with six independent 2D textures.**
The `Sky` shader takes `VERTEX_POSITION` plus **one 2D texcoord set**
([sky_dx9.cpp:59](../../source-sdk-2013/src/materialsystem/stdshaders/sky_dx9.cpp#L59)),
samples with `tex2D`
([sky_ps2x.fxc](../../source-sdk-2013/src/materialsystem/stdshaders/sky_ps2x.fxc)),
and falls back to plain `UnlitGeneric` on dx6
([sky_dx6.cpp:13](../../source-sdk-2013/src/materialsystem/stdshaders/sky_dx6.cpp#L13)).
`texCUBE` appears in **no** sky shader file. The engine's own sky interface is
six *materials*:

```c
struct SkyBoxMaterials_t
{
	// order: "rt", "bk", "lf", "ft", "up", "dn"
	IMaterial *material[6];
};
```
— [public/cdll_int.h:123](../../source-sdk-2013/src/public/cdll_int.h#L123)

Six 2D textures is also the only thing consistent with the files: sides are
512 × 256 and caps 512 × 512, which no cubemap can hold. And each face carries
`CLAMPS | CLAMPT | NOMIP | NOLOD`, so the engine never filters across a face
boundary and never leaves mip 0.

**Steps:**

- `worldspawn`'s `skyname` → `materials/skybox/<name>{rt,lf,bk,ft,up,dn}.vmt`,
  shader `sky`. **Resolve the texture through `$basetexture`**, not by filename
  convention — `sky_hydro_01dn` points at `sky_hydro_01bk`, and
  `sky_dustbowl_01rt.vtf` does not exist under that name. A face with no
  `$basetexture` becomes a 1 × 1 of its `$color`.
- **Six separate `Image`s at native size.** No resampling: each face keeps its
  own dimensions. `mip_level_count: 1`, `ClampToEdge` on u and v, `Linear`
  mag/min. That reproduces `CLAMPS | CLAMPT | NOMIP | NOLOD` exactly — the GPU
  then *cannot* blend across a face join or drop to a coarser mip, so neither can
  produce a seam even if a face is oriented wrong.
- **⚠️ A half-height face fills the top of the cube face; it does not stretch.**
  The line above about keeping native dimensions is right, but the mapping onto
  the square face is *not* a stretch: a 512 × 256 side holds only the sky above
  the horizon and belongs in the face's top half with its last row repeated
  below. Measured on the mixed-height skies — see the findings. This is why every
  sky face carries `CLAMPT`, and it needs no code beyond running the quad's `v`
  from 0 to `width / height`.
- **A box of six quads, translation-synced to the camera** (not parented —
  parenting would rotate the sky with the view). One `Material` per face,
  `unlit`, with `specialize()` setting `depth_write_enabled: false` — that is the
  usable half of `MATERIAL_VAR_IGNOREZ`
  ([sky_dx9.cpp:34](../../source-sdk-2013/src/materialsystem/stdshaders/sky_dx9.cpp#L34)).
  Source can ignore depth entirely because the engine draws the sky in a known
  slot; Bevy's opaque phase is *binned*, not distance sorted, so there is no draw
  order to lean on. Leaving the depth **test** on while turning **write** off is
  order-independent: drawn first the sky passes a cleared buffer and writes
  nothing, drawn last it fails wherever geometry already sits. That is why the box
  has a real radius scaled to the map rather than sitting at unit distance — the
  depth test is doing real work, so the box has to be genuinely further away than
  the map.
- **The four sides are determined by axes, not by search.** Facing outward with
  image `u` to the right and `v` down, in Source's Z-up axes:

  | face | outward | `u` along | `v` along |
  |---|---|---|---|
  | `rt` | +X | −Y | −Z |
  | `lf` | −X | +Y | −Z |
  | `bk` | +Y | +X | −Z |
  | `ft` | −Y | −X | −Z |

  Directions from `CubeMapFaceIndex_t`'s own comments — `BACK` is +Y, `FRONT` is
  −Y ([public/vtf/vtf.h:135](../../source-sdk-2013/src/public/vtf/vtf.h#L135)).
  Because we author the quads, there is no external cube convention to fight and
  no mirror to discover; the previous attempt's per-face flip was an artifact of
  wgpu's cube face conventions and disappears here.
- **The two caps need one measurement each, and it is well posed.** With the four
  sides pinned above, `up` is fixed by its four edges against them — the pole
  measurement in the findings below already answers this over four skies with a
  ten-fold margin, and needs only re-expressing in the quad frame. `dn` is
  undeterminable from anything TF2 ships; match it to `up` and label that a
  convention.
- Express each face's orientation as a **2 × 3 UV transform**, which is exactly
  Source's `$basetexturetransform`
  ([sky_dx9.cpp:108](../../source-sdk-2013/src/materialsystem/stdshaders/sky_dx9.cpp#L108)).
  A wrong face is then one table row, not a repacked texture.
- Apply a **half-texel inset** if a hairline survives at a quad edge, using
  Valve's own constant: `0.5/w − 0.01/max(w,h)`
  ([sky_hdr_dx9.cpp:223](../../source-sdk-2013/src/materialsystem/stdshaders/sky_hdr_dx9.cpp#L223)).
  Inflate each quad slightly so edges overlap rather than abut, so a
  rasterization crack cannot show through.
- Brightness comes from `bevy_bsp::lightmap_exposure`, reusing M5's unit
  conversion rather than a hand-tuned number.
- Sky *geometry* stays skipped as Phase 1 already does (`SURF_SKY`/`SURF_SKY2D`
  faces are dropped); the box fills those pixels.

**Verify.** Three gates, none of which can pass by construction:

1. **Coverage.** All 70 resolvable skynames load six faces, with the map's own
   pakfile mounted (46 of the 70 need it). `sky_black_01` on `pd_atom_smash` is
   absent from the game entirely — report, do not fail.
2. **Side continuity, on real pixels.** Adjacent sides share a physical edge, so
   `rt`'s shared column must match `bk`'s. Assert that for all four side↔side
   edges, and prove the assert bites by injecting a single-face flip. This is the
   check the original continuity *search* could not be — rejecting one wrong face
   is well posed where ranking 6 × 8 arrangements on smooth gradients is not.
3. **Sun position, independent of continuity.** `light_environment` (231 maps,
   265 instances, every one carrying a `pitch` that overrides `angles[0]`) gives
   the sun direction. The brightest region of the sky should lie there. This
   cross-checks the whole arrangement against map data rather than against the
   sky's own self-consistency, so it cannot be satisfied by a uniformly wrong
   mapping — the failure mode that made every earlier iteration look plausible.

Plus `skydump`, which writes each face at native size to a BMP **labelled with
its slot** and prints the twelve per-edge differences; and `--screenshot` on 10
maps showing a horizon at eye level with no seam.

### M9 findings — delivered

> **The six-quad rewrite landed and all three gates pass on all 233 maps.** What
> follows is what the second attempt measured, including the two places where the
> milestone above was still wrong and the three places where a gate I had just
> written turned out to be unsound. The corrections are the point: every one was
> found by a check firing, not by looking at a screenshot.

**The plan's own numbers, delivered.** `skydump --all-maps`:

| gate | result |
|---|---|
| 1 — coverage | **232 / 233** maps resolve a sky; the one miss is `sky_black_01` on `pd_atom_smash`, absent from every VPK and every pakfile |
| 2 — side continuity | worst side-join spread **7.67** against a limit of 8.00 |
| 3 — sun bearing | coherent bias **−8.8°** against a limit of ±30°, concentration 0.42, 180 maps checked |

Unit tests went 118 → 137. Zero clippy warnings.

#### ⚠️ A half-height face fills the top of the cube face — it does not stretch

The milestone said to stretch each texture across its face. That is wrong, and
`sky_alpinestorm_01` is the sky that proves it: its sides are 1024×512 but its
**`bk` is 1024×1024**, one square face among three half-height ones. Under
stretch, the two joins that involve `bk` scored **15.02 and 14.31** while every
join between two half-height sides scored 0.25 — the shape of the face, not its
position, decided whether the join worked.

Looking at the artwork settles it. `sky_alpinestorm_01bk` (square) has its
horizon across the middle of the image; `sky_alpinestorm_01rt` (half-height) has
its horizon at the very bottom edge, and `sky_badlands_01rt` has no horizon at
all — it is sky all the way down, paling toward the bottom. So a half-height face
holds only the sky **above** the horizon, and it belongs in the top half of the
cube face with its last row repeated below. Fitting rather than stretching:

| | rt–bk | lf–bk | bk–dn |
|---|---|---|---|
| stretch | 15.02 | 14.31 | 30.16 |
| **fit** | **4.45** | **8.74** | **2.83** |

Four independent lines agree:

1. those numbers,
2. the artwork inspection above,
3. it is `CreateDefaultCubemaps`' documented rule for exactly this shape
   ([vbsp/cubemap.cpp](../../source-sdk-2013/src/utils/vbsp/cubemap.cpp)),
4. **`CLAMPT` is otherwise unused.** With a fit of 1 and UVs in `[0, 1]`, clamped
   addressing never engages on any sky face. It is on all 964 of them because the
   quad's `v` is meant to run past 1, and clamping is what repeats the last row.
   The renderer needs no special case: Source set the flag that makes the smear
   happen for free.

Note what this reverses. An earlier attempt dismissed vbsp's fitting as
reflection-only and stretched instead — and *that* dismissal was itself a
correction of an earlier mistake in the opposite direction. The lesson survives
in a sharper form: `CreateDefaultCubemaps` is the wrong function to copy for the
sky's *arrangement* and the right one for its *fitting*. "Is this function about
reflections or about the drawn sky" was the wrong question; the two overlap.

**The visible consequence, stated plainly.** For a sky whose sides are
half-height — most of them — everything below elevation 0° is a repeat of one
row of texels, with a step wherever two faces' rows differ. In normal play the
world fills that half of the view and it is never seen. Fly above the map and
look at the horizon and it is obvious. That band is not a bug and not a seam;
`F6` hides the sky if you need to tell the two apart.

#### The cap rotations, measured

`up` is rotation **3**, on **38 of the 41** skies that carry a verdict, with
tenfold to twentyfold margins:

| sky | rot0 | rot1 | rot2 | **rot3** |
|---|---|---|---|---|
| `sky_day01_01` | 17.02 | 17.65 | 16.92 | **0.89** |
| `sky_premuda_` | 17.23 | 20.89 | 16.93 | **0.53** |
| `sky_hydro_01` | 12.80 | 12.81 | 12.70 | **1.07** |
| `sky_nightsnow_01` | 18.01 | 12.64 | 18.07 | **2.71** |

Of the other 30 skynames, 27 are near-uniform in `up` and score flat across all
four — excluded, not averaged in, because a flat cap votes for whatever the
tie-break reaches. Three dissent with a real margin (`sky_cargo_01` and
`sky_fuji` prefer rot 0, `sky_frankenstorm_02` rot 2) and are recorded as
outliers rather than explained away.

`dn` gets a verdict from **2 of 71** skies, both rot 1 (`overcast01` 0.47 against
4.44/4.56/6.35; `sky_hadal_01` 2.82 against 8.84/8.91/9.32). Two skies is not a
measurement. What promotes it above a coin flip is that rot 1 is also what `up`
implies — `up` at rot 3 is `u = −Y, v = +X` and `dn` at rot 1 is `u = −Y, v = −X`,
the same face reflected across the horizon. Recorded as a convention with two
votes behind it, and excluded from the gate.

#### Three gates, three corrections

Each of these was a check I wrote, believed, and then had to fix because it was
measurably unsound. Worth recording in that order, because each looked fine until
it was pointed at real data.

**Gate 3, first version: the brightest texels.** Took the luminance-weighted
centroid of the brightest 0.05% of texels. On a hazy sky those are not the sun —
they are the bright band along the horizon, which runs the whole way round the
compass and whose centroid points **straight down**. `sky_badlands_01`'s sun came
out at elevation −76°. A mean of directions on a sphere is meaningless unless
they are clustered, and nothing tells you in advance whether they are. Replaced
with a **compass profile**: mean brightness per 5° of yaw. A uniform horizon band
contributes equally to every bin and cancels out of the comparison.

**Gate 3, second version: a v-uniform band is not elevation-uniform.** A cube
face's rows are not lines of constant elevation — at face coordinate `v` the
elevation is `atan(dv / sqrt(1 + du²))`, 45° mid-face and 35° at the corner. So
"the whole upper half" samples a different elevation range at different yaws, and
the horizon gradient leaks in as a spurious 90°-period modulation. On a synthetic
sky uniform in yaw *by construction* that leak reported a prominence of 0.67,
which would have read as a strong sun. Fixed by restricting to a fixed elevation
band computed from the direction, 5° to 30°.

**Gate 3, third version: per-map pass/fail is not a sound test.**
`arena_nucleus` and `arena_offblast_final` use the **same** `sky_goldrush_01`
artwork and reported 72° and 40°. Identical pixels, different answers — so the
disagreement is in the maps, not the sky. Mappers aim `light_environment` by eye
against artwork they did not paint, and on night maps it is a dim fill light
pointed wherever. What a wrong *arrangement* produces is different in kind: a
**coherent** bias, every map's signed error moving together. So the statistic is
the circular mean of the signed errors, which mapper sloppiness cannot move and a
rotation moves entirely.

**Gate 2: the mean hides which half of the edge is wrong.**
`sky_stranded_01`'s `lf`–`ft` join has a mean difference of 39.93 on four square,
same-size faces, while its other three joins score 0.04, 2.48 and 3.09. Sampling
the two columns:

```text
ft right column:  35,57,74  35,57,74  52,78,95 ... 48,73,90    0,0,0  0,0,0 ...
lf left column:   35,57,74  35,57,74  52,79,95 ... 48,73,90  62,92,110 ...
```

The top half agrees to the byte and the bottom half of `ft` is **pure black** —
the sky ships that way. Raising the limit until it passed would have blinded the
gate to real errors of the same size; excluding the sky by name would need
redoing for every such sky. The two causes differ in *kind*: a mirrored or
rotated face is wrong at every point along the edge, an artwork defect is wrong
over part of it and exact over the rest. So the gate reads the **20th percentile**
along the edge, not the mean — and the limit came down from 12 to 8.

#### Brightness: not the lightmap's exposure factor

`sky_brightness()` first returned `lightmap_exposure(1.0)`, about 1000, reasoning
that a sky texel is an albedo and should land where a fully lit white surface
lands. Right about the goal, wrong about the path: Bevy applies `Exposure` inside
`apply_pbr_lighting`, which a lit surface goes through and a custom sky shader
does not. `pl_badwater`'s sky came out flat blown-out white. The correct
comparison is with an **unlit** surface, which also skips that path and emits
`base_color` as-is — so the multiplier is **1.0**, and a sky texel of 0.7 sits
next to a lit wall of albedo 0.35 at full light exactly as it does in Source.

#### Proving the gates bite

Both surviving gates were validated by injecting the bug each targets, on real
data across all 233 maps.

| injection | gate 2 (spread) | gate 3 (bias) |
|---|---|---|
| none | 7.67 — pass | −8.8° — pass |
| `rt`'s `u` mirrored | **20.33 — fail** | — |
| whole sky turned 90° | **7.67 — pass** | **+81.2° — fail** |

The second row is the one that matters. A 90° rotation keeps the side band
perfectly continuous, so gate 2 reports the *identical* 7.67 and is completely
blind to it — while gate 3's error histogram shifts wholesale, its 51-map peak
moving from −15..+15° to +75..+105° with the shape unchanged. That is precisely
the failure mode that made the first attempt's iterations look plausible, and
gate 2 alone could never have caught it.

Unit-test side: mirroring `rt` fires `axes_are_not_mirrored` and
`sides_join_without_a_gap`; `edge_report_catches_a_single_mirrored_face` mirrors
each of the six faces in turn and requires the spread to quadruple;
`cap_scores_recovers_a_planted_rotation` plants each of the four rotations and
requires the measurement to find it — deliberately *not* the tautology of
building a sky with `SkyFace::axes` and asserting the answer is `UP_ROTATION`,
which cannot fail; and `a_localised_artwork_defect_is_not_a_mapping_error` blacks
out half a face and requires the mean to jump while the spread does not.

### M10 — Water

> **Rewritten after the first attempt was reverted.** The milestone this replaced
> said to render water as a translucent surface with `$fogstart`/`$fogend`
> attenuation, a `$bumpmap` scroll and `$reflecttint`. Four of those five details
> were wrong; the corrections are below, each with a line behind it. **M10 now
> comes before M11**, because the sky pass draws its room's water
> (`DF_RENDER_WATER` in `CSkyboxView::Setup`) and `ctf_2fort`'s sky room is 51
> faces of which 22 are water — M11 is unverifiable on most maps until this
> works.

Source ships **two** water tiers and only one of them needs render targets. This
milestone lands the render-target-free tier exactly, then adds the fog.

#### M10a — the cheap tier, exactly as the SDK writes it

`materialsystem/stdshaders/WaterCheap_ps2x.fxc`'s opaque branch, in full:

```c
HALF3 reflectVect = CalcReflectionVectorUnnormalized( worldSpaceNormal, worldSpaceEye );
HALF3 specularLighting = ENV_MAP_SCALE * texCUBE( EnvmapSampler, reflectVect );
specularLighting *= g_ReflectTint;
HALF flDotResult = 1.0f - max( 0.0f, dot( worldSpaceEye, worldSpaceNormal ) );
HALF flFresnelFactor = flDotResult * flDotResult;          // ^2
flFresnelFactor *= flFresnelFactor;                        // ^4
flFresnelFactor *= flDotResult;                            // ^5
...
flAlpha = 1.0f;
specularLighting = lerp( g_WaterFogColor, specularLighting, flFresnelFactor );
```

So: **opaque**, `mix($fogcolor, envmapReflection × $reflecttint, fresnel)`. No
depth prepass, no blending, no render targets, nothing to sort.

- **Reflection comes from M9's six sky faces**, sampled by the reflection vector.
  `$envmap env_cubemap` is on 447 of 464 materials and open-air water reflects
  sky almost exclusively, which is nearly all of TF2's water. Reuse
  `sky::SkyFace::axes` and `FaceAxes::coords` for the direction→face lookup
  rather than writing new cube maths. The real `env_cubemap` (CUBEMAPS lump) is a
  later upgrade, and is what indoor water needs.
- **⚠️ Leave backface culling ON.** This is the single most important line in the
  milestone. The top face (+Z, water material) and the bottom face (−Z,
  `$bottommaterial`) are a *coincident matched pair* emitted by vbsp, and culling
  is what selects the right one. The reverted attempt set `cull_mode: None`, drew
  both, and let the down-facing one win the depth fight — which is why
  `dot(to_eye, n)` measured −1, the fresnel saturated to 1, and every surface came
  out flat opaque white. "Orient the normal toward the viewer" treated the
  symptom of a self-inflicted bug.
- **⚠️ Generate tangents.** `MATERIAL_VAR2_NEEDS_TANGENT_SPACES` is set for water,
  and the basis is derivable exactly from data already parsed —
  `GenerateDispSurfTangentSpaces` (`public/builddisp.cpp:1692`):

  ```c
  TangentT = normalize( tAxis );
  TangentS = normalize( cross( Normal, TangentT ) );
  TangentT = normalize( cross( TangentS, Normal ) );
  if ( dot( planeNormal, cross( sAxis, tAxis ) ) > 0 ) TangentS = -TangentS;
  ```

  `sAxis`/`tAxis` are `texinfo.texture_vecs[0]`/`[1]`. The reverted attempt had
  no tangents at all, so the ripple normal was applied in a bogus basis — and a
  wrong basis still shimmers plausibly, which is why it needs a test rather than
  an eyeball.
- The animated `$normalmap` uploads as a **texture array**, one layer per frame,
  selected with `floor(time × 30) % frames` and **nearest layer** — a blend of
  frame 7 and 8 of a normal map is not a normal.
- `$bottommaterial` needs **no runtime handling**: those faces are already in the
  BSP with their own material.

#### M10b — the fog volume

Only once M10a reads right on its own.

**⚠️ The mechanism is a fog volume, not a term in the water shader.**
`CAboveWaterView::CRefractionView::Draw` renders the refraction pass with the
water's own fog volume enabled and the target cleared to the fog colour:

```c
SetFogVolumeState( GetOuter()->m_fogInfo, true );   // bUseHeightFog
SetClearColorToFogColor();
DrawExecute( ..., VIEW_REFRACTION, ... );
```

So `$fogstart`/`$fogend` **do** shape the above-water look — as attenuation along
the submerged part of the view ray. The water shader's *own* extra term,
`saturate(destAlphaDepth − 0.05)`, is a separate distance fade that needs
`OO_DESTALPHA_DEPTH_RANGE`, a constant that exists only inside the engine.
**Skip that term and say so** rather than inventing a value.

- `DepthPrepass` on the main camera; `WaterMaterial::enable_prepass() -> false`,
  or water measures its own depth and comes out perfectly clear.
- `f = saturate((submergedDepth − $fogstart) / ($fogend − $fogstart))`, guarding a
  non-positive span.
- `AlphaMode::Blend`, expanding Source's two nested lerps so one blended surface
  reproduces them exactly:
  `alpha = 1 − (1−f)(1−fresnel)`,
  `rgb = ($fogcolor·f·(1−fresnel) + skyReflect·fresnel) / alpha`.

**Verify.** `waterdump --census` and `waterdump --all-maps`, with four gates —
each aimed at what a *plausible-looking* implementation gets wrong, because a
flat tinted pane reads as water in a screenshot and counting surfaces proves
nothing:

1. **Coverage** — all 603 surfaces across the 144 maps resolve and parse. A
   `SURF_WARP` face with no VMT still has to draw.
2. **No fog colour reads as white** — the delimiter check. Injecting that bug made
   443 of 464 materials white last time, so this gate is known to bite.
3. **Every `$normalmap` resolves** and reports its frame count; a single frame
   means static water (42 materials — reported, not failed).
4. **Tangents are orthonormal on every water surface**, with the handedness flip
   exercised. A wrong basis is invisible by eye.

Visual: `ctf_2fort`'s moat after M10a (olive, sky reflection at grazing angles,
opaque), then after M10b (the bottom visible through the shallows, fogging to
olive with depth), then `pl_badwater` and `koth_lakeside_final`.

### M11 — 3D skybox

> **Rewritten after the first attempt was reverted.** Its *architecture* was
> right and is kept; what sank it was water (below), one silent query regression,
> and a missing lightmap remap. The milestone this replaced proposed partitioning
> by leaf **cluster** via `crates/bsp/src/cluster.rs` — the wrong key, needing the
> VISIBILITY lump decompressed for no benefit.

**The key is the BSP `area`, and it needs no visibility data at all.**
`CSkyCamera` records the area its own origin falls in
(`game/server/SkyCamera.cpp:108`):

```c
m_skyboxData.area = engine->GetArea( m_skyboxData.origin );
```

and `CSkyboxView::DrawInternal` restricts drawing to that area alone:

```c
tmpbits[m_pSky3dParams->area>>3] |= 1 << (m_pSky3dParams->area&7);
*areabits = tmpbits;
```

Areas exist because of `func_areaportal`, and a sealed skybox room lands in one
of its own. `dleaf_t` carries `area` in a 9-bit field we already parse. Measured
across the 233 maps: the sky area holds a **median 0.6%** of a map's faces
(min 0.1%).

**The geometry is not scaled.** What `scale` divides is the *camera position* —
`origin/scale + sky_origin` — so the room draws at 1:1 and reads as distant
because the camera moves only 1/scale as far. As one translation:
`P' = P − O + C·(1 − 1/s)`, updated per frame. Test it against the engine's
formula, not against itself.

**Two cameras, because the engine uses two passes.** `CSkyboxView::Setup`:

```c
*pClearFlags &= ~( VIEW_CLEAR_COLOR | VIEW_CLEAR_DEPTH | VIEW_CLEAR_STENCIL | VIEW_CLEAR_FULL_TARGET );
*pClearFlags |= VIEW_CLEAR_DEPTH; // Need to clear depth after rendering the skybox
m_DrawFlags = DF_RENDER_UNDERWATER | DF_RENDER_ABOVEWATER | DF_RENDER_WATER;
if( r_skybox.GetBool() ) m_DrawFlags |= DF_DRAWSKYBOX;
```

Sky pass clears colour **and** depth and draws the room, its water and the 2D
sky; the main pass clears **depth only**. Both of those matter: the 2D sky box
belongs on the sky layer, or the main pass's depth clear lets it paint over the
room.

- **Skip the pass when the room holds no scenery.** **82 of 215** sky rooms
  contain only `TOOLS/*` brushes — `cp_badlands`' is 36 `TOOLSBLACK` + 12
  `TOOLSSKYBOX` — and drawing one paints a black band across the sky.
  `engine->IsSkyboxVisibleFromPoint` is engine-side and unavailable, so
  "the area contains non-tool faces" stands in for it. Say that it is a stand-in.
- **⚠️ `LEAF_FLAGS_SKY` is not this test.** It looks perfect — *"this leaf has 3D
  sky in its PVS"* (`public/bspfile.h:791`) — but *vrad* sets it as a lighting
  flood-fill (`utils/vrad/lightmap.cpp:1349-1458`) and it covers ~50% of leaves on
  every outdoor map: badlands 49.2%, 2fort 51.5%, carrier 50.3%.
  `LEAF_FLAGS_SKY2D` is 0 on every TF2 map checked.
- The **18 maps with no `sky_camera`** must render exactly as today; the **4** with
  more than one use the first. **Never assume `scale` is 16** — 210 of 215 use it,
  the rest 18, 32 (twice), 64 and `ctf_helltrain_event`'s 128.
- **Two regressions to avoid, both of which failed silently:**
  1. A second `Camera3d` makes any `Query<…, With<Camera3d>>.single()` return
     `Err`. It broke `sky::follow_camera` and pinned the entire 2D sky box at the
     world origin. Every such query needs `Without<Skybox3dCamera>`.
  2. The room is a *separate* build from `world`, so the `atlas.remap` that ran on
     `world` never touched it, leaving raw per-face lightmap UVs. Remap it too.

**Verify.** `skyroom --census`: 215 cameras / 18 without / 4 multi; median
sky-face share under 5%; the camera in a **non-solid leaf on 215/215**; the camera
**inside its own area's face bounds on 215/215**. Those last two are the gates
that bite — injecting the `-(leaf+1)` off-by-one fires them on 156 and 142 maps,
where the median share only moves 0.6% → 5.2%.

Visual: `cp_carrier` first (1144 scenery faces of clouds and sky cards, and it
already rendered), then `ctf_2fort` and `pl_upward` once M10 makes their
water-dominated rooms visible. Confirm the trimmed-bounds camera no longer has
skybox brushwork in range — excluding the sky room shrinks the level's bounds on
**63 of 215** maps, by up to 45%.

### M12 — Game lump reader (`bsp::gamelump`)

**Its own milestone rather than part of the prop work, because M13's acceptance gate depends on it.** The only place a map states *which models it references* is the `sprp` dictionary, so "parse every model the 233 maps reference" cannot run until this exists. It is also the piece with the highest chance of a silent byte-offset bug, and Phase 1's repeated lesson was to isolate those behind their own CLI rather than bundle them into a milestone whose gate is about something else.

- The game lump directory: `int count` followed by `dgamelump_t[count]`, with the terminating zero-id entry ignored (M1 already found that one).
- **Per-lump LZMA.** `flags & GAMELUMPFLAG_COMPRESSED` selects it; the body is the same 17-byte Valve header `crates/bsp/src/lump.rs` already inflates, so reuse that path rather than writing a second one.
- **⚠️ The compressed span comes from the next entry's `fileofs`, not from `filelen`** — which holds the *uncompressed* size when the flag is set. For the last real entry, the span ends at the end of `LUMP_GAME_LUMP`. Getting this wrong reads far past the data; see the finding above.
- `sprp` v5/v6/v7/v10: dictionary (`char name[128]`) → leaf array (`u16`) → prop array, at the strides in the table above. Version-branch on the *record size*, and refuse an unknown version rather than guessing a layout.
- Expose the prop list and the model-name dictionary; **placement, instancing and materials are M14's**.

**Verify:** `propdump --all-maps` reads all 233 maps — **353 116 props and 8 584 unique model paths** — with 0 lump errors, **0 leftover bytes on every stride**, and 0 out-of-range dictionary indices. The version histogram must come out 193/1/37/1 for v10/v7/v6/v5, and the compressed/uncompressed split 177/56, so a reader that silently handles only one path fails the gate rather than passing it quietly.

### M13 — MDL/VVD/VTX reader (`crates/mdl`)

Formats only — no Bevy, no placement. Build `mdldump` first; the same decision paid for itself in M1 and M7.

- **MDL:** `IDST`, versions **44–48**. Read `numbodyparts`/`bodypartindex`, `numtextures`/`textureindex`, `numcdtextures`/`cdtextureindex`, and the bone array. Assert the header-prefix offsets hold on real files of all five versions.
- **Material resolution:** a texture's name is joined with each `cdtexture` directory in turn and resolved through the VFS. Either part can be absolute or relative and case varies — reuse `vfs::normalise_path` and the case-insensitive VMT lookup from M6.
- **VVD:** version 4. **`numFixups`/`fixupTableStart` must be applied**, or vertices arrive scrambled for any model with more than one LOD. `vertexDataStart`/`tangentDataStart` are byte offsets from the header base.
- **VTX:** `.dx90.vtx`, version 7. Walk `FileHeader_t → BodyPartHeader_t → ModelHeader_t → ModelLODHeader_t → MeshHeader_t → StripGroupHeader_t → StripHeader_t`. Indices are into the strip group's own `Vertex_t` list; `Vertex_t.origMeshVertID` maps back to the mesh's vertex range, then offset by the model's vertex base **after** the VVD fixups. Honour `STRIP_IS_TRILIST` vs `STRIP_IS_TRISTRIP`.
- **Checksums:** `studiohdr_t.checksum`, `vertexFileHeader_t.checksum` and `FileHeader_t.checkSum` must agree — a mismatched trio means stale files and scrambled geometry, so verify rather than trust.
- LOD 0 only for Phase 2; keep the LOD index a parameter.

**Verify:** `mdldump --all-models` parses every MDL/VVD/VTX triple in the VPKs (16 215 sets) with 0 failures; `--all-maps` does the same for every model the 233 maps reference with pakfiles mounted — **the model list comes from M12's `sprp` dictionary**, which is why that milestone comes first. Assert every index is in range and every mesh's triangle count matches its strip headers.

### M14 — Static props placed in the world

- Per prop, from M12's parsed records: `m_Origin`, `m_Angles` (**Source `QAngle` is pitch-yaw-roll**, not roll-pitch-yaw), `m_PropType` into the dictionary, `m_Skin`, `m_FadeMinDist`/`m_FadeMaxDist`, `m_LightingOrigin`, `m_Flags`.
- **Instance, don't duplicate.** 4 383 props on one map over far fewer unique models: one mesh and one material per model, many transforms.
- Prop materials are `VertexLitGeneric` — `$basetexture`, `$bumpmap`, `$phong`. `$bumpmap` maps onto `StandardMaterial::normal_map_texture` for free, and M7 already returns normal maps linear (`is_normal_map` covers `TEXTUREFLAGS_NORMAL`, `SSBUMP` and `UV88`).
- Honour `m_Skin` against the MDL's skin family table, and skip props whose `m_nMinDXLevel`/`m_nMaxDXLevel` exclude a modern renderer.

**Verify:** this gate is about *placement*, since M12 already proved the lump parses. All 233 maps spawn their props with every model resolved to a mesh; prop and world triangle counts appear separately in the F1 HUD. Screenshot `cp_badlands`: barrels, fences and signs sit **on the ground** at the right scale and orientation. Add a coverage-style measure that a count alone cannot pass — the fraction of props whose transform is distinct, or the spread of their positions against the map bounds — because 353 116 props all stacked at the origin satisfies every count-based check.

### M15 — Prop lighting from the baked scene

The milestone that makes props belong rather than float.

- Sample `LUMP_LEAF_AMBIENT_LIGHTING` (and `_HDR`) at each prop's `m_LightingOrigin` via `LUMP_LEAF_AMBIENT_INDEX`'s `dleafambientindex_t { ambientSampleCount, firstAmbientSample }`. Each `dleafambientlighting_t` is a `CompressedLightCube` plus `x`/`y`/`z` fixed-point fractions of the leaf bounds, so samples are *positioned* inside the leaf and must be interpolated by distance, not just taken first.
- **⚠️ `CompressedLightCube` is alignment 1** — the exact trap behind the `DLeafV0` bug found in the Phase 1 review, where the cube sat at byte 30 rather than 32 and both layouts were 56 bytes, so no size assert could see it. `dleafambientlighting_t` is 28 bytes with no padding before the cube; pin it with `offset_of!` as `raw.rs` now does.
- Add the sun from `light_environment` — and **read `pitch`, which overrides `angles[0]`** (present on all 265 instances measured).
- `GlobalAmbientLight::affects_lightmapped_meshes` needs care: M5 turned it off so lightmapped geometry would not be lifted off the floor, but props are *not* lightmapped and do need an ambient term. A real interaction, not a flag flip.

**Verify:** a prop indoors is dark and a prop in sunlight is bright, on the same map, matching the world surface beneath it. A new debug key toggles prop lighting so the difference is visible. Compare `cp_badlands` and `ctf_2fort` against real-game screenshots at matching `cl_showpos` coordinates.

### Stretch, if the above lands cleanly

- **Detail props** — `dprp` v4 on all 233 maps, `dplt`/`dplh` on 126: grass and clutter sprites.
- **Overlays / decals** — the OVERLAYS lump, non-empty on 232 maps.
- **Cubemap reflections** — the CUBEMAPS lump on all 233 maps, with per-map cubemap VTFs packed under `materials/maps/<map>/`. Feeds `$envmap` and would upgrade M10's water.

---

## Verification

**Automated** — the all-maps sweep stays the highest-value test, and each new CLI extends it:

```bash
cargo test --workspace
cargo run -p bevy_bsp --example skydump   -- --all-maps "<tf dir>" # M9:  every skyname, per-edge + sun check
cargo run -p bevy_bsp --example waterdump -- --census   "<tf dir>" # M10: 464 materials and their params
cargo run -p bevy_bsp --example waterdump -- --all-maps "<tf dir>" # M10: 603 surfaces, four gates
cargo run -p bevy_bsp --example skyroom   -- --census   "<tf dir>" # M11: 215 sky_cameras, area shares
cargo run -p bsp --example propdump -- --all-maps   "<tf dir>"   # M12: 353 116 props, all 4 sprp versions
cargo run -p mdl --example mdldump  -- --all-models "<tf dir>"   # M13: 16 215 MDL/VVD/VTX sets
cargo run -p mdl --example mdldump  -- --all-maps   "<tf dir>"   # M13: needs M12 for the model list
cargo run --release -- --map <name> --screenshot out.png         # ~10 maps by default
```

**Two lessons from Phase 1 that apply directly:**

- **"No errors" is not "correct output".** M8's 233-maps-0-failures claim hid three empty frames. Keep measuring screenshot coverage — and extend it, because a map whose prop *count* is right but whose props are all at the origin passes every count-based gate.
- **A check that cannot fail is worse than no check.** The retired terrain-UV gate looked like evidence while being true by construction. Every new invariant here should be validated by deliberately injecting the bug it targets — that is how both displacement checks were confirmed, and how M9's side-continuity assert must be.

**Manual:** fly `cp_badlands`, `ctf_2fort`, `pl_upward` and `koth_lakeside_final`. Confirm sky orientation, water at the right height, props on the ground and lit like their surroundings, and an empty unresolved-material log.

---

## Out of scope — phase 3 and later

- Prop **animation and skinning** — bones are read for the bind pose only.
- **VIS/PVS culling** as an optimisation; M11 builds the leaf partition it would need, but Bevy's frustum culling remains sufficient at these poly counts.
- Physics: `.phy`, `PHYSCOLLIDE`, VPhysics, collision response.
- Entity logic, brush-entity *motion*, netcode, gameplay.
- HDR lighting throughout (`LUMP_LIGHTING_HDR` is read but LDR remains the default).

## Risks

| Risk | Mitigation |
|---|---|
| VVD fixup table skipped → scrambled vertices on multi-LOD models | The most common MDL bug. Verify the MDL/VVD/VTX checksum trio matches, and assert every remapped index is in range across all 16 215 sets. |
| Pre-48 MDL layouts differ where we read → wrong bodyparts or materials | 50% of referenced models are pre-48. Assert the header prefix offsets hold on real files of all five versions before trusting any field. |
| Game lump compressed span read from `filelen` → garbage props | Span comes from the next entry's `fileofs`. 177 maps compressed and 56 not, so the sweep exercises both paths. |
| `sprp` v7 assumed identical to v10 because both are 72 bytes | Check against the SDK's V6/V10 structs; it is 1 map and 536 props, so fail closed with a warning rather than guess. |
| 3D skybox needs leaf partitioning Phase 1 lacks | Build it as `bsp::area` — BSP **areas**, not clusters, and no visibility data needed. Median 0.6% of faces per sky area across 233 maps. |
| Water drawn double-sided → the down-facing `$bottommaterial` face wins the depth fight | Leave backface culling **on**. The two faces are a coincident matched pair emitted by vbsp; culling is what selects between them. Turning it off is what made every water surface flat white on the first attempt. |
| A colour read at the wrong scale → every water surface white | `{...}` is 0–255 and `[...]` is 0–1, per *value* not per key. Gate on "no fog colour reads as white"; the injected bug turns 443 of 464 white. |
| A second `Camera3d` silently breaks `single()` queries | Adding the sky-pass camera made `sky::follow_camera` return `Err` and pinned the 2D sky at the world origin. Every `With<Camera3d>` query needs `Without<Skybox3dCamera>`. |
| A separately-built geometry set misses the lightmap remap | The sky room is its own `build_worldspawn_filtered`, so `atlas.remap` on `world` does not reach it. Remap every set that is spawned. |
| Sky face orientation wrong → seams or a flipped sky | Six clamped 2D faces make cross-face filtering impossible, so a wrong face shows as wrong *content*, not as a hairline. Sides come from the SDK's axes; the two caps are measured. Cross-check against `light_environment`'s sun direction, which no self-consistent-but-wrong mapping can satisfy. |
| Prop counts hurt frame time (4 383 on `pl_patagonia`) | Instance per unique model; honour `m_FadeMaxDist`; measure with the existing F1 frame timing before optimising. |

# Phase 2 — Sky, Water, and Static Props

> Companion to [`PHASE1_MAP_VIEWER.md`](PHASE1_MAP_VIEWER.md). Like that
> document, this one gains a `### Mn findings` section as each milestone lands,
> so the record of what the data actually turned out to be lives next to the
> plan that predicted it.

## Context

Phase 1 delivered a working map viewer: brush geometry, displacement terrain, VTF textures and baked lightmaps, verified across all 233 shipped TF2 maps. What it renders is *architecturally* complete and *visually* half-finished. Three gaps dominate a screenshot:

- **The sky is a flat clear colour.** Every one of the 233 maps names a real sky, and 215 additionally place a `sky_camera` for a 3D skybox.
- **Water is a provisional translucent teal.** 547 material references use the `Water` shader, and none has a `$basetexture` to draw.
- **There are no props.** 353 116 static props across 8 584 unique models — the barrels, fences, pipes, signs and ammo crates that make a Source map read as populated rather than as bare architecture.

**Order: sky and water first, props second.** Sky and water reuse the VTF/VMT/KeyValues work already shipped and need no new binary format, so they land quickly and change screenshots immediately. Props are three new interlocking formats and deserve their own uninterrupted run of milestones.

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
- **⚠️ Sky faces are neither square nor uniform.** `sky_badlands_01`: the four sides are **512 × 256**, `up` and `dn` are **512 × 512**, all `BGR888`, VTF 7.4, flagged `CLAMPS | CLAMPT | NOMIP | NOLOD`. A cubemap needs six equal square faces, so these cannot feed `bevy_light::Skybox` directly. `BGR888` also means CPU decode — no BC passthrough.
- **Sky formats vary.** Several skies (`sky_trainyard_01`, `sky_night_01`, `sky_alpinestorm_01`) are `DXT1`, and some sets are not named `<skyname><face>` uniformly (`sky_dustbowl_01rt.vtf` does not exist under that name). Our `vtf` crate already decodes every format present; a loader must resolve the face texture through the **VMT's `$basetexture`** rather than guessing the filename.
- **The face → cubemap-slot arrangement is derivable, not guessable.** A cube's six faces are one continuous image, so sampling either side of each of the twelve shared edges must agree. Scored over `sky_badlands_01` during planning, the best arrangement came in at **8.94** mean per-channel difference against **47.80** for the worst — a clear signal. Two caveats found: a sky whose `up`/`dn` are near-uniform cannot pin those two rotations, so the search must aggregate several skies; and the search must include mirroring, not just quarter turns.

### Water materials

- **Not identifiable by shader name alone.** `nature/water_movingplane` is **`LightmappedGeneric` with `%compilewater 1`**, `$abovewater 1`, `$bottommaterial`, `$bumpmap dev/water_normal`, `$envmap env_cubemap`. `dev/dev_water3` is shader **`Water`**.
- `$reflecttexture`/`$refracttexture` name **render targets** (`_rt_WaterReflection`), not files, and sit inside `Water_DX90` sub-blocks — which the VMT itself comments are *"used to determine whether to do the reflection"*, i.e. presence is the switch.
- 49 water materials under `materials/nature/` alone; the Phase 1 census counted **547 `Water` references** across all maps.

---

## Architecture

One new crate; the existing four grow. Phase 1's split holds — nothing below `bevy_bsp` gains a Bevy dependency.

```
crates/
  bsp/      + gamelump.rs   (M12: per-lump LZMA, sprp v5/6/7/10, dprp)
            + ambient.rs    (M15: leaf ambient cube lookup)
            + cluster.rs    (M11: leaf/cluster partition, for the 3D skybox)
            examples/propdump.rs
  mdl/      NEW — studiohdr / VVD / VTX, engine-free
            examples/mdldump.rs
  vfs/      (unchanged — pakfile mounting already covers packed models)
  vtf/      (unchanged — cubemap faces already exposed via surface())
  bevy_bsp/ + sky.rs, water.rs, props.rs
```

`crates/mdl` returns plain vertex/index buffers plus material names, exactly as `bsp::geometry` does, so the Bevy layer stays a thin adapter. Static props are rigid, so **no skinning** in Phase 2 — but bone transforms are read, because a model's bind pose is not guaranteed to be identity.

---

## Milestones

Same rule as Phase 1: each ends in something observable, and a failing verification stops the milestone.

### M9 — 2D skybox

- `worldspawn`'s `skyname` → `materials/skybox/<name>{rt,lf,bk,ft,up,dn}.vmt`, shader `sky`. **Resolve the texture through `$basetexture`**, not by filename convention.
- **Resample all six faces to one square edge** (the largest dimension in the set, 512 for TF2) and assemble a 6-layer cubemap `Image` — `depth_or_array_layers: 6` plus `texture_view_descriptor` with `TextureViewDimension::Cube`, as `bevy_image/src/dds.rs:91-101` does — then hand it to `bevy_light::Skybox { image, brightness, rotation }`. Resampling is what the GPU does anyway when stretching a 512 × 256 side across a square face; this does the stretch once, at load.
- **Derive the face arrangement, don't guess it.** Score candidate slot assignments and rotations by edge continuity across all twelve cube edges (method and numbers above), aggregating over several skies so near-uniform caps cannot leave a rotation undetermined. Hard-code the winner as a table with the evidence in its doc comment, and add a test asserting that turning any single face makes the score worse.
- `Skybox::brightness` multiplies by `Exposure` exactly as M5's `lightmap_exposure` did — reuse that reasoning (`bevy_bsp::lightmap_exposure`) rather than hardcoding a number.
- Sky *geometry* stays skipped as Phase 1 already does (`SURF_SKY`/`SURF_SKY2D` faces are dropped); the cubemap fills those pixels.

**Verify:** every distinct `skyname` across the 233 maps assembles six faces with none missing; a `skydump` example writes the cubemap cross to BMP for eyeballing and prints the continuity score; `--screenshot` on 10 maps shows a correctly oriented horizon with no seam.

### M10 — Water

- Classify a surface as water by **any** of: shader `Water`, `%compilewater 1`, or `SURF_WARP` on the face. Measured above — no single test suffices.
- Render as a translucent surface: `$fogcolor` tint with `$fogstart`/`$fogend` depth attenuation, `$bumpmap` scrolling as a ripple normal, `$reflecttint`/`$refracttint` where present. Honour `$abovewater`; note `$bottommaterial` for the under-surface case.
- `$reflecttexture`/`$refracttexture` are render targets, so **planar reflection is deliberately deferred** — a screen-space approximation via Bevy's `ssr` module is the natural follow-up, not this milestone's gate.
- Sort back-to-front through the existing translucent path. Water is flat and large, so self-ordering is the failure to watch for.

**Verify:** all 547 `Water` references resolve; water on `ctf_2fort`, `pl_badwater` and `koth_lakeside_final` reads as water rather than teal; no z-fighting against the pool floor.

### M11 — 3D skybox

- Read the first `sky_camera`: `origin`, `scale` (16 on most maps, up to 128), and its `fog*` keys.
- **The hard part is knowing which geometry is skybox geometry.** The 3D skybox is ordinary worldspawn brushwork in a distant room; the engine separates it by drawing a sky pass with a scaled camera and relying on VIS. Phase 1 has no leaf separation — which is exactly why M3's auto-framing found `cp_badlands` spanning 16 500 units vertically. So M11 must **partition faces by leaf/cluster** (`crates/bsp/src/cluster.rs`), take the cluster containing the `sky_camera` origin as the sky room, and render those faces in a second pass transformed by `(world − sky_origin) / scale` relative to the camera.
- That partition is also the groundwork VIS/PVS culling needs later, so build it properly rather than by bounding box.
- The **18 maps with no `sky_camera`** must render exactly as they do today, and the 4 with multiple must use the first.

**Verify:** `cp_badlands` and `pl_upward` show distant mesas and structures with correct parallax; the trimmed-bounds camera no longer has skybox brushes in range; each of the 5 non-16 scales and the 4 multi-camera maps renders sanely.

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
cargo run -p bevy_bsp --example skydump -- --all-maps "<tf dir>" # M9:  every skyname, continuity scored
cargo run -p bsp --example propdump -- --all-maps   "<tf dir>"   # M12: 353 116 props, all 4 sprp versions
cargo run -p mdl --example mdldump  -- --all-models "<tf dir>"   # M13: 16 215 MDL/VVD/VTX sets
cargo run -p mdl --example mdldump  -- --all-maps   "<tf dir>"   # M13: needs M12 for the model list
cargo run --release -- --map <name> --screenshot out.png         # ~10 maps by default
```

**Two lessons from Phase 1 that apply directly:**

- **"No errors" is not "correct output".** M8's 233-maps-0-failures claim hid three empty frames. Keep measuring screenshot coverage — and extend it, because a map whose prop *count* is right but whose props are all at the origin passes every count-based gate.
- **A check that cannot fail is worse than no check.** The retired terrain-UV gate looked like evidence while being true by construction. Every new invariant here should be validated by deliberately injecting the bug it targets — that is how both displacement checks were confirmed, and how M9's sky arrangement will be.

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
| 3D skybox needs leaf partitioning Phase 1 lacks | Build it as its own reviewable step in M11 — it is also the groundwork VIS culling needs. |
| Sky cubemap face rotations wrong → seams or a flipped sky | Derive by edge continuity over several skies, not by convention; test that turning any single face worsens the score. |
| Prop counts hurt frame time (4 383 on `pl_patagonia`) | Instance per unique model; honour `m_FadeMaxDist`; measure with the existing F1 frame timing before optimising. |

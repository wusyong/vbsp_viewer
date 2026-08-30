use crate::*;
use binrw::BinReaderExt;
use binrw::io::Cursor;
use std::borrow::Cow;

pub struct BspFile<'a> {
    data: &'a [u8],
    directories: Directories,
    header: Header,
}

impl<'a> BspFile<'a> {
    pub fn new(data: &'a [u8]) -> BspResult<Self> {
        let mut cursor = Cursor::new(data);
        let header: Header = cursor.read_le().map_err(|err| match err {
            binrw::Error::BadMagic { .. } => {
                BspError::UnexpectedHeader(data[..4].try_into().expect(
                    "always enough data because otherwise a different binrw error would be hit",
                ))
            }
            error => BspError::MalformedData(error),
        })?;
        let mut directories: Directories = cursor.read_le()?;

        if header.version == BspVersion::Version21 && directories.is_l4d2_lump_order(data.len()) {
            directories.fixup_lumps();
        }

        Ok(BspFile {
            data,
            directories,
            header,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn lump_reader(&self, lump: LumpType) -> BspResult<LumpReader<Cursor<Cow<'_, [u8]>>>> {
        let entry = self.get_lump_entry(lump);
        let data = self.get_lump(entry, lump)?;
        Ok(LumpReader::new(data, lump, entry.version))
    }

    pub fn get_lump_entry(&self, lump: LumpType) -> &LumpEntry {
        &self.directories[lump]
    }

    pub fn get_lump(&self, lump: &LumpEntry, lump_type: LumpType) -> BspResult<Cow<'_, [u8]>> {
        if lump.length == 0 {
            return Err(BspError::InvalidLumpSize {
                lump: lump_type,
                element_size: 0,
                lump_size: lump.length as _,
            });
        }

        let raw_data = self
            .data
            .get(lump.offset as usize..lump.offset as usize + lump.length as usize)
            .ok_or(BspError::LumpOutOfBounds(*lump))?;

        Ok(match lump.ident {
            0 => Cow::Borrowed(raw_data),
            _ => {
                let data = lzma_decompress_with_header(raw_data, lump.ident as usize)?;
                Cow::Owned(data)
            }
        })
    }
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LumpType {
    Entities,
    Planes,
    TextureData,
    Vertices,
    Visibility,
    Nodes,
    TextureInfo,
    Faces,
    Lighting,
    Occlusion,
    Leaves,
    FaceIds,
    Edges,
    SurfaceEdges,
    Models,
    WorldLights,
    LeafFaces,
    LeafBrushes,
    Brushes,
    BrushSides,
    Areas,
    AreaPortals,
    Unused0,
    Unused1,
    Unused2,
    Unused3,
    DisplacementInfo,
    OriginalFaces,
    PhysDisplacement,
    PhysCollide,
    VertNormals,
    VertNormalIndices,
    DisplacementLightMapAlphas,
    DisplacementVertices,
    DisplacementLightMapSamplePositions,
    GameLump,
    LeafWaterData,
    Primitives,
    PrimVertices,
    PrimIndices,
    PakFile,
    ClipPortalVertices,
    CubeMaps,
    TextureDataStringData,
    TextureDataStringTable,
    Overlays,
    LeafMinimumDistanceToWater,
    FaceMacroTextureInfo,
    DisplacementTris,
    PhysicsCollideSurface,
    WaterOverlays,
    LeafAmbientIndexHdr,
    LeafAmbientIndex,
    LightingHdr,
    WorldLightsHdr,
    LeafAmbientLightingHdr,
    LeafAmbientLighting,
    XZipPakFile,
    FacesHdr,
    MapFlags,
    OverlayFades,
    OverlaySystemLevels,
    PhysLevel,
    DisplacementMultiBlend,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct LumpTypeOutOfBounds(u32);

impl TryFrom<u32> for LumpType {
    type Error = LumpTypeOutOfBounds;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        const ENTITIES_U32: u32 = LumpType::Entities as u32;
        const PLANES_U32: u32 = LumpType::Planes as u32;
        const TEXTUREDATA_U32: u32 = LumpType::TextureData as u32;
        const VERTICES_U32: u32 = LumpType::Vertices as u32;
        const VISIBILITY_U32: u32 = LumpType::Visibility as u32;
        const NODES_U32: u32 = LumpType::Nodes as u32;
        const TEXTUREINFO_U32: u32 = LumpType::TextureInfo as u32;
        const FACES_U32: u32 = LumpType::Faces as u32;
        const LIGHTING_U32: u32 = LumpType::Lighting as u32;
        const OCCLUSION_U32: u32 = LumpType::Occlusion as u32;
        const LEAVES_U32: u32 = LumpType::Leaves as u32;
        const FACEIDS_U32: u32 = LumpType::FaceIds as u32;
        const EDGES_U32: u32 = LumpType::Edges as u32;
        const SURFACEEDGES_U32: u32 = LumpType::SurfaceEdges as u32;
        const MODELS_U32: u32 = LumpType::Models as u32;
        const WORLDLIGHTS_U32: u32 = LumpType::WorldLights as u32;
        const LEAFFACES_U32: u32 = LumpType::LeafFaces as u32;
        const LEAFBRUSHES_U32: u32 = LumpType::LeafBrushes as u32;
        const BRUSHES_U32: u32 = LumpType::Brushes as u32;
        const BRUSHSIDES_U32: u32 = LumpType::BrushSides as u32;
        const AREAS_U32: u32 = LumpType::Areas as u32;
        const AREAPORTALS_U32: u32 = LumpType::AreaPortals as u32;
        const UNUSED0_U32: u32 = LumpType::Unused0 as u32;
        const UNUSED1_U32: u32 = LumpType::Unused1 as u32;
        const UNUSED2_U32: u32 = LumpType::Unused2 as u32;
        const UNUSED3_U32: u32 = LumpType::Unused3 as u32;
        const DISPLACEMENTINFO_U32: u32 = LumpType::DisplacementInfo as u32;
        const ORIGINALFACES_U32: u32 = LumpType::OriginalFaces as u32;
        const PHYSDISPLACEMENT_U32: u32 = LumpType::PhysDisplacement as u32;
        const PHYSCOLLIDE_U32: u32 = LumpType::PhysCollide as u32;
        const VERTNORMALS_U32: u32 = LumpType::VertNormals as u32;
        const VERTNORMALINDICES_U32: u32 = LumpType::VertNormalIndices as u32;
        const DISPLACEMENTLIGHTMAPALPHAS_U32: u32 = LumpType::DisplacementLightMapAlphas as u32;
        const DISPLACEMENTVERTICES_U32: u32 = LumpType::DisplacementVertices as u32;
        const DISPLACEMENTLIGHTMAPSAMPLEPOSITIONS_U32: u32 =
            LumpType::DisplacementLightMapSamplePositions as u32;
        const GAMELUMP_U32: u32 = LumpType::GameLump as u32;
        const LEAFWATERDATA_U32: u32 = LumpType::LeafWaterData as u32;
        const PRIMITIVES_U32: u32 = LumpType::Primitives as u32;
        const PRIMVERTICES_U32: u32 = LumpType::PrimVertices as u32;
        const PRIMINDICES_U32: u32 = LumpType::PrimIndices as u32;
        const PAKFILE_U32: u32 = LumpType::PakFile as u32;
        const CLIPPORTALVERTICES_U32: u32 = LumpType::ClipPortalVertices as u32;
        const CUBEMAPS_U32: u32 = LumpType::CubeMaps as u32;
        const TEXTUREDATASTRINGDATA_U32: u32 = LumpType::TextureDataStringData as u32;
        const TEXTUREDATASTRINGTABLE_U32: u32 = LumpType::TextureDataStringTable as u32;
        const OVERLAYS_U32: u32 = LumpType::Overlays as u32;
        const LEAFMINIMUMDISTANCETOWATER_U32: u32 = LumpType::LeafMinimumDistanceToWater as u32;
        const FACEMACROTEXTUREINFO_U32: u32 = LumpType::FaceMacroTextureInfo as u32;
        const DISPLACEMENTTRIS_U32: u32 = LumpType::DisplacementTris as u32;
        const PHYSICSCOLLIDESURFACE_U32: u32 = LumpType::PhysicsCollideSurface as u32;
        const WATEROVERLAYS_U32: u32 = LumpType::WaterOverlays as u32;
        const LEAFAMBIENTINDEXHDR_U32: u32 = LumpType::LeafAmbientIndexHdr as u32;
        const LEAFAMBIENTINDEX_U32: u32 = LumpType::LeafAmbientIndex as u32;
        const LIGHTINGHDR_U32: u32 = LumpType::LightingHdr as u32;
        const WORLDLIGHTSHDR_U32: u32 = LumpType::WorldLightsHdr as u32;
        const LEAFAMBIENTLIGHTINGHDR_U32: u32 = LumpType::LeafAmbientLightingHdr as u32;
        const LEAFAMBIENTLIGHTING_U32: u32 = LumpType::LeafAmbientLighting as u32;
        const XZIPPAKFILE_U32: u32 = LumpType::XZipPakFile as u32;
        const FACESHDR_U32: u32 = LumpType::FacesHdr as u32;
        const MAPFLAGS_U32: u32 = LumpType::MapFlags as u32;
        const OVERLAYFADES_U32: u32 = LumpType::OverlayFades as u32;
        const OVERLAYSYSTEMLEVELS_U32: u32 = LumpType::OverlaySystemLevels as u32;
        const PHYSLEVEL_U32: u32 = LumpType::PhysLevel as u32;
        const DISPLACEMENTMULTIBLEND_U32: u32 = LumpType::DisplacementMultiBlend as u32;

        match value {
            ENTITIES_U32 => Ok(LumpType::Entities),
            PLANES_U32 => Ok(LumpType::Planes),
            TEXTUREDATA_U32 => Ok(LumpType::TextureData),
            VERTICES_U32 => Ok(LumpType::Vertices),
            VISIBILITY_U32 => Ok(LumpType::Visibility),
            NODES_U32 => Ok(LumpType::Nodes),
            TEXTUREINFO_U32 => Ok(LumpType::TextureInfo),
            FACES_U32 => Ok(LumpType::Faces),
            LIGHTING_U32 => Ok(LumpType::Lighting),
            OCCLUSION_U32 => Ok(LumpType::Occlusion),
            LEAVES_U32 => Ok(LumpType::Leaves),
            FACEIDS_U32 => Ok(LumpType::FaceIds),
            EDGES_U32 => Ok(LumpType::Edges),
            SURFACEEDGES_U32 => Ok(LumpType::SurfaceEdges),
            MODELS_U32 => Ok(LumpType::Models),
            WORLDLIGHTS_U32 => Ok(LumpType::WorldLights),
            LEAFFACES_U32 => Ok(LumpType::LeafFaces),
            LEAFBRUSHES_U32 => Ok(LumpType::LeafBrushes),
            BRUSHES_U32 => Ok(LumpType::Brushes),
            BRUSHSIDES_U32 => Ok(LumpType::BrushSides),
            AREAS_U32 => Ok(LumpType::Areas),
            AREAPORTALS_U32 => Ok(LumpType::AreaPortals),
            UNUSED0_U32 => Ok(LumpType::Unused0),
            UNUSED1_U32 => Ok(LumpType::Unused1),
            UNUSED2_U32 => Ok(LumpType::Unused2),
            UNUSED3_U32 => Ok(LumpType::Unused3),
            DISPLACEMENTINFO_U32 => Ok(LumpType::DisplacementInfo),
            ORIGINALFACES_U32 => Ok(LumpType::OriginalFaces),
            PHYSDISPLACEMENT_U32 => Ok(LumpType::PhysDisplacement),
            PHYSCOLLIDE_U32 => Ok(LumpType::PhysCollide),
            VERTNORMALS_U32 => Ok(LumpType::VertNormals),
            VERTNORMALINDICES_U32 => Ok(LumpType::VertNormalIndices),
            DISPLACEMENTLIGHTMAPALPHAS_U32 => Ok(LumpType::DisplacementLightMapAlphas),
            DISPLACEMENTVERTICES_U32 => Ok(LumpType::DisplacementVertices),
            DISPLACEMENTLIGHTMAPSAMPLEPOSITIONS_U32 => {
                Ok(LumpType::DisplacementLightMapSamplePositions)
            }
            GAMELUMP_U32 => Ok(LumpType::GameLump),
            LEAFWATERDATA_U32 => Ok(LumpType::LeafWaterData),
            PRIMITIVES_U32 => Ok(LumpType::Primitives),
            PRIMVERTICES_U32 => Ok(LumpType::PrimVertices),
            PRIMINDICES_U32 => Ok(LumpType::PrimIndices),
            PAKFILE_U32 => Ok(LumpType::PakFile),
            CLIPPORTALVERTICES_U32 => Ok(LumpType::ClipPortalVertices),
            CUBEMAPS_U32 => Ok(LumpType::CubeMaps),
            TEXTUREDATASTRINGDATA_U32 => Ok(LumpType::TextureDataStringData),
            TEXTUREDATASTRINGTABLE_U32 => Ok(LumpType::TextureDataStringTable),
            OVERLAYS_U32 => Ok(LumpType::Overlays),
            LEAFMINIMUMDISTANCETOWATER_U32 => Ok(LumpType::LeafMinimumDistanceToWater),
            FACEMACROTEXTUREINFO_U32 => Ok(LumpType::FaceMacroTextureInfo),
            DISPLACEMENTTRIS_U32 => Ok(LumpType::DisplacementTris),
            PHYSICSCOLLIDESURFACE_U32 => Ok(LumpType::PhysicsCollideSurface),
            WATEROVERLAYS_U32 => Ok(LumpType::WaterOverlays),
            LEAFAMBIENTINDEXHDR_U32 => Ok(LumpType::LeafAmbientIndexHdr),
            LEAFAMBIENTINDEX_U32 => Ok(LumpType::LeafAmbientIndex),
            LIGHTINGHDR_U32 => Ok(LumpType::LightingHdr),
            WORLDLIGHTSHDR_U32 => Ok(LumpType::WorldLightsHdr),
            LEAFAMBIENTLIGHTINGHDR_U32 => Ok(LumpType::LeafAmbientLightingHdr),
            LEAFAMBIENTLIGHTING_U32 => Ok(LumpType::LeafAmbientLighting),
            XZIPPAKFILE_U32 => Ok(LumpType::XZipPakFile),
            FACESHDR_U32 => Ok(LumpType::FacesHdr),
            MAPFLAGS_U32 => Ok(LumpType::MapFlags),
            OVERLAYFADES_U32 => Ok(LumpType::OverlayFades),
            OVERLAYSYSTEMLEVELS_U32 => Ok(LumpType::OverlaySystemLevels),
            PHYSLEVEL_U32 => Ok(LumpType::PhysLevel),
            DISPLACEMENTMULTIBLEND_U32 => Ok(LumpType::DisplacementMultiBlend),
            _ => Err(LumpTypeOutOfBounds(value)),
        }
    }
}

static_assertions::const_assert_eq!(LumpType::DisplacementMultiBlend as usize, 63);

impl LumpType {
    pub fn name(&self) -> &'static str {
        match self {
            LumpType::Entities => "entities",
            LumpType::Planes => "planes",
            LumpType::TextureData => "texturedata",
            LumpType::Vertices => "vertices",
            LumpType::Visibility => "visibility",
            LumpType::Nodes => "nodes",
            LumpType::TextureInfo => "textureinfo",
            LumpType::Faces => "faces",
            LumpType::Lighting => "lighting",
            LumpType::Occlusion => "occlusion",
            LumpType::Leaves => "leaves",
            LumpType::FaceIds => "faceids",
            LumpType::Edges => "edges",
            LumpType::SurfaceEdges => "surfaceedges",
            LumpType::Models => "models",
            LumpType::WorldLights => "worldlights",
            LumpType::LeafFaces => "leaffaces",
            LumpType::LeafBrushes => "leafbrushes",
            LumpType::Brushes => "brushes",
            LumpType::BrushSides => "brushsides",
            LumpType::Areas => "areas",
            LumpType::AreaPortals => "areaportals",
            LumpType::Unused0 => "unused0",
            LumpType::Unused1 => "unused1",
            LumpType::Unused2 => "unused2",
            LumpType::Unused3 => "unused3",
            LumpType::DisplacementInfo => "displacementinfo",
            LumpType::OriginalFaces => "originalfaces",
            LumpType::PhysDisplacement => "physdisplacement",
            LumpType::PhysCollide => "physcollide",
            LumpType::VertNormals => "vertnormals",
            LumpType::VertNormalIndices => "vertnormalindices",
            LumpType::DisplacementLightMapAlphas => "displacementlightmapalphas",
            LumpType::DisplacementVertices => "displacementvertices",
            LumpType::DisplacementLightMapSamplePositions => "displacementlightmapsamplepositions",
            LumpType::GameLump => "gamelump",
            LumpType::LeafWaterData => "leafwaterdata",
            LumpType::Primitives => "primitives",
            LumpType::PrimVertices => "primvertices",
            LumpType::PrimIndices => "primindices",
            LumpType::PakFile => "pakfile",
            LumpType::ClipPortalVertices => "clipportalvertices",
            LumpType::CubeMaps => "cubemaps",
            LumpType::TextureDataStringData => "texturedatastringdata",
            LumpType::TextureDataStringTable => "texturedatastringtable",
            LumpType::Overlays => "overlays",
            LumpType::LeafMinimumDistanceToWater => "leafminimumdistancetowater",
            LumpType::FaceMacroTextureInfo => "facemacrotextureinfo",
            LumpType::DisplacementTris => "displacementtris",
            LumpType::PhysicsCollideSurface => "physicscollidesurface",
            LumpType::WaterOverlays => "wateroverlays",
            LumpType::LeafAmbientIndexHdr => "leafambientindexhdr",
            LumpType::LeafAmbientIndex => "leafambientindex",
            LumpType::LightingHdr => "lightinghdr",
            LumpType::WorldLightsHdr => "worldlightshdr",
            LumpType::LeafAmbientLightingHdr => "leafambientlightinghdr",
            LumpType::LeafAmbientLighting => "leafambientlighting",
            LumpType::XZipPakFile => "xzippakfile",
            LumpType::FacesHdr => "faceshdr",
            LumpType::MapFlags => "mapflags",
            LumpType::OverlayFades => "overlayfades",
            LumpType::OverlaySystemLevels => "overlaysystemlevels",
            LumpType::PhysLevel => "physlevel",
            LumpType::DisplacementMultiBlend => "displacementmultiblend",
        }
    }
}

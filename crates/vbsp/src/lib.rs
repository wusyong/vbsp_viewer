mod bspfile;
pub mod data;
pub mod error;
mod handle;
mod reader;
mod visdata;

use crate::bspfile::LumpType;
pub use crate::data::TextureFlags;
use crate::data::lighting::{
    AmbientLighting, ColorRGBExp32, LeafAmbientIndex, LeafWithAmbientIndex,
};
pub use crate::data::*;
use crate::error::ValidationError;
pub use crate::handle::Handle;
use crate::handle::HandleGeneric;
use binrw::io::Cursor;
use binrw::{BinRead, BinReaderExt};
use bspfile::BspFile;
pub use error::{BspError, StringError};
use glam::Vec3;
use image::{DynamicImage, ImageBuffer, Pixel, Rgb};
use lzma_rs::decompress::{Options, UnpackedSize};
use qbsp::data::LightmapStyle;
use qbsp::mesh::lightmap::{
    ComputeLightmapAtlasError, FaceUvs, LightmapInfo, LightmapPacker, LightmapPackerFaceView,
};
use reader::LumpReader;
use std::cmp::min;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use tracing::warn;

pub use image::math::Rect;

pub use vbsp_common::{AsPropPlacement, deserialize_bool};

pub use qbsp::data::{BspLighting, lighting::RgbLighting};

pub type BspResult<T> = Result<T, BspError>;

// TODO: Store all the allocated objects inline to improve cache usage
/// A parsed bsp file
#[derive(Debug)]
#[non_exhaustive]
pub struct Bsp {
    pub header: Header,
    pub entities: Entities,
    pub textures_data: Vec<TextureData>,
    pub textures_info: Vec<TextureInfo>,
    pub texture_string_tables: Vec<i32>,
    pub texture_string_data: String,
    pub planes: Vec<Plane>,
    pub nodes: Vec<Node>,
    pub leaves: Leaves,
    pub leaf_faces: Vec<LeafFace>,
    pub leaf_brushes: Vec<LeafBrush>,
    pub models: Vec<Model>,
    pub brushes: Vec<Brush>,
    pub brush_sides: Vec<BrushSide>,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub surface_edges: Vec<SurfaceEdge>,
    pub faces: Faces,
    pub original_faces: Faces,
    pub vis_data: VisData,
    pub lightmap_data: Vec<ColorRGBExp32>,
    pub ambient_lighting_indices: Vec<LeafAmbientIndex>,
    pub ambient_lighting_data: Vec<AmbientLighting>,
    pub displacement_lightmap_sample_positions: BTreeMap<usize, DisplacementSamplePosition>,
    pub displacements: Vec<DisplacementInfo>,
    pub displacement_vertices: Vec<DisplacementVertex>,
    pub displacement_triangles: Vec<DisplacementTriangle>,
    vertex_normals: Vec<VertNormal>,
    vertex_normal_indices: Vec<VertNormalIndex>,
    pub static_props: PropStaticGameLump,
    pub pack: Packfile,
}

pub struct LightmapAtlasOutput<P: LightmapPacker> {
    pub rects: HashMap<u32, Rect>,
    pub data: P::Output,
}

pub struct LightmapResult<R> {
    pub data: R,
    pub faces: HashMap<u32, FaceUvs>,
}

impl Bsp {
    pub fn read(data: &[u8]) -> BspResult<Self> {
        let bsp_file = BspFile::new(data)?;

        let entities = bsp_file.lump_reader(LumpType::Entities)?.read_entities()?;
        let textures_data = bsp_file
            .lump_reader(LumpType::TextureData)?
            .read_vec(|r| r.read())?;
        let textures_info = bsp_file
            .lump_reader(LumpType::TextureInfo)?
            .read_vec(|r| r.read())?;
        let texture_string_tables = bsp_file
            .lump_reader(LumpType::TextureDataStringTable)?
            .read_vec(|r| r.read())?;
        let texture_string_data = String::from_utf8(
            bsp_file
                .get_lump(
                    bsp_file.get_lump_entry(LumpType::TextureDataStringData),
                    LumpType::TextureDataStringData,
                )?
                .into_owned(),
        )
        .map_err(|e| BspError::String(StringError::NonUTF8(e.utf8_error())))?;
        let planes = bsp_file
            .lump_reader(LumpType::Planes)?
            .read_vec(|r| r.read())?;
        let mut nodes_lump = bsp_file.lump_reader(LumpType::Nodes)?;
        let nodes = match bsp_file.header().version {
            BspVersion::Version25 => nodes_lump.read_vec(|r| r.read())?,
            BspVersion::Version19 | BspVersion::Version20 | BspVersion::Version21 => {
                let nodes_v0: Vec<NodeV0> = nodes_lump.read_vec(|r| r.read())?;

                nodes_v0.into_iter().map(Into::into).collect()
            }
        };
        let leaves = bsp_file.lump_reader(LumpType::Leaves)?.read_lump_args()?;
        let leaf_faces = bsp_file
            .lump_reader(LumpType::LeafFaces)?
            .read_vec(|r| r.read())?;
        let leaf_brushes = bsp_file
            .lump_reader(LumpType::LeafBrushes)?
            .read_vec(|r| r.read())?;
        let models = bsp_file
            .lump_reader(LumpType::Models)?
            .read_vec(|r| r.read())?;
        let brushes = bsp_file
            .lump_reader(LumpType::Brushes)?
            .read_vec(|r| r.read())?;
        let brush_sides = bsp_file
            .lump_reader(LumpType::BrushSides)?
            .read_vec(|r| r.read())?;
        let vertices = bsp_file
            .lump_reader(LumpType::Vertices)?
            .read_vec(|r| r.read())?;
        let edges = bsp_file
            .lump_reader(LumpType::Edges)?
            .read_vec(|r| r.read())?;
        let surface_edges = bsp_file
            .lump_reader(LumpType::SurfaceEdges)?
            .read_vec(|r| r.read())?;

        let has_hdr_lighting = {
            let lump = bsp_file.get_lump_entry(LumpType::LightingHdr);
            lump.length != 0
        };

        let ambient_lighting_lump = if has_hdr_lighting {
            bsp_file
                .lump_reader(LumpType::LeafAmbientLightingHdr)
                .or_else(|_| bsp_file.lump_reader(LumpType::LeafAmbientLighting))
                .ok()
        } else {
            bsp_file.lump_reader(LumpType::LeafAmbientLighting).ok()
        };

        let ambient_lighting_data = ambient_lighting_lump
            .and_then(|mut lump| lump.read_vec(|r| r.read()).ok())
            .unwrap_or_default();

        let ambient_lighting_index_lump = if has_hdr_lighting {
            bsp_file
                .lump_reader(LumpType::LeafAmbientIndexHdr)
                .or_else(|_| bsp_file.lump_reader(LumpType::LeafAmbientIndex))
                .ok()
        } else {
            bsp_file.lump_reader(LumpType::LeafAmbientIndex).ok()
        };

        let ambient_lighting_indices = ambient_lighting_index_lump
            .and_then(|mut lump| lump.read_vec(|r| r.read()).ok())
            .unwrap_or_default();

        let mut face_lump = if has_hdr_lighting {
            bsp_file
                .lump_reader(LumpType::FacesHdr)
                .or_else(|_| bsp_file.lump_reader(LumpType::Faces))?
        } else {
            bsp_file.lump_reader(LumpType::Faces)?
        };
        let face_lump_version = face_lump.args().version;
        let faces = face_lump.read_lump_args()?;

        let mut original_faces_lump = bsp_file.lump_reader(LumpType::OriginalFaces)?;
        // Portal Revolution BSPs seem to have a bug where the inner face type is version 2, but only
        // the version of the faces lump is bumped.
        let original_faces_lump_args = LumpArgs {
            version: face_lump_version,
            ..original_faces_lump.args()
        };
        let original_faces = original_faces_lump.read_with_args(original_faces_lump_args)?;

        let lighting = {
            let lump = if has_hdr_lighting {
                bsp_file.lump_reader(LumpType::LightingHdr)
            } else {
                bsp_file.lump_reader(LumpType::Lighting)
            };

            lump.ok()
                .and_then(|mut lump| lump.read_vec(|r| r.read()).ok())
                .unwrap_or_default()
        };

        let displacement_lightmap_sample_positions = 'read: {
            let Ok(mut reader) =
                bsp_file.lump_reader(LumpType::DisplacementLightMapSamplePositions)
            else {
                break 'read BTreeMap::new();
            };
            let mut sample_positions = BTreeMap::new();
            let mut offset = 0;
            loop {
                match reader.read_with_args::<RawDisplacementSamplePosition>(offset) {
                    Ok(pos) => {
                        sample_positions.insert(
                            offset,
                            DisplacementSamplePosition {
                                index: pos.index,
                                coords: pos.coords,
                            },
                        );
                        offset = pos.offset;
                    }
                    Err(BspError::Io(std_err))
                        if std_err.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }

            sample_positions
        };

        let vis_data = bsp_file.lump_reader(LumpType::Visibility)?.read_visdata()?;
        let displacements = bsp_file
            .lump_reader(LumpType::DisplacementInfo)?
            .read_vec(|r| r.read())?;
        let displacement_vertices = bsp_file
            .lump_reader(LumpType::DisplacementVertices)?
            .read_vec(|r| r.read())?;
        let displacement_triangles = bsp_file
            .lump_reader(LumpType::DisplacementTris)?
            .read_vec(|r| r.read())?;
        let vertex_normals = bsp_file
            .lump_reader(LumpType::VertNormals)?
            .read_vec(|r| r.read())?;
        let vertex_normal_indices = bsp_file
            .lump_reader(LumpType::VertNormalIndices)?
            .read_vec(|r| r.read())?;
        let game_lumps: GameLumpHeader = bsp_file.lump_reader(LumpType::GameLump)?.read()?;

        // TODO: Debugging code
        // {
        //     let pack_dbg = Packfile::read(bsp_file.lump_reader(LumpType::PakFile)?.into_data())?;

        //     dbg!(
        //         pack_dbg
        //             .into_zip()
        //             .lock()
        //             .unwrap()
        //             .file_names()
        //             .collect::<Vec<_>>()
        //     );
        // }

        let pack = Packfile::read(bsp_file.lump_reader(LumpType::PakFile)?.into_data())?;

        let static_props = game_lumps
            .find(data)
            .ok_or(ValidationError::NoStaticPropLump)??;

        let bsp = Bsp {
            header: bsp_file.header().clone(),
            entities,
            textures_data,
            textures_info,
            texture_string_tables,
            texture_string_data,
            planes,
            nodes,
            leaves,
            leaf_faces,
            leaf_brushes,
            ambient_lighting_indices,
            ambient_lighting_data,
            lightmap_data: lighting,
            models,
            brushes,
            brush_sides,
            vertices,
            edges,
            surface_edges,
            faces,
            original_faces,
            vis_data,
            displacement_lightmap_sample_positions,
            displacements,
            displacement_vertices,
            displacement_triangles,
            vertex_normals,
            vertex_normal_indices,
            static_props,
            pack,
        };
        bsp.validate()?;
        Ok(bsp)
    }

    pub fn lighting_rgb32f(&self) -> Vec<f32> {
        self.lightmap_data
            .iter()
            .flat_map(|pixel| pixel.to_rgb32f().0)
            .collect()
    }

    fn compute_lightmap_atlas_internal<P, Px>(
        &self,
        mut packer: P,
        lighting_buffer: &[Px::Subpixel],
    ) -> Result<LightmapAtlasOutput<P>, ComputeLightmapAtlasError>
    where
        P: LightmapPacker,
        Px: Pixel,
        DynamicImage: From<ImageBuffer<Px, Vec<Px::Subpixel>>>,
    {
        let settings = packer.settings();

        let mut lightmap_rects: HashMap<u32, Rect> = HashMap::new();

        for (face_idx, face) in self.faces().enumerate() {
            let Ok(face_idx_32) = face_idx.try_into() else {
                warn!("Face ID overflowed u32");
                continue;
            };

            if face.light_map_texture_size.element_product() == 0 {
                lightmap_rects.insert(
                    face_idx as u32,
                    Rect {
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
                    },
                );
                continue;
            }

            let lm_info = LightmapInfo {
                lightmap_size: face.light_map_texture_size + 1,
                // This is in elements, not in pixels (our element size is 3 but in the original map it's 4)
                lightmap_offset: face.light_offset / 4,
            };

            let view = LightmapPackerFaceView {
                lm_info: &lm_info,

                lightmap_styles: face.styles.map(LightmapStyle),
                face_idx,
                lighting_buffer,
            };

            let input = packer.read_from_face::<Px>(view);

            let frame = packer.pack::<Px>(view, input)?;

            let offset = frame.min + settings.extrusion;

            lightmap_rects.insert(
                face_idx_32,
                Rect {
                    x: offset.x,
                    y: offset.y,
                    width: face.light_map_texture_size.x,
                    height: face.light_map_texture_size.y,
                },
            );
        }

        let atlas = packer.export();

        Ok(LightmapAtlasOutput {
            rects: lightmap_rects,
            data: atlas,
        })
    }

    /// Packs every face's lightmap together onto a single atlas for GPU rendering.
    pub fn compute_lightmap_atlas_rgb32f<P: LightmapPacker>(
        &self,
        packer: P,
    ) -> Result<LightmapAtlasOutput<P>, ComputeLightmapAtlasError> {
        let lighting = self.lighting_rgb32f();
        self.compute_lightmap_atlas_internal::<P, Rgb<f32>>(packer, &lighting)
    }

    pub fn leaf(&self, n: usize) -> Option<HandleGeneric<'_, LeafWithAmbientIndex<'_>>> {
        let leaf = self.leaves.get(n)?;
        let ambient_index = self.ambient_lighting_indices.get(n);

        Some(HandleGeneric {
            bsp: self,
            data: LeafWithAmbientIndex {
                leaf,
                ambient_index,
            },
        })
    }

    pub fn plane(&self, n: usize) -> Option<Handle<'_, Plane>> {
        self.planes.get(n).map(|plane| Handle::new(self, plane))
    }

    pub fn brush(&self, n: usize) -> Option<Handle<'_, Brush>> {
        self.brushes.get(n).map(|brush| Handle::new(self, brush))
    }

    pub fn face(&self, n: usize) -> Option<Handle<'_, Face>> {
        self.faces.get(n).map(|face| Handle::new(self, face))
    }

    pub fn node(&self, n: usize) -> Option<Handle<'_, Node>> {
        self.nodes.get(n).map(|node| Handle::new(self, node))
    }

    pub fn texture_info(&self, n: usize) -> Option<Handle<'_, TextureInfo>> {
        self.textures_info
            .get(n)
            .map(|texture_info| Handle::new(self, texture_info))
    }

    pub fn displacement(&self, n: usize) -> Option<Handle<'_, DisplacementInfo>> {
        self.displacements
            .get(n)
            .map(|displacement| Handle::new(self, displacement))
    }

    fn displacement_vertex(&self, n: usize) -> Option<Handle<'_, DisplacementVertex>> {
        self.displacement_vertices
            .get(n)
            .map(|vert| Handle::new(self, vert))
    }

    /// Get the root node of the bsp
    pub fn root_node(&self) -> Handle<'_, Node> {
        self.node(0).unwrap()
    }

    /// Get all models stored in the bsp
    pub fn models(&self) -> impl Iterator<Item = Handle<'_, Model>> {
        self.models.iter().map(move |m| Handle::new(self, m))
    }

    /// Get all models stored in the bsp
    pub fn textures(&self) -> impl Iterator<Item = Handle<'_, TextureInfo>> {
        self.textures_info.iter().map(move |m| Handle::new(self, m))
    }

    /// Find a leaf for a specific position
    pub fn leaf_at(&self, point: Vec3) -> Option<Handle<'_, Leaf>> {
        let mut current = self.models().next()?.root()?;

        loop {
            let plane = current.plane();
            let dot = point.dot(plane.normal()) - plane.dist;

            let [front, back] = current.children;

            let next = if dot < 0. { back } else { front };

            if next < 0 {
                return Some(self.leaf((!next) as usize)?.into());
            } else {
                current = self.node(next as usize)?;
            }
        }
    }

    pub fn static_props(&self) -> impl Iterator<Item = Handle<'_, StaticPropLumpEntry>> {
        self.static_props
            .props
            .props
            .iter()
            .map(|lump| Handle::new(self, lump))
    }

    /// Get all faces stored in the bsp
    pub fn faces(&self) -> impl Iterator<Item = Handle<'_, Face>> {
        self.faces.iter().map(move |face| Handle::new(self, face))
    }

    /// Get all original faces stored in the bsp
    pub fn original_faces(&self) -> impl Iterator<Item = Handle<'_, Face>> {
        self.original_faces
            .iter()
            .map(move |face| Handle::new(self, face))
    }

    fn validate(&self) -> BspResult<()> {
        self.validate_indices(
            self.faces
                .iter()
                .filter_map(|face| face.displacement_index()),
            &self.displacements,
            "face",
            "displacement",
        )?;
        self.validate_indices(
            self.displacements
                .iter()
                .map(|displacement| displacement.map_face),
            &self.faces,
            "displacement",
            "face",
        )?;
        self.validate_indices(
            self.faces
                .iter()
                .map(|face| face.first_edge + face.num_edges as i32 - 1),
            &self.surface_edges,
            "face",
            "surface_edge",
        )?;
        self.validate_indices(
            self.surface_edges.iter().map(|edge| edge.edge_index()),
            &self.edges,
            "surface_edge",
            "edge",
        )?;
        self.validate_indices(
            self.edges
                .iter()
                .flat_map(|edge| [edge.start_index, edge.end_index]),
            &self.vertices,
            "edge",
            "vertex",
        )?;
        self.validate_indices(
            self.displacements
                .iter()
                .flat_map(|displacement| &displacement.corner_neighbours)
                .flat_map(|corner| corner.neighbours()),
            &self.displacements,
            "displacement",
            "displacement",
        )?;
        self.validate_indices(
            self.displacements
                .iter()
                .flat_map(|displacement| &displacement.edge_neighbours)
                .flat_map(|edge| edge.iter())
                .map(|sub| sub.neighbour_index),
            &self.displacements,
            "displacement",
            "displacement",
        )?;
        self.validate_indices(
            self.faces.iter().map(|face| face.texture_info),
            &self.textures_info,
            "face",
            "texture_info",
        )?;
        self.validate_indices(
            self.textures_info
                .iter()
                .map(|texture| texture.texture_data_index),
            &self.textures_data,
            "texture_info",
            "texture_data",
        )?;
        self.validate_indices(
            self.textures_data
                .iter()
                .map(|texture| texture.name_string_table_id),
            &self.texture_string_tables,
            "textures_data",
            "texture_string_tables",
        )?;
        self.validate_indices(
            self.texture_string_tables.iter().copied(),
            self.texture_string_data.as_bytes(),
            "texture_string_tables",
            "texture_string_data",
        )?;
        self.validate_indices(
            self.nodes.iter().map(|node| node.plane_index),
            &self.planes,
            "node",
            "plane",
        )?;
        self.validate_indices(
            self.nodes
                .iter()
                .flat_map(|node| node.children)
                .filter(|index| *index >= 0),
            &self.nodes,
            "node",
            "node",
        )?;
        self.validate_indices(
            self.nodes
                .iter()
                .flat_map(|node| node.children)
                .filter_map(|index| (index < 0).then_some(!index)),
            &self.leaves,
            "node",
            "leaf",
        )?;
        self.validate_indices(
            self.static_props().map(|prop| prop.prop_type),
            &self.static_props.dict.name,
            "static props",
            "static prop models",
        )?;
        self.validate_indices(
            self.vertex_normal_indices.iter().map(|i| i.index),
            &self.vertex_normals,
            "vertex normal indices",
            "vertex normals",
        )?;

        if self.nodes.is_empty() {
            return Err(ValidationError::NoRootNode.into());
        }

        for face in &*self.faces {
            if face.displacement_index().is_some() && face.num_edges != 4 {
                return Err(ValidationError::NonSquareDisplacement(face.num_edges).into());
            }
        }

        Ok(())
    }

    fn validate_indices<
        'b,
        Index: TryInto<usize> + Into<i64> + Copy + Ord + Default,
        Indices: Iterator<Item = Index>,
        T: 'b,
    >(
        &'b self,
        indices: Indices,
        list: &[T],
        source: &'static str,
        target: &'static str,
    ) -> BspResult<()> {
        let max = match indices.max() {
            Some(max) => max,
            None => return Ok(()),
        };
        max.try_into()
            .ok()
            .and_then(|index| list.get(index))
            .ok_or_else(|| ValidationError::ReferenceOutOfRange {
                source_: source,
                target,
                index: max.into(),
                size: list.len(),
            })?;
        Ok(())
    }
}

/// LZMA decompression with the header used by source
fn lzma_decompress_with_header(data: &[u8], expected_length: usize) -> Result<Vec<u8>, BspError> {
    // extra 8 byte because game lumps need some padding for reasons
    let mut output: Vec<u8> = Vec::with_capacity(min(expected_length + 8, 8 * 1024 * 1024));
    let mut cursor = Cursor::new(data);
    if b"LZMA" != &<[u8; 4]>::read(&mut cursor)? {
        return Err(BspError::LumpDecompressError(
            lzma_rs::error::Error::LzmaError("Invalid lzma header".into()),
        ));
    }
    let actual_size: u32 = cursor.read_le()?;
    let lzma_size: u32 = cursor.read_le()?;
    if data.len() < lzma_size as usize + 12 {
        return Err(BspError::UnexpectedCompressedLumpSize {
            got: data.len() as u32,
            expected: lzma_size,
        });
    }
    lzma_rs::lzma_decompress_with_options(
        &mut cursor,
        &mut output,
        &Options {
            unpacked_size: UnpackedSize::UseProvided(Some(actual_size as u64)),
            allow_incomplete: false,
            memlimit: None,
        },
    )
    .map_err(BspError::LumpDecompressError)?;
    if output.len() != expected_length {
        return Err(BspError::UnexpectedUncompressedLumpSize {
            got: output.len() as u32,
            expected: expected_length as u32,
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::Bsp;

    #[test]
    fn tf2_file() {
        use std::fs::read;

        let data = read("koth_bagel_rc2a.bsp").unwrap();

        Bsp::read(&data).unwrap();
    }
}

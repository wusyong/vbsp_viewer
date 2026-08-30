use super::Handle;
use crate::data::*;
use glam::{Vec2, Vec3};
use itertools::Either;

impl<'a> Handle<'a, Face> {
    /// Get the texture of the face
    pub fn texture(&self) -> Handle<'a, TextureInfo> {
        self.bsp
            .textures_info
            .get(self.texture_info as usize)
            .map(|texture_info| Handle {
                bsp: self.bsp,
                data: texture_info,
            })
            .unwrap()
    }

    /// Get all vertices making up the face
    pub fn vertices(&self) -> impl Iterator<Item = &'a Vertex> + use<'a> {
        let bsp = self.bsp;
        self.vertex_indices()
            .map(move |vert_index| bsp.vertices.get(vert_index as usize).unwrap())
    }

    /// Get the vertex indexes of all vertices making up the face
    ///
    /// The indexes index into the `vertices` field of the bsp file
    pub fn vertex_indices(&self) -> impl ExactSizeIterator<Item = u16> + use<'a> {
        let bsp = self.bsp;
        (self.data.first_edge..(self.data.first_edge + self.data.num_edges as i32))
            .map(move |surface_edge| bsp.surface_edges.get(surface_edge as usize).unwrap())
            .map(move |surface_edge| {
                bsp.edges
                    .get(surface_edge.edge_index() as usize)
                    .map(|edge| (edge, surface_edge.direction()))
                    .unwrap()
            })
            .map(|(edge, direction)| match direction {
                EdgeDirection::FirstToLast => edge.start_index,
                EdgeDirection::LastToFirst => edge.end_index,
            })
    }

    pub fn triangulate_brush_indices(&self) -> impl Iterator<Item = usize> + use<'a> {
        let mut indices = 0..self.vertex_indices().len();

        let a = indices.next().expect("face with <3 points");
        let mut b = indices.next().expect("face with <3 points");

        indices.flat_map(move |c| {
            let points = [c, b, a];
            b = c;
            points
        })
    }

    pub fn triangulate_indices(&self) -> impl Iterator<Item = usize> + use<'a> {
        self.displacement()
            .map(|displacement| Either::Left(displacement.triangulated_indices()))
            .unwrap_or_else(|| Either::Right(self.triangulate_brush_indices()))
    }

    pub fn edge_direction(&self) -> EdgeDirection {
        self.bsp.surface_edges[self.first_edge as usize].direction()
    }

    /// Check if the face is flagged as visible
    pub fn is_visible(&self) -> bool {
        let texture = self.texture();
        !texture.flags.intersects(
            TextureFlags::SKY2D
                | TextureFlags::SKY
                | TextureFlags::TRIGGER
                | TextureFlags::HINT
                | TextureFlags::SKIP
                | TextureFlags::NODRAW,
        )
    }

    pub fn uvs(&self) -> impl Iterator<Item = Vec2> {
        self.vertex_positions().map(|pos| self.texture().uv(pos))
    }

    /// Triangulate the face
    ///
    /// Triangulation only works for faces that can be turned into a triangle fan trivially
    pub fn triangulate(&self) -> impl Iterator<Item = [Vec3; 3]> + 'a {
        let mut vertices = self.vertices();

        let a = vertices.next().expect("face with <3 points");
        let mut b = vertices.next().expect("face with <3 points");

        vertices.map(move |c| {
            let points = [c.position, b.position, a.position];
            b = c;
            points
        })
    }

    pub fn displacement(&self) -> Option<Handle<'a, DisplacementInfo>> {
        self.bsp
            .displacement(self.displacement_info.try_into().ok()?)
    }

    pub fn lightmap_uvs(&self) -> impl Iterator<Item = Vec2> + use<'a> {
        let (lm_transforms, lm_tex_min, lm_tex_size) = (
            self.texture().lightmap_transforms.clone(),
            self.light_map_texture_min,
            self.light_map_texture_size,
        );

        self.displacement()
            .map(move |displacement| Either::Left(displacement.lightmap_uvs()))
            .unwrap_or_else(move || {
                Either::Right(
                    self.vertices()
                        .map(move |v| v.position)
                        .map(move |v| lm_transforms.project(v))
                        .map(move |uv| uv - lm_tex_min.as_vec2())
                        .map(move |uv| uv / lm_tex_size.as_vec2()),
                )
            })
    }

    /// Get the vertex positions for the face
    pub fn vertex_positions(&self) -> impl Iterator<Item = Vec3> + 'a {
        self.displacement()
            .map(|displacement| displacement.displaced_vertices())
            .map(Either::Left)
            .unwrap_or_else(|| Either::Right(self.vertices().map(|v| v.position)))
    }

    pub fn plane(&self) -> Handle<'a, Plane> {
        self.bsp.plane(self.plane_num as usize).unwrap()
    }

    pub fn normal(&self) -> Vec3 {
        self.plane().normal
    }
}

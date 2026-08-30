use super::Handle;
use crate::data::*;
use arrayvec::ArrayVec;
use glam::{Vec2, Vec3};

impl<'a> Handle<'a, DisplacementInfo> {
    pub fn edge_neighbours(
        &self,
    ) -> impl Iterator<Item = Handle<'a, DisplacementSubNeighbour>> + use<'a> {
        self.data
            .edge_neighbours
            .iter()
            .flat_map(|edge| edge.iter())
            .map(|sub| Handle::new(self.bsp, sub))
    }

    pub fn corner_neighbours(
        &self,
    ) -> impl Iterator<Item = Handle<'a, DisplacementInfo>> + use<'a> {
        self.data
            .corner_neighbours
            .iter()
            .flat_map(|corner| corner.neighbours())
            .filter_map(|id| self.bsp.displacement(id as usize))
    }

    pub fn displacement_vertices(
        &self,
    ) -> impl Iterator<Item = Handle<'a, DisplacementVertex>> + use<'a> {
        (self.displacement_vertex_start..(self.displacement_vertex_start + self.vertex_count()))
            .flat_map(|i| self.bsp.displacement_vertex(i as usize))
    }

    pub fn face(&self) -> Option<Handle<'a, Face>> {
        self.bsp.face(self.map_face as usize)
    }

    /// Get the positions of the corners of the displaced face
    fn corner_positions(&self) -> [Vec3; 4] {
        let face = self.face().unwrap();
        let vertices: [_; 4] = face
            .vertices()
            .collect::<ArrayVec<_, 4>>()
            .as_ref()
            .try_into()
            .unwrap();
        let mut corner_positions: [Vec3; 4] = vertices.map(|v| v.position);

        // find the corner closest to the start position of the displacement
        let start_index = corner_positions
            .iter()
            .copied()
            .map(|point| point - self.start_position)
            .enumerate()
            .min_by(|(_a, a_pos), (_b, b_pos)| {
                a_pos
                    .length_squared()
                    .partial_cmp(&b_pos.length_squared())
                    .unwrap()
            })
            .map(|(i, _pos)| i)
            .unwrap();

        corner_positions.rotate_left(start_index);
        corner_positions
    }

    pub fn lightmap_uvs(&self) -> impl Iterator<Item = Vec2> + use<'a> {
        let steps = 2usize.pow(self.power as u32) + 1;

        let step_scale = 1.0 / (steps as f32 - 1.0);

        (0..steps)
            .flat_map(move |x| (0..steps).map(move |y| (y, x)))
            .map(move |(x, y)| Vec2::new(x as f32, y as f32) * step_scale)
    }

    pub fn subdivided_face(&self) -> impl Iterator<Item = Vec3> + use<'a> {
        let steps = 2usize.pow(self.power as u32) + 1;
        let corner_positions = self.corner_positions();

        let step_scale = 1.0 / (steps as f32 - 1.0);
        let edge_intervals = [
            (corner_positions[1] - corner_positions[0]) * step_scale,
            (corner_positions[2] - corner_positions[3]) * step_scale,
        ];

        (0..steps)
            .flat_map(move |x| (0..steps).map(move |y| (x, y)))
            .map(move |(x, y)| {
                let edge_positions = [
                    corner_positions[0] + edge_intervals[0] * x as f32,
                    corner_positions[3] + edge_intervals[1] * x as f32,
                ];
                let segment_interval = (edge_positions[1] - edge_positions[0]) * step_scale;
                edge_positions[0] + (segment_interval * y as f32)
            })
    }

    pub fn raw_vertices(
        &self,
    ) -> impl Iterator<Item = (Handle<'a, DisplacementVertex>, Vec3)> + use<'a> {
        self.displacement_vertices().zip(self.subdivided_face())
    }

    pub fn displaced_vertices(&self) -> impl Iterator<Item = Vec3> + use<'a> {
        self.raw_vertices()
            .map(move |(displacement, base_pos)| base_pos + displacement.displacement())
    }

    pub fn triangulated_indices(&self) -> impl Iterator<Item = usize> + use<> {
        let steps = 2usize.pow(self.power as u32);

        let index = move |x: usize, y: usize| y * (steps + 1) + x;

        (0..steps)
            .flat_map(move |x| (0..steps).map(move |y| (x, y)))
            .flat_map(move |(x, y)| {
                [
                    index(x, y),
                    index(x + 1, y),
                    index(x, y + 1),
                    index(x + 1, y),
                    index(x + 1, y + 1),
                    index(x, y + 1),
                ]
            })
    }

    pub fn triangulated_displaced_vertices(&self) -> impl Iterator<Item = Vec3> + use<'a> {
        let vertices: Vec<_> = self.displaced_vertices().collect();

        self.triangulated_indices().map(move |i| vertices[i])
    }
}

impl<'a> Handle<'a, DisplacementSubNeighbour> {
    pub fn displacement(&self) -> Option<Handle<'a, DisplacementInfo>> {
        self.bsp.displacement(self.data.neighbour_index as usize)
    }
}

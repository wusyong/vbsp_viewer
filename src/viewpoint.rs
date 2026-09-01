//! Where the camera starts when `--pos` and `--angles` are not given.
//!
//! A player spawn when the map has one, because framing a map from *outside*
//! only works for open maps — see [`default_viewpoint`].
use bevy::prelude::Vec3;
use bsp::geometry::ModelGeometry;

/// Where the camera starts when `--pos`/`--angles` are not given.
pub struct Viewpoint {
    /// Source units.
    pub position: [f32; 3],
    /// Radians, matching [`FlyCamera`].
    pub yaw: f32,
    pub pitch: f32,
}

/// Eye height above a spawn point's origin, in Source units. A TF2 player is
/// 72 units tall and the view sits near the top of that.
pub const EYE_HEIGHT: f32 = 64.0;

/// Where to put the camera when `--pos`/`--angles` are not given.
///
/// **A player spawn is used when the map has one.** Framing a map from outside
/// only works for open maps: rendering all 233 showed three — `ctf_turbine`,
/// `vsh_outburst`, `koth_dryfield` — as essentially empty frames, because they
/// are fully enclosed and from outside there is nothing to see but culled
/// backfaces. A spawn point is inside the playable space by definition, which
/// is exactly the guarantee the bounding box cannot give.
pub fn default_viewpoint(geometry: &ModelGeometry, entities: &[vfs::Entity]) -> Viewpoint {
    if let Some(spawn) = player_spawn(entities) {
        let origin = spawn.origin();
        // Face where the mapper pointed the spawn — `angles` is
        // `pitch yaw roll` in degrees, and a spawn faces out of its room into
        // the level. Aiming at the map centre instead puts the camera's nose
        // against whichever wall happens to lie that way.
        let yaw = match spawn.vector("angles") {
            Some(angles) => angles[1].to_radians(),
            None => {
                let centre = trimmed_centre(geometry);
                (centre[1] - origin[1]).atan2(centre[0] - origin[0])
            }
        };
        return Viewpoint {
            position: [origin[0], origin[1], origin[2] + EYE_HEIGHT],
            yaw: source_yaw_to_bevy(yaw),
            pitch: -0.09,
        };
    }
    frame_from_outside(geometry)
}

/// The first player spawn, preferring a team spawn over a generic start.
pub fn player_spawn(entities: &[vfs::Entity]) -> Option<&vfs::Entity> {
    [
        "info_player_teamspawn",
        "info_player_start",
        "info_observer_point",
    ]
    .iter()
    .find_map(|classname| {
        entities
            .iter()
            .find(|e| e.classname.eq_ignore_ascii_case(classname))
    })
}

/// Source measures yaw about +Z from +X; the fly camera measures it about +Y
/// in Bevy space, where Source's +X is -Z. Hence the quarter turn.
pub fn source_yaw_to_bevy(yaw: f32) -> f32 {
    yaw - std::f32::consts::FRAC_PI_2
}

/// The 2nd and 98th percentile of vertex positions per axis.
///
/// Percentiles rather than the MODELS-lump box, which includes 3D-skybox
/// brushes and bottomless pits.
pub fn trimmed_bounds(geometry: &ModelGeometry) -> Option<([f32; 3], [f32; 3])> {
    let total: usize = geometry.surfaces.iter().map(|s| s.vertices.len()).sum();
    if total == 0 {
        return None;
    }

    let mut lo = [0.0f32; 3];
    let mut hi = [0.0f32; 3];
    let mut values = Vec::with_capacity(total);
    for axis in 0..3 {
        values.clear();
        values.extend(
            geometry
                .surfaces
                .iter()
                .flat_map(|s| s.vertices.iter().map(|v| v.position[axis])),
        );
        let last = values.len() - 1;
        // `total_cmp`: a NaN would make a `partial_cmp` comparator inconsistent.
        let at = |values: &mut Vec<f32>, q: f32| {
            let n = (last as f32 * q) as usize;
            values.select_nth_unstable_by(n, |a, b| a.total_cmp(b));
            values[n]
        };
        lo[axis] = at(&mut values, 0.02);
        hi[axis] = at(&mut values, 0.98);
    }
    Some((lo, hi))
}

/// Middle of the trimmed bounds, or the world origin for empty geometry.
pub fn trimmed_centre(geometry: &ModelGeometry) -> [f32; 3] {
    let Some((lo, hi)) = trimmed_bounds(geometry) else {
        return [0.0; 3];
    };
    [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ]
}

/// Frame the whole map from outside it.
///
/// Two traps make the obvious approaches fail:
///
/// * The MODELS-lump bounding box includes 3D-skybox brushes and pits, so on
///   `cp_badlands` it spans 16 500 units vertically and its centre is
///   thousands of units below the floor. Percentile-trimmed bounds ignore
///   those outliers.
/// * Any point sampled *inside* the level — a median, say — lands in solid
///   geometry. On `ctf_2fort` the horizontal median is inside the central
///   tower, so the view is a wall whatever height it is used at.
///
/// So: take trimmed bounds, then stand back along a diagonal and look at their
/// centre, at a distance scaled to the map's horizontal size.
pub fn frame_from_outside(geometry: &ModelGeometry) -> Viewpoint {
    let Some((lo, hi)) = trimmed_bounds(geometry) else {
        return Viewpoint {
            position: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
        };
    };

    let centre = [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ];
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0);

    // Stand off along a diagonal and above, so the shot reads as an overview
    // rather than an elevation.
    let offset = Vec3::new(-0.55, -0.70, 0.45).normalize();
    let distance = span * 0.85;
    let position = [
        centre[0] + offset.x * distance,
        centre[1] + offset.y * distance,
        centre[2] + offset.z * distance,
    ];

    // Aim back at the centre. Invert the yaw/pitch composition used by
    // `fly_camera`: forward = (-sin y * cos p, sin p, -cos y * cos p).
    let look = bevy_bsp::src_to_bevy_dir([-offset.x, -offset.y, -offset.z]).normalize();
    Viewpoint {
        position,
        yaw: (-look.x).atan2(-look.z),
        pitch: look.y.clamp(-1.0, 1.0).asin(),
    }
}

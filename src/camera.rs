//! Free-fly camera: mouse capture, and WASD/QE movement.
use crate::cli::Args;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

/// Base fly speed and sprint multiplier, in Source units per second. TF2's
/// base run speed is 300 u/s, so this is a little faster than running.
pub const FLY_SPEED_UNITS: f32 = 500.0;
pub const SPRINT_MULTIPLIER: f32 = 6.0;

/// Radians of rotation per pixel of mouse motion.
pub const LOOK_SENSITIVITY: f32 = 0.0022;

#[derive(Component)]
pub struct FlyCamera {
    pub yaw: f32,
    pub pitch: f32,
}

/// Click to capture the mouse, Escape to release.
pub fn grab_cursor(
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    args: Res<Args>,
    mut initialised: Local<bool>,
) {
    let Ok(mut cursor) = cursor.single_mut() else {
        return;
    };

    if !*initialised {
        *initialised = true;
        if args.grab {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
    }

    if mouse.just_pressed(MouseButton::Left) {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

pub fn fly_camera(
    mut camera: Query<(&mut Transform, &mut FlyCamera)>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    let Ok((mut transform, mut fly)) = camera.single_mut() else {
        return;
    };
    let grabbed = cursor
        .single()
        .is_ok_and(|c| c.grab_mode != CursorGrabMode::None);

    if grabbed && motion.delta != Vec2::ZERO {
        fly.yaw -= motion.delta.x * LOOK_SENSITIVITY;
        // Stop just short of vertical: looking exactly along the up axis makes
        // the yaw/pitch basis degenerate.
        const LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
        fly.pitch = (fly.pitch - motion.delta.y * LOOK_SENSITIVITY).clamp(-LIMIT, LIMIT);
    }
    transform.rotation = Quat::from_euler(EulerRot::YXZ, fly.yaw, fly.pitch, 0.0);

    let mut direction = Vec3::ZERO;
    let forward = *transform.forward();
    let right = *transform.right();
    for (key, delta) in [
        (KeyCode::KeyW, forward),
        (KeyCode::KeyS, -forward),
        (KeyCode::KeyD, right),
        (KeyCode::KeyA, -right),
        (KeyCode::KeyE, Vec3::Y),
        (KeyCode::KeyQ, -Vec3::Y),
    ] {
        if keys.pressed(key) {
            direction += delta;
        }
    }

    if direction != Vec3::ZERO {
        let sprint = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            SPRINT_MULTIPLIER
        } else {
            1.0
        };
        let speed = FLY_SPEED_UNITS * bevy_bsp::SOURCE_UNIT_METRES * sprint;
        transform.translation += direction.normalize() * speed * time.delta_secs();
    }
}

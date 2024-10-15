use std::collections::VecDeque;
use std::net::SocketAddr;
use bevy::color::Color;
use bevy::math::Vec2;
use bevy::prelude::{Component, Entity, Resource};
use bevy::utils::HashMap;
use rand_chacha::ChaCha8Rng;
use crate::common::*;

pub const GAP_BETWEEN_PADDLE_AND_FLOOR: f32 = 60.0;
// How close can the paddle get to the wall

// We set the z-value of the ball to 1 (WHEN SPAWNING, NOT HERE) so it renders on top in the case of overlapping sprites.
pub const BALL_STARTING_POSITION: Vec2 = Vec2::new(0.0, -50.0);
pub const PADDLE_Y: f32 = BOTTOM_WALL + GAP_BETWEEN_PADDLE_AND_FLOOR;
pub const GAP_BETWEEN_PADDLE_AND_BRICKS: f32 = 270.0;
pub const GAP_BETWEEN_BRICKS: f32 = 5.0;
// These values are lower bounds, as the number of bricks is computed
pub const GAP_BETWEEN_BRICKS_AND_CEILING: f32 = 20.0;
pub const GAP_BETWEEN_BRICKS_AND_SIDES: f32 = 20.0;
pub const BACKGROUND_COLOR: Color = Color::srgb(0.9, 0.9, 0.9);


pub const LISTEN_ADDRESS: &str = "127.0.0.1:7001";
pub const BUFFER_DELAY_S: f64 = 5.0 * TICK_S + MIN_JITTER_S;
pub const BUFFER_LEN: usize = 1 + ((BUFFER_DELAY_S / TICK_S) as usize);

#[derive(Component)]
pub struct NetConnection {
    pub addr: SocketAddr,
    pub paddle_entity: Entity,
    pub ball_entity: Entity,
    pub last_applied_input: u32,
    pub player_index: u8
}

#[derive(Default)]
pub struct ReceivedPlayerInput {
    pub data: PlayerInputData,
    pub time_received: f32
}

#[derive(Clone, Copy, Default)]
pub enum NetInputState {
    #[default]
    Buffering,
    Playing
}

#[derive(Component, Default)]
pub struct NetInput {
    pub input_state: NetInputState,
    pub inputs: VecDeque<ReceivedPlayerInput>,
    pub pings: VecDeque<PingData> // Not a good place for this, but being fast
}

#[derive(Resource, Default)]
pub struct NetConnections {
    pub addr_to_entity: HashMap<SocketAddr, Entity>,    // Players are removed when they disconnect
    pub next_player_index: u8
}

#[derive(Resource)]
pub struct RandomGen {
    pub r: ChaCha8Rng
}

#[derive(Resource)]
pub struct NetIdGenerator {
    next: u16
}

impl Default for NetIdGenerator {
    fn default() -> Self {
        NetIdGenerator {
            // we want 0 to be special
            next: 1
        }
    }
}

impl NetIdGenerator {
    pub fn next(&mut self) -> NetId {
        let next = self.next;
        self.next += 1;
        NetId(next)
    }
}
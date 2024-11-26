use std::collections::VecDeque;
use std::time;
use bevy::{prelude::*};
use bevy::utils::HashMap;
use bevy::ecs::query::{QueryData, QueryFilter};
use clap::Parser;
use crate::common::*;

pub const INTERP_DELAY_S: f64 = TICK_S + MIN_JITTER_S;

pub struct ClientWorldState {
    pub world: NetWorldStateData,
    pub net_id_to_entity: HashMap<NetId, usize>,
    pub last_applied_input: u32,
    pub local_client_index: u8
}

#[derive(QueryData)]
#[query_data(mutable)]
pub struct BallQuery {
    pub transform: &'static mut Transform,
    pub velocity: &'static mut Velocity,
    pub net_id: &'static NetId,
}

#[derive(QueryFilter)]
pub struct BallFilter {
    w0: With<LocallyPredicted>,
    w1: With<Ball>,
    w2: Without<Paddle>,
    w3: Without<Brick>,
}

#[derive(QueryData)]
#[query_data(mutable)]
pub struct PaddleQuery {
    pub entity: Entity,
    pub transform: &'static mut Transform,
    pub net_id: &'static NetId,
}

#[derive(QueryFilter)]
pub struct PaddleFilter {
    pub w0: With<LocallyPredicted>,
    pub w1: With<Paddle>,
    pub w2: With<Collider>,
    pub w3: Without<Ball>,
    pub w4: Without<Brick>,
}

#[derive(QueryData)]
#[query_data(mutable)]
pub struct RemainingCollidersQuery {
    pub entity: Entity,
    pub transform: &'static Transform,
    pub brick: Option<&'static Brick>,
}

#[derive(QueryFilter)]
pub struct RemainingCollidersFilter {
    pub w0: With<Collider>,
    pub w1: Without<Ball>,
    pub w2: Without<LocallyPredicted>,
}

pub trait LocallyPredictedEntity {
    fn transform(&self) -> &Transform;
    fn rollback_to(&mut self, ws: &ClientWorldState) -> bool;

    fn simulate_forward(&mut self, input: &PlayerInputData);
}



#[derive(Resource, Default)]
pub struct WorldStates {
    pub states: VecDeque<ClientWorldState>,
    pub interp_started: bool,
    pub received_per_sec: VecDeque<f32>,
    pub interpolating_from: Option<u32>,
    pub interpolating_to: Option<u32>
}

#[derive(Resource)]
pub struct PingState {
    pub last_sent_time: f32,
    pub next_ping_id: u32,
    pub ping_id_to_instance: HashMap<u32, time::Instant>,
    pub pongs: Vec<PingData>
}

// Parallel vectors
#[derive(Resource, Default)]
pub struct UnAckedPlayerInputs {
    pub inputs: VecDeque<PlayerInputData>,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    pub ip: String,

    #[arg(long, default_value_t = 7001)]
    pub port: u16,

    #[command(flatten)]
    pub sim_latency: SimLatencyArgs,

    #[arg(long, default_value_t = false)]
    pub disable_client_prediction: bool,
}

#[derive(Resource)]
pub struct NetIdUtils {
    pub net_id_to_entity_id: HashMap<NetId, Entity>,
    pub args: Args
}

#[derive(Component, Default)]
pub struct InterpolatedTransform {
    pub from: Transform,
    pub to: Transform,
}

#[derive(Component)]
pub struct LocallyPredicted;

pub trait SpawNetBundleEx {
    // define a method that we will be able to call on `commands`
    fn spawn_interpolated_transform_bundle<B: Bundle>(
        &mut self, bundle: B
    ) -> Entity;

    fn spawn_predicted_bundle<B: Bundle>(
        &mut self, bundle: B
    ) -> Entity;
}

// implement our trait for Bevy's `Commands`
impl<'w, 's> SpawNetBundleEx for Commands<'w, 's> {
    fn spawn_interpolated_transform_bundle<B: Bundle>(
        &mut self, bundle: B
    ) -> Entity {
        let mut e = self.spawn(bundle);
        e.insert(InterpolatedTransform::default());
        e.id()
    }

    fn spawn_predicted_bundle<B: Bundle>(
        &mut self, bundle: B
    ) -> Entity {
        let mut e = self.spawn(bundle);
        e.insert(LocallyPredicted);
        e.id()
    }
}

pub enum NetBundleType {
    Predicted,
    Interpolated
}

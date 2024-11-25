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

fn apply_velocity(delta_secs: f32, transform: &mut Transform, velocity: &Velocity) {
    transform.translation.x += velocity.x * delta_secs;
    transform.translation.y += velocity.y * delta_secs;
}

pub trait LocallyPredictedEntity {
    fn transform(&self) -> &Transform;
    fn rollback_to(&mut self, ws: &ClientWorldState) -> bool;

    fn simulate_forward(&mut self, input: &PlayerInputData);
}

impl<'w> LocallyPredictedEntity for BallQueryItem<'w> {
    fn transform(&self) -> &Transform {
        &self.transform
    }

    fn rollback_to(&mut self, ws: &ClientWorldState) -> bool {
        if let Some(e) = ws.get_by_net_id(self.net_id) {
            match &e.entity_type {
                NetEntityType::Ball(d) => {
                    self.transform.translation = Vec3::from((d.pos, 1.0));
                    *self.velocity = Velocity(d.velocity);
                    true
                },
                _ => panic!("Unexpected entity type")
            }
        } else {
            false
        }
    }

    fn simulate_forward(&mut self, _input: &PlayerInputData) {
        apply_velocity(
            TICK_S as f32,
            &mut self.transform,
            &self.velocity
        );
    }
}

impl<'w> LocallyPredictedEntity for PaddleQueryItem<'w> {
    fn transform(&self) -> &Transform {
        &self.transform
    }
    fn rollback_to(&mut self, ws: &ClientWorldState) -> bool {
        if let Some(e) = ws.get_by_net_id(self.net_id) {
            match &e.entity_type {
                NetEntityType::Paddle(d) => {
                    self.transform.translation = Vec3::from((d.pos, 0.0));
                    true
                },
                _ => panic!("Unexpected entity type")
            }
        } else {
            false
        }
    }

    fn simulate_forward(&mut self, input: &PlayerInputData) {
        move_paddle(TICK_S as f32, &mut self.transform, input);
    }
}

impl ClientWorldState {
    pub fn new(world: NetWorldStateData, last_applied_input: u32, local_client_index: u8) -> Self {
        let mut net_id_to_entity = HashMap::with_capacity(world.entities.len());
        for (i, net_entity) in world.entities.iter().enumerate() {
            net_id_to_entity.insert(net_entity.net_id, i);
        }

        ClientWorldState {
            world,
            net_id_to_entity,
            last_applied_input,
            local_client_index
        }
    }

    pub fn get_by_net_id(&self, net_id: &NetId) -> Option<&NetEntity> {
        if let Some(index) = self.net_id_to_entity.get(net_id) {
            Some(&self.world.entities[*index])
        } else {
            None
        }
    }
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

pub fn rollback_all<T: LocallyPredictedEntity>(entities: impl Iterator<Item = T>, ws: &ClientWorldState) -> Vec<Transform> {
    let mut original_transforms = Vec::new();
    for mut e in entities {
        original_transforms.push(e.transform().clone());
        e.rollback_to(&ws);
    }
    original_transforms
}

pub fn resimulate_all<T: LocallyPredictedEntity>(entities: impl Iterator<Item = T>, input: &PlayerInputData) {
    for mut e in entities {
        e.simulate_forward(input);
    }
}
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

pub fn spawn_net_bundle<B: Bundle>(commands: &mut Commands, bundle: B, net_type: NetBundleType) -> Entity {
    match net_type {
        NetBundleType::Predicted => {
            commands.spawn_predicted_bundle(bundle)
        },
        NetBundleType::Interpolated => {
            commands.spawn_interpolated_transform_bundle(bundle)
        }
    }
}

pub fn sync_net_ids_if_needed_and_update_score(
    commands: &mut Commands,
    ws: &ClientWorldState,
    net_id_query: &Query<(Entity, &NetId)>,
    net_id_util: &mut ResMut<NetIdUtils>,
    meshes: &mut Assets<Mesh>,
    score: &mut Score,
    materials: &mut Assets<ColorMaterial>
) {
    let mut ws_net_ids: Vec<NetId> = Vec::with_capacity(ws.world.entities.len());

    let paddle_bt = |player_index: NetPlayerIndex, args: &Args| {
        if args.disable_client_prediction == false && player_index.0 == ws.local_client_index {
            NetBundleType::Predicted
        } else {
            NetBundleType::Interpolated
        }
    };

    let ball_bt = |args: &Args| {
        if args.disable_client_prediction == false {
            NetBundleType::Predicted
        } else {
            NetBundleType::Interpolated
        }
    };

    // First, any spawn new entities from this world state
    for net_ent in ws.world.entities.iter() {
        ws_net_ids.push(net_ent.net_id);
        if !net_id_util.net_id_to_entity_id.contains_key(&net_ent.net_id) {
            let entity_id = match &net_ent.entity_type {
                NetEntityType::Paddle(d) => {
                    let bundle = PaddleBundle::new(d.pos, net_ent.net_id, d.player_index);
                    Some(spawn_net_bundle(commands, bundle, paddle_bt(d.player_index, &net_id_util.args)))
                }
                NetEntityType::Brick(d) => {
                    let bundle = BrickBundle::new(d.pos, net_ent.net_id);
                    Some(spawn_net_bundle(commands, bundle, NetBundleType::Interpolated))
                }
                NetEntityType::Ball(d) => {
                    let bundle = BallBundle::new(meshes, materials, d.pos, net_ent.net_id, d.player_index);
                    Some(spawn_net_bundle(commands, bundle, ball_bt(&net_id_util.args)))
                }
                NetEntityType::Score(d) => {
                    // Feels gross to do this here, TODO: find a better spot
                    score.0 = d.score;
                    None
                }
            };

            if let Some(entity_id) = entity_id {
                net_id_util.net_id_to_entity_id.insert(net_ent.net_id, entity_id);
            }
        }
    }

    // Second, remove entities that don't exist in this world state
    for (entity, net_id) in net_id_query.iter() {
        if !ws_net_ids.contains(net_id) {
            commands.entity(entity).despawn();
            net_id_util.net_id_to_entity_id.remove(net_id);
        }
    }
}

fn set_transform_from_net_entity(net_ent: &NetEntity, transform: &mut Transform) {
    match &net_ent.entity_type {
        NetEntityType::Paddle(d) => {
            transform.translation = d.pos.extend(0.0);
        }
        NetEntityType::Brick(d) => {
            transform.translation = d.pos.extend(0.0);
        }
        NetEntityType::Ball(d) => {
            transform.translation = d.pos.extend(1.0);
        }
        NetEntityType::Score(_) => {}
    }
}

pub fn apply_world_state(
    query: &mut Query<&mut InterpolatedTransform>,
    net_id_map: &mut ResMut<NetIdUtils>,
    to_state: &ClientWorldState
) {
    for net_ent in to_state.world.entities.iter() {
        if let Some(entity) = net_id_map.net_id_to_entity_id.get(&net_ent.net_id) {
            if query.contains(*entity) {
                let mut interp_transform = query.get_mut(*entity).unwrap();
                interp_transform.from = interp_transform.to;
                set_transform_from_net_entity(&net_ent, &mut interp_transform.to);
            }
        }
    }
}

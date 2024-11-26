use bevy::{prelude::*};
use bevy::utils::HashMap;
use crate::common::*;
use crate::client_types::*;

pub fn apply_velocity(delta_secs: f32, transform: &mut Transform, velocity: &Velocity) {
    transform.translation.x += velocity.x * delta_secs;
    transform.translation.y += velocity.y * delta_secs;
}

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

pub fn detect_mispredicts(
    ball_query: &Query<BallQuery, BallFilter>,
    local_paddle_query: &Query<PaddleQuery, PaddleFilter>,
    original_paddle_transforms: &Vec<Transform>,
    original_ball_transforms: &Vec<Transform>
) {
    for (i, p) in local_paddle_query.iter().enumerate() {
        if *p.transform != original_paddle_transforms[i] {
            info!("PADDLE MISPREDICT (orginally {:?} now {:?}", original_paddle_transforms[i].translation, p.transform.translation);
        }
    }

    for (i, b) in ball_query.iter().enumerate() {
        if *b.transform != original_ball_transforms[i] {
            info!("BALL MISPREDICT (orginally {:?} now {:?}", original_ball_transforms[i].translation, b.transform.translation);
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

pub fn update_map_and_apply_world_state(
    commands: &mut Commands,
    query: &mut Query<&mut InterpolatedTransform>,
    net_id_query: &Query<(Entity, &NetId)>,
    net_id_map: &mut ResMut<NetIdUtils>,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<ColorMaterial>>,
    score: &mut ResMut<Score>,
    to_state: &ClientWorldState
) {
    sync_net_ids_if_needed_and_update_score(commands, to_state, net_id_query, net_id_map, meshes, score, materials);
    apply_world_state(query, net_id_map, to_state);
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
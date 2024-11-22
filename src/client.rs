use clap::Parser;
mod networking;
mod common;

use std::collections::VecDeque;
use common::*;

use std::time;
use bincode::config;
use bincode::error::DecodeError;
use bevy::{prelude::*};
use bevy::utils::HashMap;
use networking::{ClientPlugin, NetworkEvent, ResSocketAddr, ResUdpSocket, Transport};
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::ecs::query::{QueryData, QueryFilter};
use byteorder::ByteOrder;
use iyes_perf_ui::prelude::*;
use crate::networking::NetworkSystem;

const INTERP_DELAY_S: f64 = TICK_S + MIN_JITTER_S;

struct ClientWorldState {
    world: NetWorldStateData,
    net_id_to_entity: HashMap<NetId, usize>,
    last_applied_input: u32,
    local_client_index: u8
}

#[derive(QueryData)]
#[query_data(mutable)]
struct BallQuery {
    transform: &'static mut Transform,
    velocity: &'static mut Velocity,
    net_id: &'static NetId,
}

#[derive(QueryFilter)]
struct BallFilter {
    w0: With<LocallyPredicted>,
    w1: With<Ball>,
    w2: Without<Paddle>,
    w3: Without<Brick>,
}

#[derive(QueryData)]
#[query_data(mutable)]
struct PaddleQuery {
    entity: Entity,
    transform: &'static mut Transform,
    net_id: &'static NetId,
}

#[derive(QueryFilter)]
struct PaddleFilter {
    w0: With<LocallyPredicted>,
    w1: With<Paddle>,
    w2: With<Collider>,
    w3: Without<Ball>,
    w4: Without<Brick>,
}

#[derive(QueryData)]
#[query_data(mutable)]
struct RemainingCollidersQuery {
    entity: Entity,
    transform: &'static Transform,
    brick: Option<&'static Brick>,
}

#[derive(QueryFilter)]
struct RemainingCollidersFilter {
    w0: With<Collider>,
    w1: Without<Ball>,
    w2: Without<LocallyPredicted>,
}

trait LocallyPredictedEntity {
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
    fn new(world: NetWorldStateData, last_applied_input: u32, local_client_index: u8) -> Self {
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

    fn get_by_net_id(&self, net_id: &NetId) -> Option<&NetEntity> {
        if let Some(index) = self.net_id_to_entity.get(net_id) {
            Some(&self.world.entities[*index])
        } else {
            None
        }
    }
}

#[derive(Resource, Default)]
struct WorldStates {
    states: VecDeque<ClientWorldState>,
    interp_started: bool,
    received_per_sec: VecDeque<f32>,
    interpolating_from: Option<u32>,
    interpolating_to: Option<u32>
}

#[derive(Resource)]
struct PingState {
    last_sent_time: f32,
    next_ping_id: u32,
    ping_id_to_instance: HashMap<u32, time::Instant>,
    pongs: Vec<PingData>
}

// Parallel vectors
#[derive(Resource, Default)]
struct UnAckedPlayerInputs {
    inputs: VecDeque<PlayerInputData>,
}

#[derive(Resource)]
struct NetIdUtils {
    net_id_to_entity_id: HashMap<NetId, Entity>,
    args: Args
}

#[derive(Component, Default)]
struct InterpolatedTransform {
    from: Transform,
    to: Transform,
}

#[derive(Component)]
struct LocallyPredicted;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    ip: String,

    #[arg(long, default_value_t = 7001)]
    port: u16,

    #[command(flatten)]
    sim_latency: SimLatencyArgs,

    #[arg(long, default_value_t = false)]
    disable_client_prediction: bool,
}

fn main() {
    let args = Args::parse();
    let remote_addr = format!("{}:{}", args.ip, args.port).parse().expect("could not parse addr");
    let socket = ResUdpSocket::new_client(remote_addr);
    //let addr = socket.0.local_addr().unwrap();
    //println!("local socket addr: {}", addr);
    let res_addr = ResSocketAddr(remote_addr);
    let sim_settings = args.sim_latency.into();
    let net_utils = NetIdUtils {
        net_id_to_entity_id: HashMap::new(),
        args
    };

    App::new()
        .insert_resource(bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::Continuous,
        })
        .insert_resource(res_addr)
        .insert_resource(socket)
        .insert_resource(net_utils)
        .insert_resource(Time::<Fixed>::from_hz(TICK_RATE_HZ))
        .insert_resource(WorldStates::default())
        .insert_resource(Score(0))
        .insert_resource(PingState{
            last_sent_time: 0.0,
            next_ping_id: 1,
            ping_id_to_instance: HashMap::default(),
            pongs: Vec::default()
        })
        .insert_resource(FixedTickWorldResource::default())
        .insert_resource(UnAckedPlayerInputs::default())
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(PerfUiPlugin)
        .add_plugins(DefaultPlugins)
        .add_plugins(ClientPlugin{sim_settings, no_systems: true})
        .add_event::<networking::events::NetworkEvent>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                //connection_handler,
                interpolate_frame,
            )
        )
        .add_systems (
            FixedUpdate,
            (
                common::start_tick,
                networking::systems::client_recv_packet_system.in_set(NetworkSystem::Receive),
                send_input,
                connection_handler,
                reconcile_and_update_predictions,
                ping_server,
                tick_simulation,
                update_scoreboard,
                networking::systems::auto_heartbeat_system.in_set(networking::ClientSystem::Heartbeat),
                networking::systems::send_packet_system.in_set(NetworkSystem::Send),
                common::end_tick
            ).chain()
        )
        .run();
}

fn connection_handler(
    mut events: EventReader<NetworkEvent>,
    mut world_states: ResMut<WorldStates>,
    mut ping_state: ResMut<PingState>,
    //mut unacked_inputs: ResMut<UnAckedPlayerInputs>,
    time: Res<Time<Real>>,
) {
    //let mut recv_count = 0;
    for event in events.read() {
        match event {
            NetworkEvent::Message(handle, msg, _) => {
                let config = config::standard();
                if msg.len() < HEADER_LEN + 1 {
                    warn!("Packet too small, ignoring");
                    continue;
                }

                let msg_slice = msg.as_ref();

                let header_tag = byteorder::NetworkEndian::read_u32(msg_slice);
                if header_tag != WORLD_PACKET_HEADER_TAG {
                    warn!("Invalid tag, ignoring");
                    continue;
                }

                // This is gross but I wanted to stay simple, there is no framing, every message has all needed data
                // This allows the server to serialize the world state once
                let last_applied_input = byteorder::NetworkEndian::read_u32(&msg_slice[size_of::<u32>()..]);
                let local_client_index = msg_slice[size_of::<u32>() * 2];

                let msg_slice = &msg.as_ref()[HEADER_LEN..];
                type ServerToClientResult = Result<(ServerToClientPacket, usize), DecodeError>;
                let decode_result: ServerToClientResult = bincode::serde::decode_from_slice(msg_slice, config);
                match decode_result {
                    Ok((packet, _)) => {
                        match packet {
                            ServerToClientPacket::WorldState(ws) => {
                                world_states.states.push_back(ClientWorldState::new(ws, last_applied_input, local_client_index));
                                world_states.received_per_sec.push_back(time.elapsed_seconds())
                            },
                            ServerToClientPacket::Pong(pd) => {
                                ping_state.pongs.push(pd);
                            }
                        }
                    }
                    Err(err) => {
                        warn!("Error parsing message from {}: {:?} {:?}", handle, msg_slice, err);
                    }
                }
            }
            NetworkEvent::SendError(handle, err, msg) => {
                error!(
                    "NetworkEvent::SendError from {} (payload [{:?}]): {:?}",
                    handle, msg.payload, err
                );
            }
            NetworkEvent::RecvError(err) => {
                error!("NetworkEvent::RecvError: {:?}", err);
            }
            // discard irrelevant events
            _ => {}
        }
    }
    /*if recv_count > 0 {
        if world_states.received_per_sec.len() > 1 {
            let recent = world_states.received_per_sec.back().unwrap();
            let prev = world_states.received_per_sec[world_states.received_per_sec.len() - 2];
            info!("{} event recvd this frame ({} ms since prev)", recv_count, (recent - prev) * 1000.0);
        } else {
            info!("{} event recvd this frame", recv_count);
        }
    }*/
}

fn apply_velocity(delta_secs: f32, transform: &mut Transform, velocity: &Velocity) {
    transform.translation.x += velocity.x * delta_secs;
    transform.translation.y += velocity.y * delta_secs;
}

fn rollback_all<T: LocallyPredictedEntity>(entities: impl Iterator<Item = T>, ws: &ClientWorldState) -> Vec<Transform> {
    let mut original_transforms = Vec::new();
    for mut e in entities {
        original_transforms.push(e.transform().clone());
        e.rollback_to(&ws);
    }
    original_transforms
}

fn resimulate_all<T: LocallyPredictedEntity>(entities: impl Iterator<Item = T>, input: &PlayerInputData) {
    for mut e in entities {
        e.simulate_forward(input);
    }
}

fn reconcile_and_update_predictions(
    mut ball_query: Query<BallQuery, BallFilter>,
    mut local_paddle_query: Query<PaddleQuery, PaddleFilter>,
    remaining_colliders: Query<RemainingCollidersQuery, RemainingCollidersFilter>,
    mut unacked_inputs: ResMut<UnAckedPlayerInputs>,
    mut score: ResMut<Score>,
    world_states: Res<WorldStates>,
) {
    if world_states.states.is_empty() {
        return;
    }

    // Clear previous inputs
    let most_recent_state = world_states.states.back().unwrap();
    let most_recent_input = most_recent_state.last_applied_input;
    unacked_inputs.inputs.retain(|input| input.sequence > most_recent_input);

    let inputs = &unacked_inputs.inputs;
    if inputs.is_empty() {
        info!("NO UNACKED, RETURNING");
        return;
    }

    // First, rollback and resimulate from the most recent world state to now
    let original_paddle_transforms = rollback_all(local_paddle_query.iter_mut(), &most_recent_state);
    let original_ball_transforms = rollback_all(ball_query.iter_mut(), &most_recent_state);

    let mut entities_to_ignore = Vec::new();
    let last_idx = inputs.len() - 1;

    // The last input hasn't been applied yet. Don't apply it here so we can detect mispredicts
    for (i, input) in unacked_inputs.inputs.iter().enumerate() {
        if i == last_idx {
            // Print mispredicts. The last input in the list hasn't been predicted yet and is
            // for this frame. So to detect mispredicts we need to compare to the state BEFORE
            // that last input has been applied
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
        // Forward predict paddles and balls
        resimulate_all(local_paddle_query.iter_mut(), input);
        resimulate_all(ball_query.iter_mut(), input);

        // Perform collision detection
        for mut b in ball_query.iter_mut() {
            let colliders = local_paddle_query
                .iter()
                .map(|p| (p.entity, p.transform, None))
                .chain(
                    remaining_colliders
                        .iter()
                        .map(|r| (r.entity, r.transform, r.brick))
                );
            check_single_ball_collision(&mut score, colliders, &b.transform, &mut b.velocity, &mut entities_to_ignore);
        }
    }
}

fn interpolate_frame(
    mut query: Query<(&mut Transform, &InterpolatedTransform)>,
    time: Res<Time<Fixed>>,
) {
    for (mut transform, interp) in &mut query {
        let alpha= time.overstep_fraction();
        transform.translation = interp.from.translation.lerp(interp.to.translation, alpha);
    }
}

trait SpawNetBundleEx {
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

enum NetBundleType {
    Predicted,
    Interpolated
}

fn spawn_net_bundle<B: Bundle>(commands: &mut Commands, bundle: B, net_type: NetBundleType) -> Entity {
    match net_type {
        NetBundleType::Predicted => {
            commands.spawn_predicted_bundle(bundle)
        },
        NetBundleType::Interpolated => {
            commands.spawn_interpolated_transform_bundle(bundle)
        }
    }
}

fn sync_net_ids_if_needed_and_update_score(
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

fn setup(
    mut commands: Commands,
) {
    // Camera
    commands.spawn(Camera2dBundle::default());

    // Scoreboard
    commands.spawn(ScoreboardUiBundle::new());

    // Walls
    commands.spawn(WallBundle::new(WallLocation::Left));
    commands.spawn(WallBundle::new(WallLocation::Right));
    commands.spawn(WallBundle::new(WallLocation::Bottom));
    commands.spawn(WallBundle::new(WallLocation::Top));

    commands.spawn((
        PerfUiRoot {
            display_labels: false,
            layout_horizontal: true,
            ..default()
        },
        PerfUiEntryFPSWorst::default(),
        PerfUiEntryFPS::default(),
    ));
}

fn apply_world_state(
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

fn send_input (
    keyboard_input: Res<ButtonInput<KeyCode>>,
    remote_addr: Res<ResSocketAddr>,
    mut transport: ResMut<Transport>,
    world_states: ResMut<WorldStates>,
    fixed_state: ResMut<FixedTickWorldResource>,
    mut unacked_inputs: ResMut<UnAckedPlayerInputs>
) {
    if world_states.interpolating_from.is_none() {
        return;
    }

    let mut input = PlayerInputData::default();
    input.sequence = fixed_state.frame_counter;
    input.simulating_frame = world_states.interpolating_from.unwrap();

    if keyboard_input.pressed(KeyCode::ArrowLeft) {
        input.key_mask |= 1 << (NetKey::Left as u8);
    }

    if keyboard_input.pressed(KeyCode::ArrowRight) {
        input.key_mask |= 1 << (NetKey::Right as u8);
    }

    unacked_inputs.inputs.push_back(input.clone());

    let packet = ClientToServerPacket::Input(input);
    let mut buf = [0; networking::ETHERNET_MTU];
    let num_bytes = bincode::serde::encode_into_slice(packet, &mut buf, config::standard()).unwrap();
    transport.send(remote_addr.0, &buf[..num_bytes]);
}

fn ping_server(
    remote_addr: Res<ResSocketAddr>,
    mut state: ResMut<PingState>,
    mut transport: ResMut<Transport>,
    fixed_state: Res<FixedTickWorldResource>,
    time: Res<Time<Real>>,
) {
    let now = time.elapsed_seconds();

    // Send ping every 250ms
    if now - state.last_sent_time < 0.25 {
        return;
    }

    state.last_sent_time = now;
    let ping_id = state.next_ping_id;
    let packet = ClientToServerPacket::Ping(PingData { /*client_time: now,*/ ping_id });
    state.ping_id_to_instance.insert(ping_id, time::Instant::now());
    state.next_ping_id += 1;

    let mut buf = [0; networking::ETHERNET_MTU];
    let num_bytes = bincode::serde::encode_into_slice(packet, &mut buf, config::standard()).unwrap();
    transport.send(remote_addr.0, &buf[..num_bytes]);

    debug!("({})  {} at {:?}", fixed_state.frame_counter, ping_id, time::Instant::now());
}

fn tick_simulation(
    mut commands: Commands,
    mut world_states: ResMut<WorldStates>,
    mut query: Query<&mut InterpolatedTransform>,
    net_id_query: Query<(Entity, &NetId)>,
    mut net_id_map: ResMut<NetIdUtils>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut score: ResMut<Score>,
    mut ping_state: ResMut<PingState>,
    //fixed_state: Res<FixedTickWorldResource>,
    time: Res<Time<Real>>,
) {
    // Clear old entries from our stats
    let now = time.elapsed_seconds();
    while !world_states.received_per_sec.is_empty() {
        let entry = *world_states.received_per_sec.front().unwrap();
        if now > entry && now - entry > 1.0 {
            world_states.received_per_sec.pop_front();
        }  else {
            break;
        }
    }

    /*for pong in ping_state.pongs.clone().iter() {
        let instant = ping_state.ping_id_to_instance.remove(&pong.ping_id).unwrap();
        info!("({}) {} ms raw pong for ping {}", fixed_state.frame_counter, instant.elapsed().as_millis(), pong.ping_id);
    }*/
    ping_state.pongs.clear();

    //if !world_states.received_per_sec.is_empty() {
        //let mut avg_interval: f32 = world_states.received_per_sec.iter().tuple_windows().map(|(&p,&c)| c - p).sum();
        //avg_interval /= world_states.received_per_sec.len() as f32;
        //let intervals: Vec<f32> = world_states.received_per_sec.iter().tuple_windows().map(|(&p,&c)| c - p).collect();
        //warn!("{} PPS, INTERVALS {:?}", world_states.received_per_sec.len(), intervals);
    //}

    if world_states.states.len() < 2 {
        debug!("STARVED {}!", world_states.states.len());
        return;
    }

    // advance state to interp
    let mut bootstrap_first_state = false;
    if world_states.interp_started {
        world_states.interpolating_from = Some(world_states.states.front().unwrap().world.frame);
        world_states.states.remove(0);
    } else {
        world_states.interp_started = true;
        bootstrap_first_state = true;
    }

    let expected_buffer = 2 + f64::round(INTERP_DELAY_S / TICK_S) as usize;

    if world_states.received_per_sec.len() > 0 &&
        now - world_states.received_per_sec.front().unwrap() < INTERP_DELAY_S as f32 {
        warn!("STARVED INTERP {} vs {}!", now - world_states.received_per_sec.back().unwrap(), INTERP_DELAY_S);
        return;
    } else if world_states.states.len() > expected_buffer && world_states.interp_started {
        let drain_len = world_states.states.len() - expected_buffer;
        world_states.states.drain(0..drain_len);
        warn!("Skipped {} states to stay close to the edge buf {}!", drain_len, world_states.states.len());
    }

    //warn!("BUF {}!", world_states.states.len());

    if (bootstrap_first_state && world_states.states.len() < 2) ||
        world_states.states.is_empty() {
        return;
    }

    if bootstrap_first_state {
        let from_state = &world_states.states[0];
        update_map_and_apply_world_state(
            &mut commands,
            &mut query,
            &net_id_query,
            &mut net_id_map,
            &mut meshes,
            &mut materials,
            &mut score,
            from_state);
        world_states.interpolating_from = Some(from_state.world.frame);

        let to_state = &world_states.states[1];
        update_map_and_apply_world_state(
            &mut commands,
            &mut query,
            &net_id_query,
            &mut net_id_map,
            &mut meshes,
            &mut materials,
            &mut score,
            to_state);
        world_states.interpolating_to = Some(to_state.world.frame);
    } else {
        let to_state = &world_states.states[0];
        update_map_and_apply_world_state(
            &mut commands,
            &mut query,
            &net_id_query,
            &mut net_id_map,
            &mut meshes,
            &mut materials,
            &mut score,
            to_state);
        world_states.interpolating_to = Some(to_state.world.frame);
    }

    //info!("{} us", (Instant::now() - now_inst).as_micros());
}

fn update_map_and_apply_world_state(
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
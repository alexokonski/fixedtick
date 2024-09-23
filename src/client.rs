use clap::Parser;
mod networking;
mod common;

use std::collections::VecDeque;
use common::*;

use std::net::{UdpSocket};
use std::time;
use bincode::config;
use bincode::error::DecodeError;
use bevy::{prelude::*};
use bevy::utils::HashMap;
use networking::{ClientPlugin, NetworkEvent, ResSocketAddr, ResUdpSocket, Transport};
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use iyes_perf_ui::prelude::*;
use crate::networking::{transport, NetworkSystem};
//use itertools::Itertools;
//use std::time::{Instant};

const MIN_JITTER_S: f64 = (1.0 / 1000.0) * 6.0;
const TICK_S: f64 = 1.0 / TICK_RATE_HZ;
const INTERP_DELAY_S: f64 = TICK_S + MIN_JITTER_S;

#[derive(Resource, Default)]
struct WorldStates {
    states: Vec<WorldStateData>,
    interp_started: bool,
    received_per_sec: VecDeque<f32>,
}

#[derive(Resource)]
struct PingState {
    last_sent_time: f32,
    next_ping_id: u32,
    ping_id_to_instance: HashMap<u32, time::Instant>,
    pongs: Vec<PingData>
}

#[derive(Resource)]
struct PastClientInputs {
    inputs: Vec<PlayerInputData>
}

#[derive(Resource, Default)]
struct NetIdToEntityId {
    net_id_to_entity_id: HashMap<NetId, Entity>
}

#[derive(Component, Default)]
struct InterpolatedTransform {
    from: Transform,
    to: Transform,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    ip: String,

    #[arg(long, default_value_t = 7001)]
    port: u16,
}

fn main() {
    let args = Args::parse();
    let remote_addr = format!("{}:{}", args.ip, args.port).parse().expect("could not parse addr");
    let socket = ResUdpSocket::new_client("0.0.0.0:0", remote_addr);
    //let addr = socket.0.local_addr().unwrap();
    //println!("local socket addr: {}", addr);
    let res_addr = ResSocketAddr(remote_addr);

    App::new()
        .insert_resource(bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::Continuous,
        })
        .insert_resource(res_addr)
        .insert_resource(socket)
        .insert_resource(NetIdToEntityId::default())
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
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(PerfUiPlugin)
        .add_plugins(DefaultPlugins)
        //.add_plugins(ClientPlugin)
        .insert_resource(networking::transport::Transport::new())
        .insert_resource(networking::HeartbeatTimer(Timer::from_seconds(
            networking::DEFAULT_HEARTBEAT_TICK_RATE_SECS,
            TimerMode::Repeating,
        )))
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
                networking::systems::client_recv_packet_system.in_set(NetworkSystem::Receive),
                send_input,
                connection_handler,
                ping_server,
                tick_simulation,
                update_scoreboard,
                networking::systems::auto_heartbeat_system.in_set(networking::ClientSystem::Heartbeat),
                networking::systems::send_packet_system.in_set(NetworkSystem::Send),
            ).chain()
        )
        .run();
}

fn connection_handler(
    mut events: EventReader<NetworkEvent>,
    mut world_states: ResMut<WorldStates>,
    mut ping_state: ResMut<PingState>,
    mut fixed_state: ResMut<FixedTickWorldResource>,
    time: Res<Time<Real>>,
) {
    fixed_state.frame_counter += 1;
    debug!("({})", fixed_state.frame_counter);
    //let mut recv_count = 0;
    for event in events.read() {
        match event {
            NetworkEvent::Message(handle, msg, recv_time) => {
                let config = config::standard();
                type ServerToClientResult = Result<(ServerToClientPacket, usize), DecodeError>;
                let decode_result: ServerToClientResult = bincode::serde::decode_from_slice(msg.as_ref(), config);
                match decode_result {
                    Ok((packet, _)) => {
                        match packet {
                            ServerToClientPacket::WorldState(ws) => {
                                //recv_count += 1;
                                world_states.states.push(ws);
                                world_states.received_per_sec.push_back(time.elapsed_seconds())
                            },
                            ServerToClientPacket::Pong(pd) => {
                                //recv_count += 1;
                                /*debug!("({}) Received pong {} at {:?}, {} event send time",
                                        fixed_state.frame_counter,
                                        pd.ping_id,
                                        time::Instant::now(),
                                        recv_time.elapsed().as_millis());*/
                                ping_state.pongs.push(pd);
                            }
                        }
                    }
                    Err(err) => {
                        warn!("Error parsing message from {}: {:?} {:?}", handle, msg.as_ref(), err);
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

fn interpolate_frame(
    mut query: Query<(&mut Transform, &InterpolatedTransform)>,
    time: Res<Time<Fixed>>,
) {
    for (mut transform, interp) in &mut query {
        let alpha= time.overstep_fraction();
        transform.translation = interp.from.translation.lerp(interp.to.translation, alpha);
    }
}

/*fn spawn_paddle_with_interpolated_transform(
    commands: &mut Commands,
    translation: Vec2,
    net_id: NetId
) -> Entity {
    spawn_paddle(commands, translation, net_id).insert(InterpolatedTransform::default()).net_id()
}

fn spawn_ball_with_interpolated_transform(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    translation: Vec2,
    net_id: NetId
) -> Entity {
    spawn_ball(commands, meshes, materials, translation, net_id).net_id()
}
fn spawn_brick_with_interpolated_transform(
    commands: &mut Commands,
    brick_position: Vec2, net_id: NetId
) -> Entity {
    spawn_brick(commands, brick_position, net_id).net_id()
}*/

// https://www.reddit.com/r/bevy/comments/su7k1d/whats_the_proper_way_to_bundle_entities_that/hxad818/
/*trait SpawnInterpolatedTransformBundleEx<'w> {
    fn spawn_interpolated_transform_bundle<B: Bundle>(
        &mut self, bundle: B
    ) -> EntityCommands;
}

impl <'w> SpawnInterpolatedTransformBundleEx<'w> for Commands<'w, 'w> {
    fn spawn_interpolated_transform_bundle<B>(
        &mut self, bundle: B
    ) -> EntityCommands where B: Bundle {
        let mut e = self.spawn(bundle);
        e.insert(InterpolatedTransform::default());
        e
    }
}*/

trait SpawnInterpolatedTransformBundleEx {
    // define a method that we will be able to call on `commands`
    fn spawn_interpolated_transform_bundle<B: Bundle>(
        &mut self, bundle: B
    ) -> Entity;
}

// implement our trait for Bevy's `Commands`
impl<'w, 's> SpawnInterpolatedTransformBundleEx for Commands<'w, 's> {
    fn spawn_interpolated_transform_bundle<B: Bundle>(
        &mut self, bundle: B
    ) -> Entity {
        let mut e = self.spawn(bundle);
        e.insert(InterpolatedTransform::default());
        e.id()
    }
}

fn sync_net_ids_if_needed_and_update_score(
    commands: &mut Commands,
    ws: &WorldStateData,
    net_id_query: &Query<(Entity, &NetId)>,
    net_id_map: &mut ResMut<NetIdToEntityId>,
    meshes: &mut Assets<Mesh>,
    score: &mut Score,
    materials: &mut Assets<ColorMaterial>
) {
    let mut ws_net_ids: Vec<NetId> = Vec::with_capacity(ws.entities.len());
    for net_ent in ws.entities.iter() {
        ws_net_ids.push(net_ent.net_id);
        if !net_id_map.net_id_to_entity_id.contains_key(&net_ent.net_id) {
            let entity_id = match &net_ent.entity_type {
                NetEntityType::Paddle(d) => {
                    Some(commands.spawn_interpolated_transform_bundle(PaddleBundle::new(d.pos, net_ent.net_id)))
                }
                NetEntityType::Brick(d) => {
                    Some(commands.spawn_interpolated_transform_bundle(BrickBundle::new(d.pos, net_ent.net_id)))
                }
                NetEntityType::Ball(d) => {
                    Some(commands.spawn_interpolated_transform_bundle(BallBundle::new(meshes, materials, d.pos, net_ent.net_id)))
                }
                NetEntityType::Score(d) => {
                    // Feels gross to do this here, TODO: find a better spot
                    score.0 = d.score;
                    None
                }
            };

            if let Some(entity_id) = entity_id {
                net_id_map.net_id_to_entity_id.insert(net_ent.net_id, entity_id);
            }
        }
    }

    for (entity, net_id) in net_id_query.iter() {
        if !ws_net_ids.contains(net_id) {
            commands.entity(entity).despawn();
            net_id_map.net_id_to_entity_id.remove(net_id);
        }
    }
}

/*fn remove_entities(
    commands: &mut Commands,
    ws: &WorldStateData,
    net_id_map: &mut ResMut<NetIdToEntityId>,
) {
    for net_id in ws.entities_removed.iter() {
        match net_id_map.net_id_to_entity_id.get(net_id) {
            Some(entity) => {
                commands.entity(*entity).despawn();
                net_id_map.net_id_to_entity_id.remove(net_id);
            }
            _ => {}
        }
    }
}*/

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
    net_id_map: &mut ResMut<NetIdToEntityId>,
    to_state: &WorldStateData
) {
    for net_ent in to_state.entities.iter() {
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
) {
    let mut input = PlayerInputData::default();

    if keyboard_input.pressed(KeyCode::ArrowLeft) {
        input.key_mask |= 1 << (NetKey::Left as u8);
    }

    if keyboard_input.pressed(KeyCode::ArrowRight) {
        input.key_mask |= 1 << (NetKey::Right as u8);
    }

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
    mut net_id_map: ResMut<NetIdToEntityId>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut score: ResMut<Score>,
    mut ping_state: ResMut<PingState>,
    fixed_state: Res<FixedTickWorldResource>,
    time: Res<Time<Real>>,
) {
    // Clear old entries from our stats
    //let now_inst = Instant::now();
    let now = time.elapsed_seconds();
    while !world_states.received_per_sec.is_empty() {
        let entry = *world_states.received_per_sec.front().unwrap();
        if now > entry && now - entry > 1.0 {
            world_states.received_per_sec.pop_front();
        }  else {
            break;
        }
    }

    for pong in ping_state.pongs.clone().iter() {
        let instant = ping_state.ping_id_to_instance.remove(&pong.ping_id).unwrap();
        debug!("({}) {} ms raw pong for ping {}", fixed_state.frame_counter, instant.elapsed().as_millis(), pong.ping_id);
    }
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
        world_states.states.remove(0);
    } else {
        world_states.interp_started = true;
        bootstrap_first_state = true;
    }

    let expected_buffer = 1 + f64::round(INTERP_DELAY_S / TICK_S) as usize;

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
    }

    //info!("{} us", (Instant::now() - now_inst).as_micros());
}

fn update_map_and_apply_world_state(
    commands: &mut Commands,
    query: &mut Query<&mut InterpolatedTransform>,
    net_id_query: &Query<(Entity, &NetId)>,
    net_id_map: &mut ResMut<NetIdToEntityId>,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<ColorMaterial>>,
    score: &mut ResMut<Score>, to_state: &WorldStateData
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
/*fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Update, hello_world_system)
        .run();
}

fn hello_world_system() {
    println!("hello world");
}*/
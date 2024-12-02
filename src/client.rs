mod networking;
mod common;
mod client_types;

mod client_util;

use clap::Parser;
use common::*;

use std::time;
use bincode::config;
use bincode::error::DecodeError;
use bevy::{prelude::*};
use bevy::utils::HashMap;
use networking::{ClientPlugin, NetworkEvent, ResSocketAddr, ResUdpSocket, Transport};
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use byteorder::ByteOrder;
use iyes_perf_ui::prelude::*;
use crate::networking::NetworkSystem;
use crate::client_types::*;
use crate::client_util as util;

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
                interpolate_frame_for_render,
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
    let original_paddle_transforms = util::rollback_all(local_paddle_query.iter_mut(), &most_recent_state);
    let original_ball_transforms = util::rollback_all(ball_query.iter_mut(), &most_recent_state);

    let mut entities_to_ignore = Vec::new();
    let last_idx = inputs.len() - 1;

    for (i, input) in unacked_inputs.inputs.iter().enumerate() {
        if i == last_idx {
            // Print mispredicts. The last input in the list hasn't been predicted yet and is
            // for this frame. So to detect mispredicts we need to compare to the state BEFORE
            // that last input has been applied
            util::detect_mispredicts(
                &ball_query,
                &local_paddle_query,
                &original_paddle_transforms,
                &original_ball_transforms
            );
        }

        // Forward predict paddles and balls
        util::resimulate_all(local_paddle_query.iter_mut(), input);
        util::resimulate_all(ball_query.iter_mut(), input);

        // Perform collision detection on predicted objects
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

fn interpolate_frame_for_render(
    mut query: Query<(&mut Transform, &InterpolatedTransform)>,
    time: Res<Time<Fixed>>,
) {
    for (mut transform, interp) in &mut query {
        let alpha= time.overstep_fraction();
        transform.translation = interp.from.translation.lerp(interp.to.translation, alpha);
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
        util::update_map_and_apply_world_state(
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
        util::update_map_and_apply_world_state(
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
        util::update_map_and_apply_world_state(
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


use clap::Parser;
mod networking;
mod common;

use common::*;
use std::{net::UdpSocket, time, time::Duration};
use std::collections::VecDeque;
use std::ffi::c_void;
use bevy::utils::HashMap;
use std::net::SocketAddr;
use bevy::prelude::*;
use bincode;
use bincode::config;
use bincode::error::DecodeError;
use networking::{NetworkEvent, ServerPlugin, Transport, ResUdpSocket};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;
use windows::Win32::Networking::WinSock;
use std::os::windows::io::AsRawSocket;
use windows::Win32::Foundation;
use crate::networking::{NetworkResource, NetworkSystem};

pub const LISTEN_ADDRESS: &str = "127.0.0.1:7001";
const BUFFER_DELAY_S: f64 = 1.0 * TICK_S + MIN_JITTER_S;
const BUFFER_LEN: usize = 1 + ((BUFFER_DELAY_S / TICK_S) as usize);
const PADDLE_LEFT_BOUND: f32 = LEFT_WALL + WALL_THICKNESS / 2.0 + PADDLE_SIZE.x / 2.0 + PADDLE_PADDING;
const PADDLE_RIGHT_BOUND: f32 = RIGHT_WALL - WALL_THICKNESS / 2.0 - PADDLE_SIZE.x / 2.0 - PADDLE_PADDING;

#[derive(Component)]
struct NetConnection {
    addr: SocketAddr,
    paddle_entity: Entity,
    ball_entity: Entity
}

#[derive(Default)]
struct ReceivedPlayerInput {
    data: PlayerInputData,
    time_received: f32
}

#[derive(Clone, Copy, Default)]
enum NetInputState {
    #[default]
    Buffering,
    Playing
}

#[derive(Component, Default)]
struct NetInput {
    input_state: NetInputState,
    inputs: VecDeque<ReceivedPlayerInput>,
    pings: VecDeque<PingData> // Not a good place for this, but being fast
}

#[derive(Resource, Default)]
struct NetConnections {
    addr_to_entity: HashMap<SocketAddr, Entity>,    // Players are removed when they disconnect
    next_player_index: u8
}

#[derive(Resource)]
struct RandomGen {
    r: ChaCha8Rng
}

#[derive(Resource)]
struct NetIdGenerator {
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
    fn next(&mut self) -> NetId {
        let next = self.next;
        self.next += 1;
        NetId(next)
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = LISTEN_ADDRESS)]
    bind: String,
}

fn main() {
    let args = Args::parse();

    let socket = ResUdpSocket::new_server(&args.bind);

    let rng = RandomGen{ r: ChaCha8Rng::seed_from_u64(1337) };

    println!("Server now listening on {}", args.bind);

    let generator = NetIdGenerator::default();

    App::new()
        .insert_resource(bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::Continuous,
        })
        .insert_resource(socket)
        .insert_resource(rng)
        .add_plugins(DefaultPlugins)
        .add_plugins(networking::ServerPlugin{sim_settings: Default::default(), no_systems: true})
        .insert_resource(Time::<Fixed>::from_hz(TICK_RATE_HZ))
        .insert_resource(Score(0))
        .insert_resource(ClearColor(BACKGROUND_COLOR))
        .insert_resource(generator)
        .insert_resource(NetConnections::default())
        .insert_resource(FixedTickWorldResource::default())
        .add_event::<CollisionEvent>()
        .add_systems(Startup, setup)
        .add_systems(
            FixedUpdate,
            (
                common::start_tick,
                networking::systems::server_recv_packet_system.in_set(NetworkSystem::Receive),
                networking::systems::idle_timeout_system.in_set(networking::ServerSystem::IdleTimeout),
                connection_handler,
                process_input,
                apply_velocity,
                check_for_collisions,
                update_scoreboard,
                broadcast_world_state,
                networking::systems::send_packet_system.in_set(NetworkSystem::Send),
                common::end_tick
            ).chain()
        )
        .run();
}

fn setup(
    mut commands: Commands,
    mut net_id_gen: ResMut<NetIdGenerator>
) {
    // Camera
    commands.spawn(Camera2dBundle::default());

    // Sound
    //let ball_collision_sound = asset_server.load("sounds/breakout_collision.ogg");
    //commands.insert_resource(CollisionSound(ball_collision_sound));

    // Scoreboard
    commands.spawn(ScoreboardUiBundle::new());

    // Walls
    commands.spawn(WallBundle::new(WallLocation::Left));
    commands.spawn(WallBundle::new(WallLocation::Right));
    commands.spawn(WallBundle::new(WallLocation::Bottom));
    commands.spawn(WallBundle::new(WallLocation::Top));

    // Bricks
    let total_width_of_bricks = (RIGHT_WALL - LEFT_WALL) - 2. * GAP_BETWEEN_BRICKS_AND_SIDES;
    let bottom_edge_of_bricks = PADDLE_Y + GAP_BETWEEN_PADDLE_AND_BRICKS;
    let total_height_of_bricks = TOP_WALL - bottom_edge_of_bricks - GAP_BETWEEN_BRICKS_AND_CEILING;

    assert!(total_width_of_bricks > 0.0);
    assert!(total_height_of_bricks > 0.0);

    // Given the space available, compute how many rows and columns of bricks we can fit
    let n_columns = (total_width_of_bricks / (BRICK_SIZE.x + GAP_BETWEEN_BRICKS)).floor() as usize;
    let n_rows = (total_height_of_bricks / (BRICK_SIZE.y + GAP_BETWEEN_BRICKS)).floor() as usize;
    let n_vertical_gaps = n_columns - 1;

    // Because we need to round the number of columns,
    // the space on the top and sides of the bricks only captures a lower bound, not an exact value
    let center_of_bricks = (LEFT_WALL + RIGHT_WALL) / 2.0;
    let left_edge_of_bricks = center_of_bricks
        // Space taken up by the bricks
        - (n_columns as f32 / 2.0 * BRICK_SIZE.x)
        // Space taken up by the gaps
        - n_vertical_gaps as f32 / 2.0 * GAP_BETWEEN_BRICKS;

    // In Bevy, the `translation` of an entity describes the center point,
    // not its bottom-left corner
    let offset_x = left_edge_of_bricks + BRICK_SIZE.x / 2.;
    let offset_y = bottom_edge_of_bricks + BRICK_SIZE.y / 2.;

    for row in 0..n_rows {
        for column in 0..n_columns {
            let brick_position = Vec2::new(
                offset_x + column as f32 * (BRICK_SIZE.x + GAP_BETWEEN_BRICKS),
                offset_y + row as f32 * (BRICK_SIZE.y + GAP_BETWEEN_BRICKS),
            );

            commands.spawn(BrickBundle::new(brick_position, net_id_gen.next()));
        }
    }
}

fn connection_handler(
    mut commands: Commands,
    mut events: EventReader<NetworkEvent>,
    mut rng: ResMut<RandomGen>,
    mut net_id_gen: ResMut<NetIdGenerator>,
    mut client_query: Query<(&mut NetConnection, &mut NetInput)>,
    mut connections: ResMut<NetConnections>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut world_resource: ResMut<FixedTickWorldResource>,
    mut real_time: Res<Time<Real>>
) {
    world_resource.frame_counter += 1;
    debug!("[{}]", world_resource.frame_counter);

    let mut num_inputs_processed = 0;
    for event in events.read() {
        match event {
            NetworkEvent::Connected(handle) => {
                info!("{}: connected!", handle);

                let next_player = NetPlayerIndex(connections.next_player_index);
                let paddle_x = rng.r.gen_range(PADDLE_LEFT_BOUND..=PADDLE_RIGHT_BOUND);
                let paddle_entity = commands.spawn(PaddleBundle::new(Vec2::new(paddle_x, PADDLE_Y), net_id_gen.next(), next_player)).id();
                let ball_entity = commands.spawn(BallBundle::new(&mut meshes, &mut materials, BALL_STARTING_POSITION, net_id_gen.next(), next_player)).id();

                let id = commands.spawn((
                    NetConnection {
                        addr: *handle,
                        paddle_entity,
                        ball_entity
                    },
                    NetInput::default()
                )).id();
                connections.addr_to_entity.insert(handle.clone(), id);
                connections.next_player_index += 1;
            }
            NetworkEvent::Disconnected(handle) => {
                info!("{}: disconnected!", handle);
                handle_client_disconnected(
                    handle,
                    &mut commands,
                    &mut client_query,
                    &mut connections,
                );
            }
            NetworkEvent::Message(handle, msg, recv_time) => {
                let id = connections.addr_to_entity.get(handle);
                if id.is_none() || !client_query.contains(*id.unwrap()) {
                    warn!("NetworkEvent::Message received from {}, but player was not found", handle);
                } else {
                    let id = id.unwrap();
                    let config = config::standard();
                    type ClientToServerResult = Result<(ClientToServerPacket, usize), DecodeError>;
                    let decode_result: ClientToServerResult = bincode::serde::decode_from_slice(msg.as_ref(), config);
                    match decode_result {
                        Ok((packet, _)) => {
                            match packet {
                                ClientToServerPacket::Input(input) => {
                                    num_inputs_processed += 1;
                                    //debug!("recv: {}", real_time.elapsed_seconds());
                                    client_query.get_mut(*id).unwrap().1.inputs.push_back(
                                        ReceivedPlayerInput {
                                            data: input,
                                            time_received: real_time.elapsed_seconds()
                                        }
                                    );
                                },
                                ClientToServerPacket::Ping(rtt) => {
                                    debug!("Received ping {} at {:?}, {} event send time",
                                        rtt.ping_id,
                                        time::Instant::now(),
                                        recv_time.elapsed().as_millis());
                                    client_query.get_mut(*id).unwrap().1.pings.push_back(rtt);
                                }
                            }
                        }
                        Err(err) => {
                            warn!("{}: Error parsing message from {}: {:?} {:?}", id, handle, err, msg);
                        }
                    }
                    //info!("{}: Message from {}: {:?}", net_id, handle, msg);
                }
                //info!("{} sent a message: {:?}", handle, msg);
            }
            NetworkEvent::SendError(handle, err, msg) => {
                handle_client_disconnected(
                    handle,
                    &mut commands,
                    &mut client_query,
                    &mut connections,
                );
                error!(
                    "NetworkEvent::SendError (payload [{:?}]): {:?}",
                    msg.payload, err
                );
            }
            NetworkEvent::RecvError(err) => {
                error!("NetworkEvent::RecvError: {:?}", err);
            }
        }
    }

    debug!("{} inputs processed!", num_inputs_processed);
}

fn handle_client_disconnected(
    handle: &SocketAddr,
    commands: &mut Commands,
    client_query:
    &mut Query<(&mut NetConnection, &mut NetInput)>,
    connections: &mut ResMut<NetConnections>,
) {
    if connections.addr_to_entity.contains_key(handle) {
        let id = connections.addr_to_entity.get(handle).unwrap();
        let conn = client_query.get(*id).unwrap().0;
        commands.entity(conn.paddle_entity).despawn();
        commands.entity(conn.ball_entity).despawn();
        commands.entity(*id).despawn();
        connections.addr_to_entity.remove(handle);
    }
}

fn broadcast_world_state(
    bricks: Query<(&Transform, &NetId), With<Brick>>,
    balls: Query<(&Transform, &NetId, &NetPlayerIndex) , With<Ball>>,
    paddles: Query<(&Transform, &NetId, &NetPlayerIndex), With<Paddle>>,
    score: Res<Score>,
    mut transport: ResMut<Transport>,
    world_resource: Res<FixedTickWorldResource>,
    connections: ResMut<NetConnections>,
    mut client_query: Query<(&NetConnection, &mut NetInput)>,
) {
    if connections.addr_to_entity.is_empty() {
        return;
    }

    // This is definitely not as fast as it could be. Hand-serializing
    // directly into a buffer is probably faster than first copying into here?
    let mut world = WorldStateData::default();
    world.frame = world_resource.frame_counter;
    for (transform, &id) in bricks.iter() {
        world.entities.push(NetEntity {
            entity_type: NetEntityType::Brick(NetBrickData { pos: transform.translation.xy() }),
            net_id: id
        });
    }

    for (transform, &id, &player) in balls.iter() {
        world.entities.push(NetEntity {
            entity_type: NetEntityType::Ball(NetBallData { pos: transform.translation.xy(), player_index: player }),
            net_id: id
        });
    }

    for (transform, &id, &player) in paddles.iter() {
        world.entities.push(NetEntity {
            entity_type: NetEntityType::Paddle(NetPaddleData { pos: transform.translation.xy(), player_index: player }),
            net_id: id
        });
    }

    world.entities.push(NetEntity {
        entity_type: NetEntityType::Score(NetScoreData { score: score.0 }),
        net_id: NetId(0) // Singleton entity
    });

    // Will just blow up if world state gets to big, fine by me right now
    let packet = ServerToClientPacket::WorldState(world);
    let mut world_state_buf = [0; networking::ETHERNET_MTU];
    let num_bytes = bincode::serde::encode_into_slice(packet, &mut world_state_buf, config::standard()).unwrap();

    for (conn, mut input) in client_query.iter_mut() {
        transport.send(conn.addr, &world_state_buf[..num_bytes]);

        let mut ping_buf = [0; networking::ETHERNET_MTU];
        for ping in &input.pings {
            let packet = ServerToClientPacket::Pong(ping.clone());
            let num_bytes = bincode::serde::encode_into_slice(packet, &mut ping_buf, config::standard()).unwrap();

            debug!("Sent ping {} to {} at {:?}", ping.ping_id, conn.addr, time::Instant::now());

            transport.send(conn.addr, &ping_buf[..num_bytes]);
        }
        input.pings.clear();
    }
}

fn apply_velocity(mut query: Query<(&mut Transform, &Velocity)>, time: Res<Time>) {
    for (mut transform, velocity) in &mut query {
        transform.translation.x += velocity.x * time.delta_seconds();
        transform.translation.y += velocity.y * time.delta_seconds();
    }
}

// Not good strict ECS because i'm mutating both input and transforms in the same system, should maybe be broken up with events?
fn process_input(
    mut client_query: Query<(&NetConnection, &mut NetInput)>,
    mut paddle_query: Query<&mut Transform, With<Paddle>>,
    mut fixed_time: Res<Time>,
    mut real_time: Res<Time<Real>>,
) {
    for (net_connection, mut net_input) in client_query.iter_mut() {
        let mut paddle_transform = paddle_query.get_mut(net_connection.paddle_entity).unwrap();
        let mut direction = 0.0;


        let input_state = net_input.input_state;
        match input_state {
            NetInputState::Buffering => {
                let now = real_time.elapsed_seconds();
                if net_input.inputs.is_empty() {
                    info!("EMPTY INPUTS BUFFERING");
                    continue;
                } else if now - net_input.inputs.front().unwrap().time_received < BUFFER_DELAY_S as f32 {
                    info!("(NOW {}) {:?}", now, net_input.inputs.iter().map(|input| input.time_received).collect::<Vec<_>>());
                    continue;
                } else {
                    net_input.input_state = NetInputState::Playing;
                }
            }
            NetInputState::Playing => {
                if net_input.inputs.is_empty()  {
                    info!("EMPTY INPUTS TRANSITION TO BUFFERING");
                    net_input.input_state = NetInputState::Buffering;
                    continue;
                }
            }
        }

        /*if net_input.inputs.is_empty()  {
            info!("EMPTY INPUTS");
            continue;
        }

        let now = real_time.elapsed_seconds();
        if now - net_input.inputs.front().unwrap().time_received < BUFFER_DELAY_S {
            info!("{} {} {} BUFFERING INPUT {} < {} BACK DIFF (len {}) {} ",
                now,
                net_input.inputs.front().unwrap().time_received,
                net_input.inputs.back().unwrap().time_received,
                now - net_input.inputs.front().unwrap().time_received,
                BUFFER_DELAY_S,
                net_input.inputs.len(),
                now - net_input.inputs.back().unwrap().time_received);
            info!("(NOW {}) {:?}", now, net_input.inputs.iter().map(|input| input.time_received).collect::<Vec<_>>());

            continue;
        }*/

        let mut num_consumed = 0;
        let inputs = &mut net_input.inputs;
        assert!(!inputs.is_empty());
        loop {
            // Always consume at least one input
            let input = inputs.pop_front().unwrap();
            let buttons = input.data.key_mask;
            if (buttons & (1 << NetKey::Left as u8)) != 0 {
                direction -= 1.0;
            }

            if (buttons & (1 << NetKey::Right as u8)) != 0{
                direction += 1.0;
            }

            // Calculate the new horizontal paddle position based on player input
            let new_paddle_position =
                paddle_transform.translation.x + direction * PADDLE_SPEED * fixed_time.delta_seconds();

            // Update the paddle position,
            // making sure it doesn't cause the paddle to leave the arena
            paddle_transform.translation.x = new_paddle_position.clamp(PADDLE_LEFT_BOUND, PADDLE_RIGHT_BOUND);

            num_consumed += 1;

            if inputs.len() < BUFFER_LEN {
                //info!("BREAK {} remaining in buffer, {} consumed", inputs.len(), num_consumed);
                if num_consumed > 1 {
                    info!("{} consumed to catch up, {} remaining in buffer", num_consumed, inputs.len());
                }
                break;
            }
        }
        //info!("DRAINING {} INPUTS", net_input.inputs.len());
        /*for input in net_input.inputs.drain(..) {
            let buttons = input.data.key_mask;
            if (buttons & (1 << NetKey::Left as u8)) != 0 {
                direction -= 1.0;
            }

            if (buttons & (1 << NetKey::Right as u8)) != 0{
                direction += 1.0;
            }

            // Calculate the new horizontal paddle position based on player input
            let new_paddle_position =
                paddle_transform.translation.x + direction * PADDLE_SPEED * time.delta_seconds();

            // Update the paddle position,
            // making sure it doesn't cause the paddle to leave the arena
            paddle_transform.translation.x = new_paddle_position.clamp(PADDLE_LEFT_BOUND, PADDLE_RIGHT_BOUND);
        }*/
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
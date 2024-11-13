use clap::Parser;
mod networking;
mod server_types;
use crate::server_types::*;
mod common;
use common::*;
use std::time;
use std::net::SocketAddr;
use bevy::math::bounding::{Aabb2d};
use bevy::prelude::*;
use bincode;
use bincode::config;
use bincode::error::DecodeError;
use networking::{NetworkEvent, Transport, ResUdpSocket};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;
use crate::networking::NetworkSystem;
use byteorder::ByteOrder;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = LISTEN_ADDRESS)]
    bind: String,

    #[command(flatten)]
    sim_latency: SimLatencyArgs
}

fn main() {
    let args = Args::parse();
    let socket = ResUdpSocket::new_server(&args.bind);
    let rng = RandomGen{ r: ChaCha8Rng::seed_from_u64(1337) };
    let generator = NetIdGenerator::default();

    let sim_settings = args.sim_latency.into();

    println!("Server now listening on {}", args.bind);

    App::new()
        .insert_resource(bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::Continuous,
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(networking::ServerPlugin{sim_settings, no_systems: true})
        .insert_resource(socket)
        .insert_resource(rng)
        .insert_resource(Time::<Fixed>::from_hz(TICK_RATE_HZ))
        .insert_resource(Score(0))
        .insert_resource(ClearColor(BACKGROUND_COLOR))
        .insert_resource(generator)
        .insert_resource(NetConnections::default())
        .insert_resource(FixedTickWorldResource::default())
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

    //let circ = BoundingCircle::new(Vec2::new(0.0, 0.0), BALL_DIAMETER / 2.);
    let aabb = Aabb2d::new(
        Vec2::new(0.0, 0.0),
        Vec2::new(4.0, 6.0),
    );

    let p = aabb.closest_point(Vec2::new(7.0, 3.0));

    info!("{:?}", p);

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
    real_time: Res<Time<Real>>
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
                        ball_entity,
                        last_applied_input: 0,
                        player_index: next_player.0
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

fn write_header(buf: &mut [u8], conn: &NetConnection) {
    byteorder::NetworkEndian::write_u32(buf, WORLD_PACKET_HEADER_TAG);
    byteorder::NetworkEndian::write_u32(&mut buf[size_of::<u32>()..], conn.last_applied_input);
    buf[size_of::<u32>() * 2] = conn.player_index;
}

fn broadcast_world_state(
    bricks: Query<(&Transform, &NetId), With<Brick>>,
    balls: Query<(&Transform, &NetId, &Velocity, &NetPlayerIndex) , With<Ball>>,
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
    let mut world = NetWorldStateData::default();
    world.frame = world_resource.frame_counter;
    for (transform, &id) in bricks.iter() {
        world.entities.push(NetEntity {
            entity_type: NetEntityType::Brick(NetBrickData { pos: transform.translation.xy() }),
            net_id: id
        });
    }

    for (transform, &id, velocity, &player) in balls.iter() {
        world.entities.push(NetEntity {
            entity_type: NetEntityType::Ball(NetBallData { pos: transform.translation.xy(), velocity: velocity.0, player_index: player }),
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
    byteorder::NetworkEndian::write_u32(&mut world_state_buf, WORLD_PACKET_HEADER_TAG);
    // A U32 HERE will be the only one changed, min serialization overhead

    let num_bytes = HEADER_LEN + bincode::serde::encode_into_slice(packet, &mut world_state_buf[HEADER_LEN..], config::standard()).unwrap();

    for (conn, mut input) in client_query.iter_mut() {
        // Hand-serializing only the data that changes. This means we do the least serialization per client
        byteorder::NetworkEndian::write_u32(&mut world_state_buf[size_of::<u32>()..], conn.last_applied_input);
        world_state_buf[size_of::<u32>() * 2] = conn.player_index;
        transport.send(conn.addr, &world_state_buf[..num_bytes]);

        let mut ping_buf = [0; networking::ETHERNET_MTU];
        write_header(&mut ping_buf, conn);

        for ping in &input.pings {
            let packet = ServerToClientPacket::Pong(ping.clone());
            let num_bytes = HEADER_LEN + bincode::serde::encode_into_slice(packet, &mut ping_buf[HEADER_LEN..], config::standard()).unwrap();

            debug!("Sent ping {} to {} at {:?}", ping.ping_id, conn.addr, time::Instant::now());

            transport.send(conn.addr, &ping_buf[..num_bytes]);
        }
        input.pings.clear();
    }
}

fn apply_velocity(mut query: Query<(&mut Transform, &Velocity)>, time: Res<Time<Fixed>>) {
    for (mut transform, velocity) in &mut query {
        transform.translation.x += velocity.x * time.delta_seconds();
        transform.translation.y += velocity.y * time.delta_seconds();
    }
}

pub fn check_for_collisions(
    mut commands: Commands,
    mut score: ResMut<Score>,
    mut ball_query: Query<(&mut Velocity, &Transform), With<Ball>>,
    collider_query: Query<(Entity, &Transform, Option<&Brick>), (With<Collider>, Without<Ball>)>,
) {
    let colliders: Vec<(Entity, Transform, Option<Brick>)> = collider_query
        .iter()
        .map(|(e, t, b)| {
            if b.is_some() {
                (e, t.clone(), Some(*(b.unwrap())))
            } else {
                (e, t.clone(), None)
            }
        })
        .collect::<Vec<_>>();

    let mut entities_to_delete = Vec::new();
    for (mut ball_velocity, ball_transform) in ball_query.iter_mut() {
        check_single_ball_collision(&mut score, &colliders, &ball_transform, &mut ball_velocity, &mut entities_to_delete);
    }

    for e in entities_to_delete {
        commands.entity(e).despawn();
    }
}

// Not good strict ECS because i'm mutating both input and transforms in the same system, should maybe be broken up with events?
fn process_input(
    mut client_query: Query<(&mut NetConnection, &mut NetInput)>,
    mut paddle_query: Query<&mut Transform, With<Paddle>>,
    fixed_time: Res<Time<Fixed>>,
    real_time: Res<Time<Real>>,
) {
    for (mut net_connection, mut net_input) in client_query.iter_mut() {
        let mut paddle_transform = paddle_query.get_mut(net_connection.paddle_entity).unwrap();

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

        let mut num_consumed = 0;
        let mut last_consumed;
        let inputs = &mut net_input.inputs;
        assert!(!inputs.is_empty());
        loop {
            // Always consume at least one input
            let input = inputs.pop_front().unwrap();

            move_paddle(fixed_time.delta_seconds(), &mut paddle_transform, &input.data);

            num_consumed += 1;
            last_consumed = input.data.sequence;

            if inputs.len() < BUFFER_LEN {
                //info!("BREAK {} remaining in buffer, {} consumed", inputs.len(), num_consumed);
                if num_consumed > 1 {
                    info!("{} consumed to catch up, {} remaining in buffer", num_consumed, inputs.len());
                }
                break;
            }
        }

        net_connection.last_applied_input = last_consumed;
    }
}

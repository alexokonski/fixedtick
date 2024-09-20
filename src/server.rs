mod networking;
mod common;

use common::*;
use std::{net::UdpSocket, time::Duration};
use bevy::utils::HashMap;
use std::net::SocketAddr;
use bevy::{
    log::LogPlugin,
    math::bounding::{Aabb2d, BoundingCircle, BoundingVolume, IntersectsVolume},
    prelude::*,
    sprite::MaterialMesh2dBundle,
};
use bevy::asset::AssetContainer;
use bevy::ecs::system::EntityCommands;
use bincode;
use bincode::config;
use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use bincode::error::DecodeError;
use networking::{NetworkEvent, ServerPlugin, Transport, ResUdpSocket};

pub const LISTEN_ADDRESS: &str = "127.0.0.1:4567";

#[derive(Component)]
struct NetConnection {
    addr: SocketAddr,
    paddle_entity: Entity,
    ball_entity: Entity
}

#[derive(Component, Default)]
struct NetInput {
    inputs: Vec<PlayerInput>
}

#[derive(Resource, Default)]
struct NetConnections {
    addr_to_entity: HashMap<SocketAddr, Entity>
}


#[derive(Resource, Default)]
struct NetIdGenerator {
    next: u16
}

impl NetIdGenerator {
    fn next(&mut self) -> NetId {
        let next = self.next;
        self.next += 1;
        NetId(next)
    }
}

#[derive(Resource, Default)]
struct EntitiesRemoved {
    entities: Vec<Entity>
}


fn main() {
    let socket = ResUdpSocket(UdpSocket::bind(LISTEN_ADDRESS).expect("could not bind socket"));
    socket.0
        .set_nonblocking(true)
        .expect("could not set socket to be nonblocking");
    socket.0
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("could not set read timeout");

    info!("Server now listening on {}", LISTEN_ADDRESS);

    let mut generator = NetIdGenerator::default();

    App::new()
        .insert_resource(socket)
        .add_plugins(DefaultPlugins)
        .add_plugins(ServerPlugin)
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(Score(0, generator.next()))
        .insert_resource(ClearColor(BACKGROUND_COLOR))
        .insert_resource(generator)
        .insert_resource(NetConnections::default())
        .insert_resource(FixedTickWorldResource::default())
        .add_event::<CollisionEvent>()
        .add_systems(Startup, setup)
        .add_systems(Update, connection_handler)
        .add_systems(
            FixedUpdate,
            (
                move_paddles,
                apply_velocity,
                check_for_collisions,
                update_scoreboard,
                broadcast_world_state
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
    mut transport: ResMut<Transport>,
    mut net_id_gen: ResMut<NetIdGenerator>,
    mut client_query: Query<(&mut NetConnection, &mut NetInput)>,
    mut net_id_query: Query<&NetId>,
    mut connections: ResMut<NetConnections>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut world_resource: ResMut<FixedTickWorldResource>
) {
    for event in events.read() {
        match event {
            NetworkEvent::Connected(handle) => {
                info!("{}: connected!", handle);
                let paddle_entity = commands.spawn(PaddleBundle::new(Vec2::new(0.0, PADDLE_Y), net_id_gen.next())).id();
                let ball_entity = commands.spawn(BallBundle::new(&mut meshes, &mut materials, BALL_STARTING_POSITION, net_id_gen.next())).id();
                let id = commands.spawn((
                    NetConnection {
                        addr: handle.clone(),
                        paddle_entity,
                        ball_entity
                    },
                    NetInput::default()
                )).id();
                connections.addr_to_entity.insert(handle.clone(), id);
            }
            NetworkEvent::Disconnected(handle) => {
                info!("{}: disconnected!", handle);
                handle_client_disconnected(
                    handle,
                    &mut commands,
                    &mut client_query,
                    &mut net_id_query,
                    &mut connections,
                    &mut world_resource
                );
            }
            NetworkEvent::Message(handle, msg) => {
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
                                    client_query.get_mut(*id).unwrap().1.inputs.push(input);
                                }
                            }
                        }
                        Err(err) => {
                            warn!("{}: Error parsing message from {}: {:?} {:?}", id, handle, err, msg);
                        }

                    }
                    //info!("{}: Message from {}: {:?}", net_id, handle, msg);
                }
                info!("{} sent a message: {:?}", handle, msg);
            }
            NetworkEvent::SendError(handle, err, msg) => {
                handle_client_disconnected(
                    handle,
                    &mut commands,
                    &mut client_query,
                    &mut net_id_query,
                    &mut connections,
                    &mut world_resource
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
}

fn handle_client_disconnected(
    handle: &SocketAddr,
    mut commands: &mut Commands,
    client_query:
    &mut Query<(&mut NetConnection, &mut NetInput)>,
    net_id_query: &mut Query<&NetId>,
    connections: &mut ResMut<NetConnections>,
    mut world_resource: &mut ResMut<FixedTickWorldResource>
) {
    if connections.addr_to_entity.contains_key(handle) {
        let id = connections.addr_to_entity.get(handle).unwrap();
        let conn = client_query.get(*id).unwrap().0;

        if let Ok(net_id) = net_id_query.get(conn.paddle_entity) {
            despawn_net_id_entity(&mut commands, *id, *net_id, &mut world_resource);
        }
        if let Ok(net_id) = net_id_query.get(conn.ball_entity) {
            despawn_net_id_entity(&mut commands, *id, *net_id, &mut world_resource);
        }
        commands.entity(*id).despawn();
        connections.addr_to_entity.remove(handle);
    }
}

fn broadcast_world_state(
    bricks: Query<(&Transform, &NetId), With<Brick>>,
    balls: Query<(&Transform, &NetId), With<Ball>>,
    paddles: Query<(&Transform, &NetId), With<Paddle>>,
    score: Res<Score>,
    mut transport: ResMut<Transport>,
    mut world_resource: ResMut<FixedTickWorldResource>,
    connections: ResMut<NetConnections>
) {
    world_resource.frame_counter += 1;

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

    for (transform, &id) in balls.iter() {
        world.entities.push(NetEntity {
            entity_type: NetEntityType::Ball(NetBallData { pos: transform.translation.xy() }),
            net_id: id
        });
    }

    for (transform, &id) in paddles.iter() {
        world.entities.push(NetEntity {
            entity_type: NetEntityType::Paddle(NetPaddleData { pos: transform.translation.xy() }),
            net_id: id
        });
    }

    world.entities.push(NetEntity {
        entity_type: NetEntityType::Score(NetScoreData { score: score.0 }),
        net_id: score.1
    });

    //world.entities_removed = world_resource.net_ids_removed_this_frame.clone();
    //world_resource.net_ids_removed_this_frame.clear();

    let mut buf = [0; networking::ETHERNET_MTU];

    // Will just blow up if world state gets to big, fine by me right now
    let packet = ServerToClientPacket::WorldState(world);
    let num_bytes = bincode::serde::encode_into_slice(packet, &mut buf, config::standard()).unwrap();

    for (addr, _) in connections.addr_to_entity.iter() {
        transport.send(*addr, &buf[..num_bytes]);
    }
}

fn apply_velocity(mut query: Query<(&mut Transform, &Velocity)>, time: Res<Time>) {
    for (mut transform, velocity) in &mut query {
        transform.translation.x += velocity.x * time.delta_seconds();
        transform.translation.y += velocity.y * time.delta_seconds();
    }
}

// Not good strict ECS because i'm mutating both input and transforms in the same system, should maybe be broken up with events?
fn move_paddles(
    mut client_query: Query<(&NetConnection, &mut NetInput)>,
    mut paddle_query: Query<&mut Transform, With<Paddle>>,
    time: Res<Time>,
) {
    for (net_connection, mut net_input) in client_query.iter_mut() {
        let mut paddle_transform = paddle_query.get_mut(net_connection.paddle_entity).unwrap();
        let mut direction = 0.0;

        // buffer 2 inputs
        if net_input.inputs.len() < 2 {
            continue;
        }

        let buttons = net_input.inputs.remove(0).key_mask;

        if (buttons & NetKey::Left as u8) != 0 {
            direction -= 1.0;
        }

        if (buttons & NetKey::Right as u8) != 0{
            direction += 1.0;
        }

        // Calculate the new horizontal paddle position based on player input
        let new_paddle_position =
            paddle_transform.translation.x + direction * PADDLE_SPEED * time.delta_seconds();

        // Update the paddle position,
        // making sure it doesn't cause the paddle to leave the arena
        let left_bound = LEFT_WALL + WALL_THICKNESS / 2.0 + PADDLE_SIZE.x / 2.0 + PADDLE_PADDING;
        let right_bound = RIGHT_WALL - WALL_THICKNESS / 2.0 - PADDLE_SIZE.x / 2.0 - PADDLE_PADDING;

        paddle_transform.translation.x = new_paddle_position.clamp(left_bound, right_bound);
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
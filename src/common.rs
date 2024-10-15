use std::time;
use bevy::{
    math::bounding::{Aabb2d, BoundingCircle, BoundingVolume, IntersectsVolume},
    prelude::*,
    sprite::MaterialMesh2dBundle,
};
use serde::Serialize;
use serde::Deserialize;
use clap::Args;
use crate::networking;

pub const WORLD_PACKET_HEADER_TAG: u32 = 0xba11ba11;
pub const HEADER_LEN: usize = size_of::<u32>() * 2 + size_of::<u8>();
pub const TICK_RATE_HZ: f64 = 60.0;
pub const TICK_S: f64 = 1.0 / TICK_RATE_HZ;
pub const MIN_JITTER_S: f64 = (1.0 / 1000.0) * 6.0;

// These constants are defined in `Transform` units.
// Using the default 2D camera they correspond 1:1 with screen pixels.
pub const PADDLE_SIZE: Vec2 = Vec2::new(120.0, 20.0);

pub const BALL_DIAMETER: f32 = 30.;
pub const BALL_SPEED: f32 = 400.0;
pub const INITIAL_BALL_DIRECTION: Vec2 = Vec2::new(0.5, -0.5);

pub const WALL_THICKNESS: f32 = 10.0;
// x coordinates
pub const LEFT_WALL: f32 = -450.;
pub const RIGHT_WALL: f32 = 450.;
// y coordinates
pub const BOTTOM_WALL: f32 = -300.;

pub const TOP_WALL: f32 = 300.;

pub const BRICK_SIZE: Vec2 = Vec2::new(100., 30.);

pub const SCOREBOARD_FONT_SIZE: f32 = 40.0;
pub const SCOREBOARD_TEXT_PADDING: Val = Val::Px(5.0);

pub const RED: Color = Color::srgb(0.8, 0.0, 0.0);
pub const GREEN: Color = Color::srgb(0.0, 0.8, 0.0);
pub const BLUE: Color = Color::srgb(0.0, 0.0, 0.8);
pub const PURPLE: Color = Color::srgb(0.8, 0.0, 0.0);
pub const YELLOW: Color = Color::srgb(0.8, 0.8, 0.0);
pub const CYAN: Color = Color::srgb(0.0, 0.8, 0.8);
pub const VIOLET: Color = Color::srgb(0.8, 0.0, 0.8);

pub const NUM_COLORS: usize = 7;
pub const COLORS: [Color; NUM_COLORS] = [RED, GREEN, BLUE, PURPLE, YELLOW, CYAN, VIOLET];

pub const BRICK_COLOR: Color = Color::srgb(0.5, 0.5, 1.0);
pub const WALL_COLOR: Color = Color::srgb(0.8, 0.8, 0.8);
pub const TEXT_COLOR: Color = Color::srgb(0.5, 0.5, 1.0);
pub const SCORE_COLOR: Color = Color::srgb(1.0, 0.5, 0.5);

#[derive(Component)]
pub struct Paddle;

#[derive(Component)]
pub struct Ball;

#[derive(Component, Deref, DerefMut)]
pub struct Velocity(pub Vec2);

#[derive(Component)]
pub struct Collider;

#[derive(Event, Default)]
pub struct CollisionEvent;

#[derive(Component, Clone, Copy)]
pub struct Brick;

// This bundle is a collection of the components that define a "wall" in our game
#[derive(Bundle)]
pub struct WallBundle {
    // You can nest bundles inside of other bundles like this
    // Allowing you to compose their functionality
    sprite_bundle: SpriteBundle,
    collider: Collider,
}

/// Which side of the arena is this wall located on?
pub enum WallLocation {
    Left,
    Right,
    Bottom,
    Top,
}

impl WallLocation {
    /// Location of the *center* of the wall, used in `transform.translation()`
    fn position(&self) -> Vec2 {
        match self {
            WallLocation::Left => Vec2::new(LEFT_WALL, 0.),
            WallLocation::Right => Vec2::new(RIGHT_WALL, 0.),
            WallLocation::Bottom => Vec2::new(0., BOTTOM_WALL),
            WallLocation::Top => Vec2::new(0., TOP_WALL),
        }
    }

    /// (x, y) dimensions of the wall, used in `transform.scale()`
    fn size(&self) -> Vec2 {
        let arena_height = TOP_WALL - BOTTOM_WALL;
        let arena_width = RIGHT_WALL - LEFT_WALL;
        // Make sure we haven't messed up our constants
        assert!(arena_height > 0.0);
        assert!(arena_width > 0.0);

        match self {
            WallLocation::Left | WallLocation::Right => {
                Vec2::new(WALL_THICKNESS, arena_height + WALL_THICKNESS)
            }
            WallLocation::Bottom | WallLocation::Top => {
                Vec2::new(arena_width + WALL_THICKNESS, WALL_THICKNESS)
            }
        }
    }
}
impl WallBundle {
    // This "builder method" allows us to reuse logic across our wall entities,
    // making our code easier to read and less prone to bugs when we change the logic
    pub fn new(location: WallLocation) -> WallBundle {
        WallBundle {
            sprite_bundle: SpriteBundle {
                transform: Transform {
                    // We need to convert our Vec2 into a Vec3, by giving it a z-coordinate
                    // This is used to determine the order of our sprites
                    translation: location.position().extend(0.0),
                    // The z-scale of 2D objects must always be 1.0,
                    // or their ordering will be affected in surprising ways.
                    // See https://github.com/bevyengine/bevy/issues/4149
                    scale: location.size().extend(1.0),
                    ..default()
                },
                sprite: Sprite {
                    color: WALL_COLOR,
                    ..default()
                },
                ..default()
            },
            collider: Collider,
        }
    }
}

// This resource tracks the game's score
#[derive(Resource)]
pub struct Score(pub u32);

#[derive(Component)]
pub struct ScoreboardUi;

#[repr(u8)]
pub enum NetKey {
    Left,
    Right,
}

#[derive(Deserialize, Serialize, Default, Clone)]
pub struct PlayerInputData {
    pub key_mask: u8,
    pub simulating_frame: u32,
    pub sequence: u32
}

#[derive(Deserialize, Serialize, Default, Clone)]
pub struct PingData {
    pub ping_id: u32,
}

#[derive(Deserialize, Serialize)]
pub enum ClientToServerPacket {
    Input(PlayerInputData),
    Ping(PingData)
}

#[derive(Deserialize, Serialize)]
pub struct NetPaddleData {
    pub pos: Vec2,
    pub player_index: NetPlayerIndex
}

#[derive(Deserialize, Serialize)]
pub struct NetBrickData {
    pub pos: Vec2
}

#[derive(Deserialize, Serialize)]
pub struct NetBallData {
    pub pos: Vec2,
    pub velocity: Vec2, // experimental for not predicting collisions
    pub player_index: NetPlayerIndex
}

#[derive(Deserialize, Serialize)]
pub struct NetScoreData {
    pub score: u32
}

#[derive(Deserialize, Serialize)]
pub enum NetEntityType {
    Paddle(NetPaddleData),
    Brick(NetBrickData),
    Ball(NetBallData),
    Score(NetScoreData),
}

#[derive(Component, Deserialize, Serialize, Clone, Copy, Hash, PartialEq, Eq)]
pub struct NetId(pub u16);

#[derive(Component, Deserialize, Serialize, Clone, Copy, Hash, PartialEq, Eq)]
pub struct NetPlayerIndex(pub u8);

#[derive(Deserialize, Serialize)]
pub struct NetEntity {
    pub entity_type: NetEntityType,
    pub net_id: NetId,
}

#[derive(Deserialize, Serialize, Default)]
pub struct NetWorldStateData {
    pub frame: u32,
    pub entities: Vec<NetEntity>,
}

#[derive(Deserialize, Serialize)]
pub enum ServerToClientPacket {
    WorldState(NetWorldStateData),
    Pong(PingData)
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum Collision {
    Left,
    Right,
    Top,
    Bottom,
}

// Returns `Some` if `ball` collides with `bounding_box`.
// The returned `Collision` is the side of `bounding_box` that `ball` hit.
fn ball_collision(ball: BoundingCircle, bounding_box: Aabb2d) -> Option<Collision> {
    if !ball.intersects(&bounding_box) {
        return None;
    }

    let closest = bounding_box.closest_point(ball.center());
    let offset = ball.center() - closest;
    let side = if offset.x.abs() > offset.y.abs() {
        if offset.x < 0. {
            Collision::Left
        } else {
            Collision::Right
        }
    } else if offset.y > 0. {
        Collision::Top
    } else {
        Collision::Bottom
    };

    Some(side)
}

#[derive(Resource, Default)]
pub struct FixedTickWorldResource {
    pub frame_counter: u32,
    pub tick_start: Option<time::Instant>
    //pub net_ids_removed_this_frame: Vec<NetId>
}

pub fn check_single_ball_collision(
    score: &mut ResMut<Score>,
    colliders: &Vec<(Entity, Transform, Option<Brick>)>,
    ball_transform: &Transform,
    ball_velocity: &mut Velocity,
    entities_to_delete: &mut Vec<Entity>,
) {
    for (collider_entity, collider_transform, maybe_brick) in colliders {
        if entities_to_delete.contains(&collider_entity) {
            continue;
        }
        let collision = ball_collision(
            BoundingCircle::new(ball_transform.translation.truncate(), BALL_DIAMETER / 2.),
            Aabb2d::new(
                collider_transform.translation.truncate(),
                collider_transform.scale.truncate() / 2.,
            ),
        );

        if let Some(collision) = collision {
            // Sends a collision event so that other systems can react to the collision
            //collision_events.send_default();

            // Bricks should be despawned and increment the scoreboard on collision
            if maybe_brick.is_some() {
                entities_to_delete.push(*collider_entity);
                score.0 += 1;
            }

            // Reflect the ball's velocity when it collides
            let mut reflect_x = false;
            let mut reflect_y = false;

            // Reflect only if the velocity is in the opposite direction of the collision
            // This prevents the ball from getting stuck inside the bar
            match collision {
                Collision::Left => reflect_x = ball_velocity.x > 0.0,
                Collision::Right => reflect_x = ball_velocity.x < 0.0,
                Collision::Top => reflect_y = ball_velocity.y < 0.0,
                Collision::Bottom => reflect_y = ball_velocity.y > 0.0,
            }

            // Reflect velocity on the x-axis if we hit something on the x-axis
            if reflect_x {
                ball_velocity.x = -ball_velocity.x;
            }

            // Reflect velocity on the y-axis if we hit something on the y-axis
            if reflect_y {
                ball_velocity.y = -ball_velocity.y;
            }
        }
    }
}


pub const PADDLE_SPEED: f32 = 500.0;
pub const PADDLE_PADDING: f32 = 10.0;
pub const PADDLE_LEFT_BOUND: f32 = LEFT_WALL + WALL_THICKNESS / 2.0 + PADDLE_SIZE.x / 2.0 + PADDLE_PADDING;
pub const PADDLE_RIGHT_BOUND: f32 = RIGHT_WALL - WALL_THICKNESS / 2.0 - PADDLE_SIZE.x / 2.0 - PADDLE_PADDING;

pub fn move_paddle(delta_seconds: f32, paddle_transform: &mut Transform, input: &PlayerInputData) {
    let buttons = input.key_mask;
    let mut direction = 0.0;
    if (buttons & (1 << NetKey::Left as u8)) != 0 {
        direction -= 1.0;
    }

    if (buttons & (1 << NetKey::Right as u8)) != 0{
        direction += 1.0;
    }

    // Calculate the new horizontal paddle position based on player input
    let new_paddle_position =
        paddle_transform.translation.x + direction * PADDLE_SPEED * delta_seconds;

    // Update the paddle position,
    // making sure it doesn't cause the paddle to leave the arena
    paddle_transform.translation.x = new_paddle_position.clamp(PADDLE_LEFT_BOUND, PADDLE_RIGHT_BOUND);
}

pub fn update_scoreboard(score: Res<Score>, mut query: Query<&mut Text, With<ScoreboardUi>>) {
    let mut text = query.single_mut();
    text.sections[1].value = score.0.to_string();
}

#[derive(Bundle)]
pub struct PaddleBundle {
    sprite_bundle: SpriteBundle,
    paddle: Paddle,
    collider: Collider,
    net_id: NetId,
    player: NetPlayerIndex
}

impl PaddleBundle {
    pub fn new(translation: Vec2, net_id: NetId, player: NetPlayerIndex) -> Self {
        PaddleBundle {
            sprite_bundle: SpriteBundle {
                transform: Transform {
                    translation: Vec3::from((translation, 0.0)),
                    scale: PADDLE_SIZE.extend(1.0),
                    ..default()
                },
                sprite: Sprite {
                    color: COLORS[player.0 as usize % COLORS.len()],
                    ..default()
                },
                ..default()
            },
            paddle: Paddle,
            collider: Collider,
            net_id,
            player
        }
    }
}

#[derive(Bundle)]
pub struct BallBundle {
    mesh_bundle: MaterialMesh2dBundle<ColorMaterial>,
    ball: Ball,
    velocity: Velocity,
    net_id: NetId,
    player: NetPlayerIndex
}

impl BallBundle {
    pub fn new(
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<ColorMaterial>,
        translation: Vec2,
        net_id: NetId,
        player: NetPlayerIndex) -> Self {
       BallBundle {
           mesh_bundle: MaterialMesh2dBundle {
               mesh: meshes.add(Circle::default()).into(),
               material: materials.add(COLORS[player.0 as usize % COLORS.len()]),
               transform: Transform::from_translation(Vec3::from((translation, 1.0)))
                   .with_scale(Vec2::splat(BALL_DIAMETER).extend(1.)),
               ..default()
           },
           ball: Ball,
           velocity: Velocity(INITIAL_BALL_DIRECTION.normalize() * BALL_SPEED),
           net_id,
           player
       }
    }
}

#[derive(Bundle)]
pub struct BrickBundle {
    sprite_bundle: SpriteBundle,
    brick: Brick,
    collider: Collider,
    net_id: NetId
}

impl BrickBundle {
    pub fn new(brick_position: Vec2, net_id: NetId) -> Self {
        BrickBundle {
            sprite_bundle: SpriteBundle {
                sprite: Sprite {
                    color: BRICK_COLOR,
                    ..default()
                },
                transform: Transform {
                    translation: brick_position.extend(0.0),
                    scale: Vec3::new(BRICK_SIZE.x, BRICK_SIZE.y, 1.0),
                    ..default()
                },
                ..default()
            },
            brick: Brick,
            collider: Collider,
            net_id
        }
    }
}

#[derive(Bundle)]
pub struct ScoreboardUiBundle {
    scoreboard_ui: ScoreboardUi,
    text_bundle: TextBundle,
}

impl ScoreboardUiBundle {
    pub fn new() -> Self {
        let text_bundle = TextBundle::from_sections([
            TextSection::new(
                "Score: ",
                TextStyle {
                    font_size: SCOREBOARD_FONT_SIZE,
                    color: TEXT_COLOR,
                    ..default()
                },
            ),
            TextSection::from_style(TextStyle {
                font_size: SCOREBOARD_FONT_SIZE,
                color: SCORE_COLOR,
                ..default()
            }),
        ])
        .with_style(Style {
            position_type: PositionType::Absolute,
            top: SCOREBOARD_TEXT_PADDING,
            left: SCOREBOARD_TEXT_PADDING,
            ..default()
        });

        ScoreboardUiBundle {
            scoreboard_ui: ScoreboardUi,
            text_bundle
        }
    }
}

pub fn start_tick(
    mut world_resource: ResMut<FixedTickWorldResource>
) {
    world_resource.frame_counter += 1;
    world_resource.tick_start = Some(time::Instant::now());
}

pub fn end_tick(
    world_resource: ResMut<FixedTickWorldResource>
) {
    debug!("tick time: {:?}", world_resource.tick_start.unwrap().elapsed());
}

#[derive(Args, Debug)]
pub struct SimLatencyArgs {
    #[arg(long, default_value_t = 0)]
    pub send_sim_latency_ms: u32,

    #[arg(long, default_value_t = 0)]
    pub send_jitter_stddev_ms: u32,

    #[arg(long, default_value_t = 0)]
    pub recv_sim_latency_ms: u32,

    #[arg(long, default_value_t = 0)]
    pub recv_jitter_stddev_ms: u32,
}

impl From<SimLatencyArgs> for networking::SimLatencySettings {
    fn from(value: SimLatencyArgs) -> Self {
        networking::SimLatencySettings {
            send: networking::SimLatencySetting {
                latency: networking::SimLatency {
                    base_ms: value.send_sim_latency_ms,
                    jitter_stddev_ms: value.send_jitter_stddev_ms
                },
                loss: networking::SimLoss {
                    loss_chance: 0.0
                }
            },
            receive: networking::SimLatencySetting {
                latency: networking::SimLatency {
                    base_ms: value.recv_sim_latency_ms,
                    jitter_stddev_ms: value.recv_jitter_stddev_ms
                },
                loss: networking::SimLoss {
                    loss_chance: 0.0
                }
            }
        }
    }
}

pub mod events;
mod message;
pub mod systems;
pub mod transport;

use std::collections::HashMap;
use std::ffi::c_void;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

pub use self::events::NetworkEvent;

#[allow(unused_imports)]
pub use self::transport::Transport;

use bevy::prelude::*;
use windows::Win32::Foundation;
use windows::Win32::Networking::WinSock;
use std::os::windows::io::AsRawSocket;

/// Defines how many times a client automatically sends a heartbeat packet.
/// This should be no more than half of idle_timeout.
pub const DEFAULT_HEARTBEAT_TICK_RATE_SECS: f32 = 2.;
/// Defines how long the server will wait until it sends
/// NetworkEvent::Disconnected
const DEFAULT_IDLE_TIMEOUT_SECS: f32 = 5.;

pub const ETHERNET_MTU: usize = 1500;

#[derive(Resource)]
pub struct NetworkResource {
    // Hashmap of each live connection and their last known packet activity
    pub connections: HashMap<SocketAddr, Duration>,
    pub idle_timeout: Duration,
}

impl Default for NetworkResource {
    fn default() -> Self {
        Self {
            connections: Default::default(),
            idle_timeout: Duration::from_secs_f32(DEFAULT_IDLE_TIMEOUT_SECS)
        }
    }
}

/// Label for network related systems.
#[derive(Clone, Hash, Debug, PartialEq, Eq, SystemSet)]
pub enum NetworkSystem {
    Receive,
    Send,
}

/// Label for server specific systems.
#[derive(Clone, Hash, Debug, PartialEq, Eq, SystemSet)]
pub enum ServerSystem {
    IdleTimeout,
}

/// Label for client specific systems.
#[derive(Clone, Hash, Debug, PartialEq, Eq, SystemSet)]
pub enum ClientSystem {
    Heartbeat,
}

#[derive(Default)]
pub struct ServerPlugin {
    pub sim_settings: transport::SimLatencySettings,
    pub no_systems: bool
}
impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(NetworkResource::default())
            .insert_resource(transport::Transport::new(self.sim_settings.clone()))
            .add_event::<events::NetworkEvent>();

        if !self.no_systems {
            app.add_systems(
                Update,
                (
                    systems::server_recv_packet_system.in_set(NetworkSystem::Receive),
                    systems::send_packet_system.in_set(NetworkSystem::Send),
                    systems::idle_timeout_system.in_set(ServerSystem::IdleTimeout)
                )
            );
        }
    }
}

#[derive(Resource)]
pub struct HeartbeatTimer(pub Timer);

#[derive(Default)]
pub struct ClientPlugin {
    pub sim_settings: transport::SimLatencySettings,
    pub no_systems: bool
}

#[derive(Resource)]
pub struct ResUdpSocket(pub UdpSocket);

impl ResUdpSocket {
    fn new(bind_addr: &str, remote_addr: Option<SocketAddr>) -> Self {
        let socket = ResUdpSocket(UdpSocket::bind(bind_addr).expect("could not bind socket"));
        //info!("UdpSocket bound to {}", socket.0.local_addr().unwrap());
        if let Some(r) = remote_addr {
            socket.0
                .connect(r)
                .expect("could not connect to server");
        }
        socket.0
            .set_nonblocking(true)
            .expect("could not set socket to be nonblocking");

        // We don't want windows to spam us with recv errors if a remote port is closed...
        // That spams logs and chokes the API, and is useless since we don't know which
        // client it's from anyways
        // SEE: https://github.com/mas-bandwidth/yojimbo/blob/b881662d72f21a171639fc6079052ce776cc9b2c/netcode/netcode.c#L519
        if cfg!(windows) {
            let win_socket = WinSock::SOCKET(socket.0.as_raw_socket().try_into().unwrap());
            let value: Foundation::BOOL = false.into();
            let value_ptr: Option<*const c_void> = Some(&value as *const _ as *const c_void);
            let mut bytes_returned: u32 = 0;
            let bytes_returned_ptr: *mut u32 = &mut bytes_returned;
            let ret_val = unsafe {
                WinSock::WSAIoctl(
                    win_socket,
                    WinSock::SIO_UDP_CONNRESET,
                    value_ptr,
                    size_of_val(&value) as u32,
                    None,
                    0,
                    bytes_returned_ptr,
                    None,
                    None
                )
            };
            if ret_val != 0 {
                warn!("Failed to disable udp connection reset");
            }
        }

        socket
    }
    pub fn new_client(remote_addr: SocketAddr) -> Self {
        Self::new("0.0.0.0:0", Some(remote_addr))
    }

    pub fn new_server(local_bind: &str) -> Self {
        Self::new(local_bind, None)
    }
}

#[derive(Resource)]
pub struct ResSocketAddr(pub(crate) SocketAddr);

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(transport::Transport::new(self.sim_settings.clone()))
            .insert_resource(HeartbeatTimer(Timer::from_seconds(
                DEFAULT_HEARTBEAT_TICK_RATE_SECS,
                TimerMode::Repeating,
            )))
            .add_event::<events::NetworkEvent>();

        if !self.no_systems {
            app.add_systems(
                Update,
                (
                    systems::client_recv_packet_system.in_set(NetworkSystem::Receive),
                    systems::send_packet_system.in_set(NetworkSystem::Send),
                    systems::auto_heartbeat_system.in_set(ClientSystem::Heartbeat)
                )
            );
        }
    }
}
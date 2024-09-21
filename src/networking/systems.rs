use std::{
    io
};

use bevy::prelude::*;
use bytes::Bytes;

use crate::networking::{HeartbeatTimer, ETHERNET_MTU};
use crate::networking::ResUdpSocket;
use crate::networking::ResSocketAddr;

use super::{events::NetworkEvent, transport::Transport, NetworkResource};

pub fn client_recv_packet_system(socket: Res<ResUdpSocket>, mut events: EventWriter<NetworkEvent>) {
    let mut recv_count = 0;
    loop {
        let mut buf = [0; ETHERNET_MTU];
        match socket.0.recv_from(&mut buf) {
            Ok((recv_len, address)) => {
                let payload = Bytes::copy_from_slice(&buf[..recv_len]);
                if payload.len() == 0 {
                    debug!("{}: received heartbeat packet", address);
                    // discard without sending a NetworkEvent
                    continue;
                }
                debug!("received payload {:?} from {}", payload, address);
                //info!("received payload from {}", address);
                events.send(NetworkEvent::Message(address, payload));
                recv_count += 1;
            }
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    events.send(NetworkEvent::RecvError(e));
                }

                // break loop when no messages are left to read this frame
                break;
            }
        }
    }
    //info!("{} msg this frame", recv_count);
}

pub fn server_recv_packet_system(
    time: Res<Time>,
    socket: Res<ResUdpSocket>,
    mut events: EventWriter<NetworkEvent>,
    mut net: ResMut<NetworkResource>,
) {
    loop {
        let mut buf = [0; ETHERNET_MTU];
        match socket.0.recv_from(&mut buf) {
            Ok((recv_len, address)) => {
                let payload = Bytes::copy_from_slice(&buf[..recv_len]);
                if net
                    .connections
                    .insert(address, time.elapsed())
                    .is_none()
                {
                    // connection established
                    events.send(NetworkEvent::Connected(address));
                }
                if payload.len() == 0 {
                    debug!("{}: received heartbeat packet", address);
                    // discard without sending a NetworkEvent
                    continue;
                }
                debug!("received payload {:?} from {}", payload, address);
                events.send(NetworkEvent::Message(address, payload));
            }
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    events.send(NetworkEvent::RecvError(e));
                }
                // break loop when no messages are left to read this frame
                break;
            }
        }
    }
}

pub fn send_packet_system(
    socket: Res<ResUdpSocket>,
    mut events: EventWriter<NetworkEvent>,
    mut transport: ResMut<Transport>,
) {
    let messages = transport.drain_messages_to_send(|_| true);
    for message in messages {
        if let Err(e) = socket.0.send_to(&message.payload, message.destination) {
            events.send(NetworkEvent::SendError(socket.0.peer_addr().unwrap(), e, message));
        }
    }
}

pub fn idle_timeout_system(
    time: Res<Time>,
    mut net: ResMut<NetworkResource>,
    mut events: EventWriter<NetworkEvent>,
) {
    let idle_timeout = net.idle_timeout.clone();
    net.connections.retain(|addr, last_update| {
        let reached_idle_timeout = time.elapsed() - *last_update > idle_timeout;
        if reached_idle_timeout {
            events.send(NetworkEvent::Disconnected(*addr));
        }
        !reached_idle_timeout
    });
}

pub fn auto_heartbeat_system(
    time: Res<Time>,
    mut timer: ResMut<HeartbeatTimer>,
    remote_addr: Res<ResSocketAddr>,
    mut transport: ResMut<Transport>,
) {
    if timer.0.tick(time.delta()).just_finished() {
        transport.send(remote_addr.0, Default::default());
    }
}

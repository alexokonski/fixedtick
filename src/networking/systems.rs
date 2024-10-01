use crate::networking::{SimLatencyRollResult, SimLatencySetting, SimLatencySettings};
use std::{io, time};
use std::collections::VecDeque;
use bevy::prelude::*;
use bytes::Bytes;

use crate::networking::{HeartbeatTimer, ETHERNET_MTU};
use crate::networking::ResUdpSocket;
use crate::networking::ResSocketAddr;

use super::{events::NetworkEvent, transport::Transport, NetworkResource, SimLatencyReceiveQueue};

fn send_with_sim_latency(
    receive_setting: &SimLatencySetting,
    events: &mut EventWriter<NetworkEvent>,
    queue: &mut SimLatencyReceiveQueue,
    event: NetworkEvent
) {
    match receive_setting.roll() {
        SimLatencyRollResult::NoOp => {
            events.send(event);
        },
        SimLatencyRollResult::Drop => {},
        SimLatencyRollResult::Delay(t) => {
            queue.sim_latency_delayed.push_back(event);

            let pos = queue.sim_latency_delivery_times.binary_search(&t).unwrap_or_else(|p| p);
            queue.sim_latency_delivery_times.insert(pos, t);
        }
    };
}

fn process_sim_latency(
    events: &mut EventWriter<NetworkEvent>,
    queue: &mut SimLatencyReceiveQueue,
) {
    let now = time::Instant::now();

    assert_eq!(queue.sim_latency_delayed.len(), queue.sim_latency_delivery_times.len());
    let delayed_events = &mut queue.sim_latency_delayed;
    let mut i = 0;
    while i != delayed_events.len() {
        if now >= queue.sim_latency_delivery_times[i] {
            events.send(delayed_events.remove(i).unwrap());
            queue.sim_latency_delivery_times.remove(i);
        } else {
            i += 1;
        }
    }
}

pub fn client_recv_packet_system(
    socket: Res<ResUdpSocket>,
    mut events: EventWriter<NetworkEvent>,
    mut queue: ResMut<SimLatencyReceiveQueue>,
    mut sim_settings: Res<SimLatencySettings>
) {
    //let mut recv_count = 0;
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

                //debug!("{:?} received payload {:?} from {}", time::Instant::now() payload, address);
                send_with_sim_latency(
                    &sim_settings.receive,
                    &mut events,
                    &mut queue,
                    NetworkEvent::Message(address, payload, time::Instant::now())
                );
                //events.send(NetworkEvent::Message(address, payload, time::Instant::now()));
                //recv_count += 1;
            }
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    //events.send(NetworkEvent::RecvError(e));
                    send_with_sim_latency(
                        &sim_settings.receive,
                        &mut events,
                        &mut queue,
                        NetworkEvent::RecvError(e)
                    );
                }

                // break loop when no messages are left to read this frame
                break;
            }
        }
    }
    //info!("{} msg this frame", recv_count);
    process_sim_latency(&mut events, &mut queue);
}

pub fn server_recv_packet_system(
    time: Res<Time>,
    socket: Res<ResUdpSocket>,
    mut events: EventWriter<NetworkEvent>,
    mut net: ResMut<NetworkResource>,
    mut queue: ResMut<SimLatencyReceiveQueue>,
    mut sim_settings: Res<SimLatencySettings>
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
                    //events.send(NetworkEvent::Connected(address));
                    send_with_sim_latency(
                        &sim_settings.receive,
                        &mut events,
                        &mut queue,
                        NetworkEvent::Connected(address)
                    );
                }
                if payload.len() == 0 {
                    debug!("{}: received heartbeat packet", address);
                    // discard without sending a NetworkEvent
                    continue;
                }
                let now = time::Instant::now();
                let msg = NetworkEvent::Message(address, payload, now);
                //debug!("{:?} received payload {:?} from {}", now, payload, address);
                send_with_sim_latency(
                    &sim_settings.receive,
                    &mut events,
                    &mut queue,
                    msg
                );
            }
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    send_with_sim_latency(
                        &sim_settings.receive,
                        &mut events,
                        &mut queue,
                        NetworkEvent::RecvError(e)
                    );
                }
                // break loop when no messages are left to read this frame
                break;
            }
        }
    }

    // Process sim latency
    process_sim_latency(&mut events, &mut queue);
}

pub fn send_packet_system(
    socket: Res<ResUdpSocket>,
    mut events: EventWriter<NetworkEvent>,
    mut transport: ResMut<Transport>,
) {
    let messages = transport.drain_messages_to_send(|_| true);
    for message in messages {
        debug!("{} Send packet {:?} at {:?}", message.destination, message.payload, time::Instant::now());
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

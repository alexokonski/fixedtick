use bevy::prelude::*;
use crate::common::*;
use std::net::SocketAddr;

use byteorder::ByteOrder;

use crate::server_types::*;

pub fn handle_client_disconnected(
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

pub fn write_header(buf: &mut [u8], conn: &NetConnection) {
    byteorder::NetworkEndian::write_u32(buf, WORLD_PACKET_HEADER_TAG);
    byteorder::NetworkEndian::write_u32(&mut buf[size_of::<u32>()..], conn.last_applied_input);
    buf[size_of::<u32>() * 2] = conn.player_index;
}

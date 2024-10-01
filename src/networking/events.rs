use std::{io, net::SocketAddr, time};

use bytes::Bytes;

use super::message::Message;

#[derive(bevy::prelude::Event)]
pub enum NetworkEvent {
    // A message was received from a client
    #[allow(dead_code)]
    Message(SocketAddr, Bytes, time::Instant),
    // A new client has connected to us
    #[allow(dead_code)]
    Connected(SocketAddr),
    // A client has disconnected from us
    #[allow(dead_code)]
    Disconnected(SocketAddr),
    // An error occurred while receiving a message
    #[allow(dead_code)]
    RecvError(io::Error),
    // An error occurred while sending a message
    #[allow(dead_code)]
    SendError(SocketAddr, io::Error, Message),
}

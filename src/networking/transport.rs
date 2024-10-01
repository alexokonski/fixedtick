use crate::networking::SimLatencyRollResult;
use crate::networking::SimLatencySetting;
use std::{collections::VecDeque, net::SocketAddr};

use super::message::Message;
use std::time;

use bevy::prelude::*;
use rand::Rng;
use rand_distr::{Normal, Distribution};

/// Resource serving as the owner of the queue of messages to be sent. This resource also serves
/// as the interface for other systems to send messages.
#[derive(bevy::prelude::Resource)]
pub struct Transport {
    messages: VecDeque<Message>,
    sim_send_times: VecDeque<time::Instant>, // parallel to messages
    sim_send_settings: SimLatencySetting,
}

impl Transport {
    /// Creates a new `Transport`.
    pub fn new(sim_send_settings: SimLatencySetting) -> Self {
        Self {
            messages: VecDeque::new(),
            sim_send_times: VecDeque::new(),
            sim_send_settings,
        }
    }

    /// Creates a `Message` with the default guarantees provided by the `Socket` implementation and
    /// pushes it onto the messages queue to be sent on the next frame.
    pub fn send(&mut self, destination: SocketAddr, payload: &[u8]) {
        match self.sim_send_settings.roll() {
            SimLatencyRollResult::NoOp => {},
            SimLatencyRollResult::Drop => return,
            SimLatencyRollResult::Delay(t) => {
                // Sort sim times from soonest to latest. This ensures we still send in order.
                // It does mean that the delay we just rolled won't necessarily be the one used for this
                // packet.
                let pos = self.sim_send_times.binary_search(&t).unwrap_or_else(|p| p);
                self.sim_send_times.insert(pos, t);
            }
        };

        let message = Message::new(destination, payload);
        self.messages.push_back(message);
    }

    /// Returns true if there are messages enqueued to be sent.
    #[allow(dead_code)]
    pub fn has_messages(&self) -> bool {
        !self.messages.is_empty()
    }

    /// Returns a reference to the owned messages.
    #[allow(dead_code)]
    pub fn get_messages(&self) -> &VecDeque<Message> {
        &self.messages
    }

    /// Drains the messages queue and returns the drained messages. The filter allows you to drain
    /// only messages that adhere to your filter. This might be useful in a scenario like draining
    /// messages with a particular urgency requirement.
    pub fn drain_messages_to_send(
        &mut self,
        mut filter: impl FnMut(&mut Message) -> bool,
    ) -> Vec<Message> {
        let using_send_sim = self.sim_send_settings.is_set();
        if using_send_sim {
            assert_eq!(self.messages.len(), self.sim_send_times.len());
        } else {
            assert!(self.sim_send_times.is_empty());
        }
        let mut drained = Vec::with_capacity(self.messages.len());
        let mut i = 0;
        let now = time::Instant::now();
        let sim_time_valid = |idx: usize, send_times: &VecDeque<time::Instant>| {
            if using_send_sim { now >= send_times[idx] } else { true }
        };

        while i != self.messages.len() {
            let msg = &mut self.messages[i];
            if sim_time_valid(i, &self.sim_send_times) && filter(msg) {
                if let Some(m) = self.messages.remove(i) {
                    drained.push(m);
                    if using_send_sim {
                        self.sim_send_times.remove(i);
                    }
                }
            } else {
                i += 1;
            }
        }
        drained
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            messages: VecDeque::new(),
            sim_send_settings: Default::default(),
            sim_send_times: VecDeque::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send() {
        let mut transport = create_test_transport();

        transport.send("127.0.0.1:3000".parse().unwrap(), test_payload());

        let packet = &transport.messages[0];

        assert_eq!(transport.messages.len(), 1);
        assert_eq!(packet.payload, test_payload());
    }

    #[test]
    fn test_has_messages() {
        let mut transport = create_test_transport();
        assert_eq!(transport.has_messages(), false);
        transport.send("127.0.0.1:3000".parse().unwrap(), test_payload());
        assert_eq!(transport.has_messages(), true);
    }

    #[test]
    fn test_drain_only_heartbeat_messages() {
        let mut transport = create_test_transport();

        let addr = "127.0.0.1:3000".parse().unwrap();
        transport.send(addr, test_payload());
        transport.send(addr, heartbeat_payload());
        transport.send(addr, test_payload());
        transport.send(addr, heartbeat_payload());
        transport.send(addr, test_payload());

        assert_eq!(
            transport
                .drain_messages_to_send(|m| m.payload == heartbeat_payload())
                .len(),
            2
        );
        // validate removal
        assert_eq!(
            transport
                .drain_messages_to_send(|m| m.payload == heartbeat_payload())
                .len(),
            0
        );
        assert_eq!(transport.drain_messages_to_send(|_| false).len(), 0);
        assert_eq!(transport.drain_messages_to_send(|_| true).len(), 3);
        // validate removal
        assert_eq!(transport.drain_messages_to_send(|_| true).len(), 0);
    }

    fn heartbeat_payload() -> &'static [u8] {
        b""
    }

    fn test_payload() -> &'static [u8] {
        b"test"
    }

    fn create_test_transport() -> Transport {
        Transport::new(SimLatencySetting::default())
    }
}

// Copyright 2019 Parity Technologies (UK) Ltd.
// Copyright 2019 Sigma Prime.
// This file is part of Parity Shasper.

// Parity Shasper is free software: you can redistribute it and/or modify it
// under the terms of the GNU General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option) any
// later version.

// Parity Shasper is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE.  See the GNU General Public License for more
// details.

// You should have received a copy of the GNU General Public License along with
// Parity Shasper.  If not, see <http://www.gnu.org/licenses/>.

use crate::behaviour::{Behaviour, BehaviourEvent, PubsubMessage};
use crate::config::*;
use crate::Error;
use crate::multiaddr::Protocol;
use crate::rpc::RPCEvent;
use crate::NetworkConfig;
use futures::prelude::*;
use futures::Stream;
use libp2p::core::{
    identity::Keypair,
    multiaddr::Multiaddr,
    muxing::StreamMuxerBox,
    nodes::Substream,
    transport::boxed::Boxed,
    upgrade::{InboundUpgradeExt, OutboundUpgradeExt},
};
use libp2p::{core, secio, PeerId, Swarm, Transport};
use libp2p::gossipsub::{Topic, TopicHash};
use log::*;
use std::time::Duration;

type Libp2pStream = Boxed<(PeerId, StreamMuxerBox), Error>;
type Libp2pBehaviour = Behaviour<Substream<StreamMuxerBox>>;

/// The configuration and state of the libp2p components for the beacon node.
pub struct Service {
    /// The libp2p Swarm handler.
    pub swarm: Swarm<Libp2pStream, Libp2pBehaviour>,
    /// This node's PeerId.
    pub local_peer_id: PeerId,
}

impl Service {
    pub fn new(config: NetworkConfig) -> Result<Self, Error> {
        trace!("Libp2p Service starting");

        // load the private key from CLI flag, disk or generate a new one
        let local_private_key = load_private_key();
        let local_peer_id = PeerId::from(local_private_key.public());
        info!("Libp2p Service {:?}", local_peer_id);

        let mut swarm = {
            // Set up the transport - tcp/ws with secio and mplex/yamux
            let transport = build_transport(local_private_key.clone());
            // Lighthouse network behaviour
            let behaviour = Behaviour::new(&local_private_key, &config)?;
            Swarm::new(transport, behaviour, local_peer_id.clone())
        };

        // listen on the specified address
        let listen_multiaddr = {
            let mut m = Multiaddr::from(config.listen_address);
            m.push(Protocol::Tcp(config.libp2p_port));
            m
        };

        match Swarm::listen_on(&mut swarm, listen_multiaddr.clone()) {
            Ok(_) => {
                let mut log_address = listen_multiaddr;
                log_address.push(Protocol::P2p(local_peer_id.clone().into()));
                info!("Listening established {}", log_address);
            }
            Err(err) => {
                warn!(
                    "Unable to listen on libp2p address {:?} {}",
					err,
                    listen_multiaddr,
                );
                return Err("Libp2p was unable to listen on the given listen address."
						   .to_string().into());
            }
        };

        // attempt to connect to user-input libp2p nodes
        for multiaddr in config.libp2p_nodes {
            match Swarm::dial_addr(&mut swarm, multiaddr.clone()) {
                Ok(()) => debug!("Dialing libp2p peer {}", multiaddr),
                Err(err) => debug!(
                    "Could not connect to peer {}, {:?}", multiaddr, err
                ),
            };
        }

        // subscribe to default gossipsub topics
        let mut topics = vec![];

        /* Here we subscribe to all the required gossipsub topics required for interop.
         * The topic builder adds the required prefix and postfix to the hardcoded topics that we
         * must subscribe to.
         */
        let topic_builder = |topic| {
            Topic::new(format!(
                "/{}/{}/{}",
                TOPIC_PREFIX, topic, TOPIC_ENCODING_POSTFIX,
            ))
        };
        topics.push(topic_builder(BEACON_BLOCK_TOPIC));
        topics.push(topic_builder(BEACON_ATTESTATION_TOPIC));
        topics.push(topic_builder(VOLUNTARY_EXIT_TOPIC));
        topics.push(topic_builder(PROPOSER_SLASHING_TOPIC));
        topics.push(topic_builder(ATTESTER_SLASHING_TOPIC));

        // Add any topics specified by the user
        topics.append(
            &mut config
                .topics
                .iter()
                .cloned()
                .map(|s| Topic::new(s))
                .collect(),
        );

        let mut subscribed_topics = vec![];
        for topic in topics {
            if swarm.subscribe(topic.clone()) {
                trace!("Subscribed to topic {}", topic);
                subscribed_topics.push(topic);
            } else {
                warn!("Could not subscribe to topic {}", topic);
            }
        }
        info!("Subscribed to topics {:?}", subscribed_topics.iter().map(|t| format!("{}", t)).collect::<Vec<String>>());

        Ok(Service {
            local_peer_id,
            swarm,
        })
    }
}

impl Stream for Service {
    type Item = Libp2pEvent;
    type Error = crate::error::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        loop {
            match self.swarm.poll() {
                //Behaviour events
                Ok(Async::Ready(Some(event))) => match event {
                    // TODO: Stub here for debugging
                    BehaviourEvent::GossipMessage {
                        source,
                        topics,
                        message,
                    } => {
                        trace!("Gossipsub message received (swarm)");
                        return Ok(Async::Ready(Some(Libp2pEvent::PubsubMessage {
                            source,
                            topics,
                            message,
                        })));
                    }
                    BehaviourEvent::RPC(peer_id, event) => {
                        return Ok(Async::Ready(Some(Libp2pEvent::RPC(peer_id, event))));
                    }
                    BehaviourEvent::PeerDialed(peer_id) => {
                        return Ok(Async::Ready(Some(Libp2pEvent::PeerDialed(peer_id))));
                    }
                    BehaviourEvent::PeerDisconnected(peer_id) => {
                        return Ok(Async::Ready(Some(Libp2pEvent::PeerDisconnected(peer_id))));
                    }
                },
                Ok(Async::Ready(None)) => unreachable!("Swarm stream shouldn't end"),
                Ok(Async::NotReady) => break,
                _ => break,
            }
        }
        Ok(Async::NotReady)
    }
}

/// The implementation supports TCP/IP, WebSockets over TCP/IP, secio as the encryption layer, and
/// mplex or yamux as the multiplexing layer.
fn build_transport(local_private_key: Keypair) -> Boxed<(PeerId, StreamMuxerBox), Error> {
    // TODO: The Wire protocol currently doesn't specify encryption and this will need to be customised
    // in the future.
    let transport = libp2p::tcp::TcpConfig::new();
    let transport = libp2p::dns::DnsConfig::new(transport);
    #[cfg(feature = "libp2p-websocket")]
    let transport = {
        let trans_clone = transport.clone();
        transport.or_transport(websocket::WsConfig::new(trans_clone))
    };
    transport
        .with_upgrade(secio::SecioConfig::new(local_private_key))
        .and_then(move |out, endpoint| {
            let peer_id = out.remote_key.into_peer_id();
            let peer_id2 = peer_id.clone();
            let upgrade = core::upgrade::SelectUpgrade::new(
                libp2p::yamux::Config::default(),
                libp2p::mplex::MplexConfig::new(),
            )
            // TODO: use a single `.map` instead of two maps
            .map_inbound(move |muxer| (peer_id, muxer))
            .map_outbound(move |muxer| (peer_id2, muxer));

            core::upgrade::apply(out.stream, upgrade, endpoint)
                .map(|(id, muxer)| (id, core::muxing::StreamMuxerBox::new(muxer)))
        })
        .with_timeout(Duration::from_secs(20))
        .map_err(|err| Error::Libp2p(Box::new(err)))
        .boxed()
}

/// Events that can be obtained from polling the Libp2p Service.
#[derive(Debug)]
pub enum Libp2pEvent {
    /// An RPC response request has been received on the swarm.
    RPC(PeerId, RPCEvent),
    /// Initiated the connection to a new peer.
    PeerDialed(PeerId),
    /// A peer has disconnected.
    PeerDisconnected(PeerId),
    /// Received pubsub message.
    PubsubMessage {
        source: PeerId,
        topics: Vec<TopicHash>,
        message: PubsubMessage,
    },
}

/// Loads a private key from disk. If this fails, a new key is
/// generated and is then saved to disk.
///
/// Currently only secp256k1 keys are allowed, as these are the only keys supported by discv5.
fn load_private_key() -> Keypair {
    // if a key could not be loaded from disk, generate a new one and save it
    let local_private_key = Keypair::generate_secp256k1();
    local_private_key
}

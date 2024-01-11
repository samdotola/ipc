// Copyright 2022-2023 Protocol Labs
// Copyright 2019-2022 ChainSafe Systems
// SPDX-License-Identifier: MIT
use std::{
    cmp,
    collections::VecDeque,
    task::{Context, Poll},
    time::Duration,
};

use libp2p::{
    core::Endpoint,
    identify::Info,
    kad::{self, store::MemoryStore},
    multiaddr::Protocol,
    swarm::{
        behaviour::toggle::{Toggle, ToggleConnectionHandler},
        derive_prelude::FromSwarm,
        ConnectionDenied, ConnectionId, NetworkBehaviour, THandler, THandlerInEvent,
        THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId, StreamProtocol,
};
use log::{debug, warn};
use tokio::time::Interval;

use crate::stats;

use super::NetworkConfig;

// NOTE: The Discovery behaviour is largely based on what exists in Forest. If it ain't broken...
// NOTE: Not sure if emitting events is going to be useful yet, but for now it's an example of having one.

/// Event generated by the `Discovery` behaviour.
#[derive(Debug)]
pub enum Event {
    /// Event emitted when a peer is added or updated in the routing table,
    /// which means if we later ask for its addresses, they should be known.
    Added(PeerId, Vec<Multiaddr>),

    /// Event emitted when a peer is removed from the routing table.
    Removed(PeerId),
}

/// Configuration for [`discovery::Behaviour`].
#[derive(Clone, Debug)]
pub struct Config {
    /// Custom nodes which never expire, e.g. bootstrap or reserved nodes.
    ///
    /// The addresses must end with a `/p2p/<peer-id>` part.
    pub static_addresses: Vec<Multiaddr>,
    /// Number of connections at which point we pause further discovery lookups.
    pub target_connections: usize,
    /// Option to disable Kademlia, for example in a fixed static network.
    pub enable_kademlia: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("invalid network: {0}")]
    InvalidNetwork(String),
    #[error("invalid bootstrap address: {0}")]
    InvalidBootstrapAddress(Multiaddr),
    #[error("no bootstrap address")]
    NoBootstrapAddress,
}

/// Discovery behaviour, periodically running a random lookup with Kademlia to find new peers.
///
/// Our other option for peer discovery would be to rely on the Peer Exchange of Gossipsub.
/// However, the required Signed Records feature is not available in the Rust version of the library, as of v0.50.
pub struct Behaviour {
    /// Local peer ID.
    peer_id: PeerId,
    /// User-defined list of nodes and their addresses.
    /// Typically includes bootstrap nodes, or it can be used for a static network.
    static_addresses: Vec<(PeerId, Multiaddr)>,
    /// Name of the peer discovery protocol.
    protocol_name: StreamProtocol,
    /// Kademlia behaviour, if enabled.
    inner: Toggle<kad::Behaviour<MemoryStore>>,
    /// Number of current connections.
    num_connections: usize,
    /// Number of connections where further lookups are paused.
    target_connections: usize,
    /// Interval between random lookups.
    lookup_interval: Interval,
    /// Buffer incoming identify requests until we have finished the bootstrap.
    bootstrap_buffer: Option<Vec<(PeerId, Info)>>,
    /// Events to return when polled.
    outbox: VecDeque<Event>,
}

impl Behaviour {
    /// Create a [`discovery::Behaviour`] from the configuration.
    pub fn new(nc: NetworkConfig, dc: Config) -> Result<Self, ConfigError> {
        if nc.network_name.is_empty() {
            return Err(ConfigError::InvalidNetwork(nc.network_name));
        }

        // Parse static addresses.
        let mut static_addresses = Vec::new();
        for multiaddr in dc.static_addresses {
            let mut addr = multiaddr.clone();
            if let Some(Protocol::P2p(peer_id)) = addr.pop() {
                static_addresses.push((peer_id, addr))
            } else {
                return Err(ConfigError::InvalidBootstrapAddress(multiaddr));
            }
        }

        let mut outbox = VecDeque::new();
        let protocol_name = format!("/ipc/{}/kad/1.0.0", nc.network_name);
        let protocol_name =
            StreamProtocol::try_from_owned(protocol_name).expect("valid protocol name");

        let mut bootstrap_buffer = None;

        let kademlia_opt = if dc.enable_kademlia {
            let mut kad_config = kad::Config::default();
            kad_config.set_protocol_names(vec![protocol_name.clone()]);

            // Disable inserting records into the memory store, so peers cannot send `PutRecord`
            // messages to store content in the memory of our node.
            kad_config.set_record_filtering(kad::StoreInserts::FilterBoth);

            let store = MemoryStore::new(nc.local_peer_id());

            let mut kademlia = kad::Behaviour::with_config(nc.local_peer_id(), store, kad_config);

            // Setting the mode to server so that it doesn't deny connections until the external address is established.
            // At least this seems to prevent in-memory tests from working, I'm not sure about what will happen with real servers.
            kademlia.set_mode(Some(kad::Mode::Server));

            // Bootstrap from the seeds. The first seed to stand up might have nobody to bootstrap from,
            // although ideally there would be at least another peer, so we can easily restart it and come back.
            if !static_addresses.is_empty() {
                for (peer_id, addr) in static_addresses.iter() {
                    kademlia.add_address(peer_id, addr.clone());
                }
                kademlia
                    .bootstrap()
                    .map_err(|_| ConfigError::NoBootstrapAddress)?;

                bootstrap_buffer = Some(Vec::new());
            }

            Some(kademlia)
        } else {
            // It would be nice to use `.group_by` here but it's unstable.
            // Make sure static peers are reported as routable.
            for (peer_id, addr) in static_addresses.iter() {
                outbox.push_back(Event::Added(*peer_id, vec![addr.clone()]))
            }
            None
        };

        Ok(Self {
            peer_id: nc.local_peer_id(),
            static_addresses,
            protocol_name,
            inner: kademlia_opt.into(),
            lookup_interval: tokio::time::interval(Duration::from_secs(1)),
            outbox,
            num_connections: 0,
            bootstrap_buffer,
            target_connections: dc.target_connections,
        })
    }

    /// Lookup a peer, unless we already know their address, so that we have a chance to connect to them later.
    pub fn background_lookup(&mut self, peer_id: PeerId) {
        if self.addresses_of_peer(peer_id).is_empty() {
            if let Some(kademlia) = self.inner.as_mut() {
                stats::DISCOVERY_BACKGROUND_LOOKUP.inc();
                kademlia.get_closest_peers(peer_id);
            }
        }
    }

    /// Check if a peer has a user defined addresses.
    fn is_static(&self, peer_id: PeerId) -> bool {
        self.static_addresses.iter().any(|(id, _)| *id == peer_id)
    }

    /// Add addresses we learned from the `Identify` protocol to Kademlia.
    ///
    /// This seems to be the only way, because Kademlia rightfully treats
    /// incoming connections as ephemeral addresses, but doesn't have an
    /// alternative exchange mechanism.
    pub fn add_identified(&mut self, peer_id: &PeerId, info: Info) {
        if info.protocols.contains(&self.protocol_name) {
            // If we are still in the process of bootstrapping peers, buffer the incoming self-identify records,
            // to protect against eclipse attacks that could fill the k-table with entries to crowd out honest peers.
            if let Some(buffer) = self.bootstrap_buffer.as_mut() {
                if buffer.len() < self.target_connections
                    && !buffer.iter().any(|(id, _)| id == peer_id)
                {
                    buffer.push((*peer_id, info))
                }
            } else {
                for addr in info.listen_addrs.iter().cloned() {
                    self.add_address(peer_id, addr);
                }
            }
        }
    }

    /// Add a known address to Kademlia.
    pub fn add_address(&mut self, peer_id: &PeerId, address: Multiaddr) {
        if let Some(kademlia) = self.inner.as_mut() {
            kademlia.add_address(peer_id, address);
        }
    }

    fn addresses_of_peer(&mut self, peer_id: PeerId) -> Vec<Multiaddr> {
        self.handle_pending_outbound_connection(
            ConnectionId::new_unchecked(0),
            Some(peer_id),
            &[],
            Endpoint::Listener,
        )
        .unwrap_or_default()
    }
}

impl NetworkBehaviour for Behaviour {
    type ConnectionHandler = ToggleConnectionHandler<
        <kad::Behaviour<MemoryStore> as NetworkBehaviour>::ConnectionHandler,
    >;

    type ToSwarm = Event;

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match &event {
            FromSwarm::ConnectionEstablished(e) => {
                if e.other_established == 0 {
                    stats::DISCOVERY_CONNECTED_PEERS.inc();
                    self.num_connections += 1;
                }
            }
            FromSwarm::ConnectionClosed(e) => {
                if e.remaining_established == 0 {
                    stats::DISCOVERY_CONNECTED_PEERS.dec();
                    self.num_connections -= 1;
                }
            }
            _ => {}
        };
        self.inner.on_swarm_event(event)
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.inner
            .on_connection_handler_event(peer_id, connection_id, event)
    }

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.inner
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        )
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        let mut addrs = self.inner.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?;

        if let Some(peer_id) = maybe_peer {
            addrs.extend(
                self.static_addresses
                    .iter()
                    .filter(|(p, _)| *p == peer_id)
                    .map(|(_, a)| a.clone()),
            );
        }

        Ok(addrs)
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner
            .handle_established_outbound_connection(connection_id, peer, addr, role_override)
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        // Emit own events first.
        if let Some(ev) = self.outbox.pop_front() {
            return Poll::Ready(ToSwarm::GenerateEvent(ev));
        }

        // Trigger periodic queries.
        if self.lookup_interval.poll_tick(cx).is_ready() {
            if self.num_connections < self.target_connections {
                if let Some(k) = self.inner.as_mut() {
                    debug!("looking up a random peer");
                    let random_peer_id = PeerId::random();
                    k.get_closest_peers(random_peer_id);
                }
            }

            // Schedule the next random query with exponentially increasing delay, capped at 60 seconds.
            self.lookup_interval = tokio::time::interval(cmp::min(
                self.lookup_interval.period() * 2,
                Duration::from_secs(60),
            ));
            // we need to reset the interval, otherwise the next tick completes immediately.
            self.lookup_interval.reset();
        }

        // Poll Kademlia.
        while let Poll::Ready(ev) = self.inner.poll(cx) {
            match ev {
                ToSwarm::GenerateEvent(ev) => {
                    match ev {
                        // We get this event for inbound connections, where the remote address may be ephemeral.
                        kad::Event::UnroutablePeer { peer } => {
                            debug!("{peer} unroutable from {}", self.peer_id);
                        }
                        kad::Event::InboundRequest {
                            request: kad::InboundRequest::PutRecord { source, .. },
                        } => {
                            warn!("disallowed Kademlia requests from {source}",)
                        }
                        // Information only.
                        kad::Event::InboundRequest { .. } => {}
                        kad::Event::ModeChanged { .. } => {}
                        // Finish bootstrapping.
                        kad::Event::OutboundQueryProgressed { result, step, .. } => match result {
                            kad::QueryResult::Bootstrap(result) if step.last => {
                                debug!("Bootstrapping finished with {result:?}");
                                if let Some(buffer) = self.bootstrap_buffer.take() {
                                    debug!("Adding {} self-identified peers.", buffer.len());
                                    for (peer_id, info) in buffer {
                                        self.add_identified(&peer_id, info)
                                    }
                                }
                            }
                            _ => {}
                        },
                        // The config ensures peers are added to the table if there's room.
                        // We're not emitting these as known peers because the address will probably not be returned by `addresses_of_peer`,
                        // so the outside service would have to keep track, which is not what we want.
                        kad::Event::RoutablePeer { peer, .. } => {
                            debug!("Kademlia in manual mode or bucket full, cannot add {peer}");
                        }
                        // Unfortunately, looking at the Kademlia behaviour, it looks like when it goes from pending to active,
                        // it won't emit another event, so we might as well tentatively emit an event here.
                        kad::Event::PendingRoutablePeer { peer, address } => {
                            debug!("{peer} pending to the routing table of {}", self.peer_id);
                            self.outbox.push_back(Event::Added(peer, vec![address]))
                        }
                        // This event should ensure that we will be able to answer address lookups later.
                        kad::Event::RoutingUpdated {
                            peer,
                            addresses,
                            old_peer,
                            ..
                        } => {
                            debug!("{peer} added to the routing table of {}", self.peer_id);
                            // There are two events here; we can only return one, so let's defer them to the outbox.
                            if let Some(peer_id) = old_peer {
                                if self.is_static(peer_id) {
                                    self.outbox.push_back(Event::Removed(peer_id))
                                }
                            }
                            self.outbox
                                .push_back(Event::Added(peer, addresses.into_vec()))
                        }
                    }
                }
                other => {
                    return Poll::Ready(other.map_out(|_| unreachable!("already handled")));
                }
            }
        }

        Poll::Pending
    }
}

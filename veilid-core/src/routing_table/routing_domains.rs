use super::*;

/// Mechanism required to contact another node
#[derive(Clone, Debug)]
pub(crate) enum ContactMethod {
    /// Node is not reachable by any means
    Unreachable,
    /// Connection should have already existed
    Existing,
    /// Contact the node directly
    Direct(DialInfo),
    /// Request via signal the node connect back directly (relay, target)
    SignalReverse(TypedKey, TypedKey),
    /// Request via signal the node negotiate a hole punch (relay, target)
    SignalHolePunch(TypedKey, TypedKey),
    /// Must use an inbound relay to reach the node
    InboundRelay(TypedKey),
    /// Must use outbound relay to reach the node
    OutboundRelay(TypedKey),
}

#[derive(Debug)]
pub(crate) struct RoutingDomainDetailCommon {
    routing_domain: RoutingDomain,
    network_class: Option<NetworkClass>,
    outbound_protocols: ProtocolTypeSet,
    inbound_protocols: ProtocolTypeSet,
    address_types: AddressTypeSet,
    relay_node: Option<NodeRef>,
    relay_node_last_keepalive: Option<Timestamp>,
    capabilities: Vec<Capability>,
    dial_info_details: Vec<DialInfoDetail>,
    // caches
    cached_peer_info: Mutex<Option<PeerInfo>>,
}

impl RoutingDomainDetailCommon {
    pub fn new(routing_domain: RoutingDomain) -> Self {
        Self {
            routing_domain,
            network_class: Default::default(),
            outbound_protocols: Default::default(),
            inbound_protocols: Default::default(),
            address_types: Default::default(),
            relay_node: Default::default(),
            relay_node_last_keepalive: Default::default(),
            capabilities: Default::default(),
            dial_info_details: Default::default(),
            cached_peer_info: Mutex::new(Default::default()),
        }
    }

    // Set from network manager
    pub(super) fn setup_network(
        &mut self,
        outbound_protocols: ProtocolTypeSet,
        inbound_protocols: ProtocolTypeSet,
        address_types: AddressTypeSet,
        capabilities: Vec<Capability>,
    ) {
        self.outbound_protocols = outbound_protocols;
        self.inbound_protocols = inbound_protocols;
        self.address_types = address_types;
        self.capabilities = capabilities;
        self.clear_cache();
    }

    pub(super) fn set_network_class(&mut self, network_class: Option<NetworkClass>) {
        self.network_class = network_class;
        self.clear_cache();
    }
    pub fn network_class(&self) -> Option<NetworkClass> {
        self.network_class
    }
    pub fn outbound_protocols(&self) -> ProtocolTypeSet {
        self.outbound_protocols
    }
    pub fn inbound_protocols(&self) -> ProtocolTypeSet {
        self.inbound_protocols
    }
    pub fn address_types(&self) -> AddressTypeSet {
        self.address_types
    }
    pub fn capabilities(&self) -> Vec<Capability> {
        self.capabilities.clone()
    }
    pub fn relay_node(&self) -> Option<NodeRef> {
        self.relay_node.clone()
    }
    pub fn relay_node_last_keepalive(&self) -> Option<Timestamp> {
        self.relay_node_last_keepalive
    }
    pub(super) fn set_relay_node(&mut self, opt_relay_node: Option<NodeRef>) {
        self.relay_node = opt_relay_node.map(|nr| {
            nr.filtered_clone(NodeRefFilter::new().with_routing_domain(self.routing_domain))
        });
        self.relay_node_last_keepalive = None;
        self.clear_cache();
    }
    pub(super) fn set_relay_node_last_keepalive(&mut self, ts: Option<Timestamp>) {
        self.relay_node_last_keepalive = ts;
    }
    pub fn dial_info_details(&self) -> &Vec<DialInfoDetail> {
        &self.dial_info_details
    }
    pub(super) fn clear_dial_info_details(
        &mut self,
        address_type: Option<AddressType>,
        protocol_type: Option<ProtocolType>,
    ) {
        self.dial_info_details.retain_mut(|e| {
            let mut remove = true;
            if let Some(pt) = protocol_type {
                if pt != e.dial_info.protocol_type() {
                    remove = false;
                }
            }
            if let Some(at) = address_type {
                if at != e.dial_info.address_type() {
                    remove = false;
                }
            }
            !remove
        });
        self.clear_cache();
    }
    pub(super) fn add_dial_info_detail(&mut self, did: DialInfoDetail) {
        self.dial_info_details.push(did);
        self.dial_info_details.sort();
        self.clear_cache();
    }

    pub fn has_valid_network_class(&self) -> bool {
        self.network_class.unwrap_or(NetworkClass::Invalid) != NetworkClass::Invalid
    }

    fn make_peer_info(&self, rti: &RoutingTableInner) -> PeerInfo {
        let node_info = NodeInfo::new(
            self.network_class.unwrap_or(NetworkClass::Invalid),
            self.outbound_protocols,
            self.address_types,
            VALID_ENVELOPE_VERSIONS.to_vec(),
            VALID_CRYPTO_KINDS.to_vec(),
            self.capabilities.clone(),
            self.dial_info_details.clone(),
        );

        let relay_info = if let Some(rn) = &self.relay_node {
            let opt_relay_pi = rn.locked(rti).make_peer_info(self.routing_domain);
            if let Some(relay_pi) = opt_relay_pi {
                let (relay_ids, relay_sni) = relay_pi.destructure();
                match relay_sni {
                    SignedNodeInfo::Direct(d) => Some((relay_ids, d)),
                    SignedNodeInfo::Relayed(_) => {
                        warn!("relay node should not have a relay itself! if this happens, a relay updated its signed node info and became a relay, which should cause the relay to be dropped");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let signed_node_info = match relay_info {
            Some((relay_ids, relay_sdni)) => SignedNodeInfo::Relayed(
                SignedRelayedNodeInfo::make_signatures(
                    rti.unlocked_inner.crypto(),
                    rti.unlocked_inner.node_id_typed_key_pairs(),
                    node_info,
                    relay_ids,
                    relay_sdni,
                )
                .unwrap(),
            ),
            None => SignedNodeInfo::Direct(
                SignedDirectNodeInfo::make_signatures(
                    rti.unlocked_inner.crypto(),
                    rti.unlocked_inner.node_id_typed_key_pairs(),
                    node_info,
                )
                .unwrap(),
            ),
        };

        PeerInfo::new(rti.unlocked_inner.node_ids(), signed_node_info)
    }

    pub fn with_peer_info<F, R>(&self, rti: &RoutingTableInner, f: F) -> R
    where
        F: FnOnce(&PeerInfo) -> R,
    {
        let mut cpi = self.cached_peer_info.lock();
        if cpi.is_none() {
            // Regenerate peer info
            let pi = self.make_peer_info(rti);

            // Cache the peer info
            *cpi = Some(pi);
        }
        f(cpi.as_ref().unwrap())
    }

    #[allow(dead_code)]
    pub fn inbound_dial_info_filter(&self) -> DialInfoFilter {
        DialInfoFilter::all()
            .with_protocol_type_set(self.inbound_protocols)
            .with_address_type_set(self.address_types)
    }
    pub fn outbound_dial_info_filter(&self) -> DialInfoFilter {
        DialInfoFilter::all()
            .with_protocol_type_set(self.outbound_protocols)
            .with_address_type_set(self.address_types)
    }

    pub(super) fn clear_cache(&self) {
        *self.cached_peer_info.lock() = None;
    }
}

/// General trait for all routing domains
pub(crate) trait RoutingDomainDetail {
    // Common accessors
    fn common(&self) -> &RoutingDomainDetailCommon;
    fn common_mut(&mut self) -> &mut RoutingDomainDetailCommon;

    /// Can this routing domain contain a particular address
    fn can_contain_address(&self, address: Address) -> bool;

    /// Get the contact method required for node A to reach node B in this routing domain
    /// Routing table must be locked for reading to use this function
    fn get_contact_method(
        &self,
        rti: &RoutingTableInner,
        peer_a: &PeerInfo,
        peer_b: &PeerInfo,
        dial_info_filter: DialInfoFilter,
        sequencing: Sequencing,
        dif_sort: Option<Arc<DialInfoDetailSort>>,
    ) -> ContactMethod;
}

/////////////////////////////////////////////////////////////////////////////////////////////////////////////////

/// Public Internet routing domain internals
#[derive(Debug)]
pub struct PublicInternetRoutingDomainDetail {
    /// Common implementation for all routing domains
    common: RoutingDomainDetailCommon,
}

impl Default for PublicInternetRoutingDomainDetail {
    fn default() -> Self {
        Self {
            common: RoutingDomainDetailCommon::new(RoutingDomain::PublicInternet),
        }
    }
}

fn first_filtered_dial_info_detail_between_nodes(
    from_node: &NodeInfo,
    to_node: &NodeInfo,
    dial_info_filter: &DialInfoFilter,
    sequencing: Sequencing,
    dif_sort: Option<Arc<DialInfoDetailSort>>,
) -> Option<DialInfoDetail> {
    // Consider outbound capabilities
    let dial_info_filter = (*dial_info_filter).filtered(
        &DialInfoFilter::all()
            .with_address_type_set(from_node.address_types())
            .with_protocol_type_set(from_node.outbound_protocols()),
    );

    // Apply sequencing and get sort
    // Include sorting by external dial info sort for rotating through dialinfo
    // based on an external preference table, for example the one kept by
    // AddressFilter to deprioritize dialinfo that have recently failed to connect
    let (ordered, dial_info_filter) = dial_info_filter.with_sequencing(sequencing);
    let sort: Option<Box<DialInfoDetailSort>> = if ordered {
        if let Some(dif_sort) = dif_sort {
            Some(Box::new(move |a, b| {
                let mut ord = dif_sort(a, b);
                if ord == core::cmp::Ordering::Equal {
                    ord = DialInfoDetail::ordered_sequencing_sort(a, b);
                }
                ord
            }))
        } else {
            Some(Box::new(move |a, b| {
                DialInfoDetail::ordered_sequencing_sort(a, b)
            }))
        }
    } else if let Some(dif_sort) = dif_sort {
        Some(Box::new(move |a, b| dif_sort(a, b)))
    } else {
        None
    };

    // If the filter is dead then we won't be able to connect
    if dial_info_filter.is_dead() {
        return None;
    }

    // Get the best match dial info for node B if we have it
    let direct_filter = |did: &DialInfoDetail| did.matches_filter(&dial_info_filter);
    to_node.first_filtered_dial_info_detail(sort, direct_filter)
}

impl RoutingDomainDetail for PublicInternetRoutingDomainDetail {
    fn common(&self) -> &RoutingDomainDetailCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut RoutingDomainDetailCommon {
        &mut self.common
    }
    fn can_contain_address(&self, address: Address) -> bool {
        address.is_global()
    }
    fn get_contact_method(
        &self,
        rti: &RoutingTableInner,
        peer_a: &PeerInfo,
        peer_b: &PeerInfo,
        dial_info_filter: DialInfoFilter,
        sequencing: Sequencing,
        dif_sort: Option<Arc<DialInfoDetailSort>>,
    ) -> ContactMethod {
        let ip6_prefix_size = rti
            .unlocked_inner
            .config
            .get()
            .network
            .max_connections_per_ip6_prefix_size as usize;

        // Get the nodeinfos for convenience
        let node_a = peer_a.signed_node_info().node_info();
        let node_b = peer_b.signed_node_info().node_info();

        // Check to see if these nodes are on the same network
        let same_ipblock = node_a.node_is_on_same_ipblock(node_b, ip6_prefix_size);

        // Get the node ids that would be used between these peers
        let cck = common_crypto_kinds(&peer_a.node_ids().kinds(), &peer_b.node_ids().kinds());
        let Some(best_ck) = cck.first().copied() else {
            // No common crypto kinds between these nodes, can't contact
            return ContactMethod::Unreachable;
        };

        //let node_a_id = peer_a.node_ids().get(best_ck).unwrap();
        let node_b_id = peer_b.node_ids().get(best_ck).unwrap();

        // Get the best match dial info for node B if we have it
        // Don't try direct inbound at all if the two nodes are on the same ipblock to avoid hairpin NAT issues
        // as well avoiding direct traffic between same-network nodes. This would be done in the LocalNetwork RoutingDomain.
        if let Some(target_did) = (!same_ipblock)
            .then(|| {
                first_filtered_dial_info_detail_between_nodes(
                    node_a,
                    node_b,
                    &dial_info_filter,
                    sequencing,
                    dif_sort.clone(),
                )
            })
            .flatten()
        {
            // Do we need to signal before going inbound?
            if !target_did.class.requires_signal() {
                // Go direct without signaling
                return ContactMethod::Direct(target_did.dial_info);
            }

            // Get the target's inbound relay, it must have one or it is not reachable
            if let Some(node_b_relay) = peer_b.signed_node_info().relay_info() {
                // Note that relay_peer_info could be node_a, in which case a connection already exists
                // and we only get here if the connection had dropped, in which case node_a is unreachable until
                // it gets a new relay connection up
                if peer_b
                    .signed_node_info()
                    .relay_ids()
                    .contains_any(peer_a.node_ids())
                {
                    return ContactMethod::Existing;
                }

                // Get best node id to contact relay with
                let Some(node_b_relay_id) = peer_b.signed_node_info().relay_ids().get(best_ck)
                else {
                    // No best relay id
                    return ContactMethod::Unreachable;
                };

                // Can node A reach the inbound relay directly?
                if first_filtered_dial_info_detail_between_nodes(
                    node_a,
                    node_b_relay,
                    &dial_info_filter,
                    sequencing,
                    dif_sort.clone(),
                )
                .is_some()
                {
                    // Can node A receive anything inbound ever?
                    if matches!(node_a.network_class(), NetworkClass::InboundCapable) {
                        ///////// Reverse connection

                        // Get the best match dial info for an reverse inbound connection from node B to node A
                        if let Some(reverse_did) = first_filtered_dial_info_detail_between_nodes(
                            node_b,
                            node_a,
                            &dial_info_filter,
                            sequencing,
                            dif_sort.clone(),
                        ) {
                            // Ensure we aren't on the same public IP address (no hairpin nat)
                            if reverse_did.dial_info.ip_addr() != target_did.dial_info.ip_addr() {
                                // Can we receive a direct reverse connection?
                                if !reverse_did.class.requires_signal() {
                                    return ContactMethod::SignalReverse(
                                        node_b_relay_id,
                                        node_b_id,
                                    );
                                }
                            }
                        }

                        ///////// UDP hole-punch

                        // Does node B have a direct udp dialinfo node A can reach?
                        let udp_dial_info_filter = dial_info_filter
                            .filtered(&DialInfoFilter::all().with_protocol_type(ProtocolType::UDP));
                        if let Some(target_udp_did) = first_filtered_dial_info_detail_between_nodes(
                            node_a,
                            node_b,
                            &udp_dial_info_filter,
                            sequencing,
                            dif_sort.clone(),
                        ) {
                            // Does node A have a direct udp dialinfo that node B can reach?
                            if let Some(reverse_udp_did) =
                                first_filtered_dial_info_detail_between_nodes(
                                    node_b,
                                    node_a,
                                    &udp_dial_info_filter,
                                    sequencing,
                                    dif_sort.clone(),
                                )
                            {
                                // Ensure we aren't on the same public IP address (no hairpin nat)
                                if reverse_udp_did.dial_info.ip_addr()
                                    != target_udp_did.dial_info.ip_addr()
                                {
                                    // The target and ourselves have a udp dialinfo that they can reach
                                    return ContactMethod::SignalHolePunch(
                                        node_b_relay_id,
                                        node_b_id,
                                    );
                                }
                            }
                        }
                        // Otherwise we have to inbound relay
                    }

                    return ContactMethod::InboundRelay(node_b_relay_id);
                }
            }
        }
        // If the node B has no direct dial info or is on the same ipblock, it needs to have an inbound relay
        else if let Some(node_b_relay) = peer_b.signed_node_info().relay_info() {
            // Note that relay_peer_info could be node_a, in which case a connection already exists
            // and we only get here if the connection had dropped, in which case node_b is unreachable until
            // it gets a new relay connection up
            if peer_b
                .signed_node_info()
                .relay_ids()
                .contains_any(peer_a.node_ids())
            {
                return ContactMethod::Existing;
            }

            // Get best node id to contact relay with
            let Some(node_b_relay_id) = peer_b.signed_node_info().relay_ids().get(best_ck) else {
                // No best relay id
                return ContactMethod::Unreachable;
            };

            // Can we reach the inbound relay?
            if first_filtered_dial_info_detail_between_nodes(
                node_a,
                node_b_relay,
                &dial_info_filter,
                sequencing,
                dif_sort.clone(),
            )
            .is_some()
            {
                ///////// Reverse connection

                // Get the best match dial info for an reverse inbound connection from node B to node A
                // unless both nodes are on the same ipblock
                if let Some(reverse_did) = (!same_ipblock)
                    .then(|| {
                        first_filtered_dial_info_detail_between_nodes(
                            node_b,
                            node_a,
                            &dial_info_filter,
                            sequencing,
                            dif_sort.clone(),
                        )
                    })
                    .flatten()
                {
                    // Can we receive a direct reverse connection?
                    if !reverse_did.class.requires_signal() {
                        return ContactMethod::SignalReverse(node_b_relay_id, node_b_id);
                    }
                }

                return ContactMethod::InboundRelay(node_b_relay_id);
            }
        }

        // If node A can't reach the node by other means, it may need to use its outbound relay
        if peer_a
            .signed_node_info()
            .node_info()
            .network_class()
            .outbound_wants_relay()
        {
            if let Some(node_a_relay_id) = peer_a.signed_node_info().relay_ids().get(best_ck) {
                // Ensure it's not our relay we're trying to reach
                if node_a_relay_id != node_b_id {
                    return ContactMethod::OutboundRelay(node_a_relay_id);
                }
            }
        }

        ContactMethod::Unreachable
    }
}

/// Local Network routing domain internals
#[derive(Debug)]
pub struct LocalNetworkRoutingDomainDetail {
    /// The local networks this domain will communicate with
    local_networks: Vec<(IpAddr, IpAddr)>,
    /// Common implementation for all routing domains
    common: RoutingDomainDetailCommon,
}

impl Default for LocalNetworkRoutingDomainDetail {
    fn default() -> Self {
        Self {
            local_networks: Default::default(),
            common: RoutingDomainDetailCommon::new(RoutingDomain::LocalNetwork),
        }
    }
}

impl LocalNetworkRoutingDomainDetail {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn set_local_networks(&mut self, mut local_networks: Vec<(IpAddr, IpAddr)>) -> bool {
        local_networks.sort();
        if local_networks == self.local_networks {
            return false;
        }
        self.local_networks = local_networks;
        true
    }
}

impl RoutingDomainDetail for LocalNetworkRoutingDomainDetail {
    fn common(&self) -> &RoutingDomainDetailCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut RoutingDomainDetailCommon {
        &mut self.common
    }
    fn can_contain_address(&self, address: Address) -> bool {
        let ip = address.ip_addr();
        for localnet in &self.local_networks {
            if ipaddr_in_network(ip, localnet.0, localnet.1) {
                return true;
            }
        }
        false
    }

    fn get_contact_method(
        &self,
        _rti: &RoutingTableInner,
        peer_a: &PeerInfo,
        peer_b: &PeerInfo,
        dial_info_filter: DialInfoFilter,
        sequencing: Sequencing,
        dif_sort: Option<Arc<DialInfoDetailSort>>,
    ) -> ContactMethod {
        // Get the nodeinfos for convenience
        let node_a = peer_a.signed_node_info().node_info();
        let node_b = peer_b.signed_node_info().node_info();

        // Get the node ids that would be used between these peers
        let cck = common_crypto_kinds(&peer_a.node_ids().kinds(), &peer_b.node_ids().kinds());
        let Some(_best_ck) = cck.first().copied() else {
            // No common crypto kinds between these nodes, can't contact
            return ContactMethod::Unreachable;
        };

        if let Some(target_did) = first_filtered_dial_info_detail_between_nodes(
            node_a,
            node_b,
            &dial_info_filter,
            sequencing,
            dif_sort,
        ) {
            return ContactMethod::Direct(target_did.dial_info);
        }

        ContactMethod::Unreachable
    }
}

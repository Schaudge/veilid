use super::*;

impl RoutingTable {
    /// Utility to find all closest nodes to a particular key, including possibly our own node and nodes further away from the key than our own, returning their peer info
    pub fn find_all_closest_peers(&self, key: TypedKey) -> NetworkResult<Vec<PeerInfo>> {
        let Some(own_peer_info) = self.get_own_peer_info(RoutingDomain::PublicInternet) else {
            // Our own node info is not yet available, drop this request.
            return NetworkResult::service_unavailable();
        };

        // find N nodes closest to the target node in our routing table
        let filter = Box::new(
            move |rti: &RoutingTableInner, opt_entry: Option<Arc<BucketEntry>>| {
                // Ensure only things that are valid/signed in the PublicInternet domain are returned
                rti.filter_has_valid_signed_node_info(
                    RoutingDomain::PublicInternet,
                    true,
                    opt_entry,
                )
            },
        ) as RoutingTableEntryFilter;
        let filters = VecDeque::from([filter]);

        let node_count = {
            let c = self.config.get();
            c.network.dht.max_find_node_count as usize
        };

        let closest_nodes = self.find_closest_nodes(
            node_count,
            key,
            filters,
            // transform
            |rti, entry| {
                rti.transform_to_peer_info(RoutingDomain::PublicInternet, &own_peer_info, entry)
            },
        );

        NetworkResult::value(closest_nodes)
    }

    /// Utility to find nodes that are closer to a key than our own node, returning their peer info
    pub fn find_peers_closer_to_key(&self, key: TypedKey) -> NetworkResult<Vec<PeerInfo>> {
        // add node information for the requesting node to our routing table
        let crypto_kind = key.kind;
        let own_node_id = self.node_id(crypto_kind);

        // find N nodes closest to the target node in our routing table
        // ensure the nodes returned are only the ones closer to the target node than ourself
        let Some(vcrypto) = self.crypto().get(crypto_kind) else {
            return NetworkResult::invalid_message("unsupported cryptosystem");
        };
        let own_distance = vcrypto.distance(&own_node_id.value, &key.value);

        let filter = Box::new(
            move |rti: &RoutingTableInner, opt_entry: Option<Arc<BucketEntry>>| {
                // Exclude our own node
                let Some(entry) = opt_entry else {
                    return false;
                };
                // Ensure only things that are valid/signed in the PublicInternet domain are returned
                if !rti.filter_has_valid_signed_node_info(
                    RoutingDomain::PublicInternet,
                    true,
                    Some(entry.clone()),
                ) {
                    return false;
                }
                // Ensure things further from the key than our own node are not included
                let Some(entry_node_id) = entry.with(rti, |_rti, e| e.node_ids().get(crypto_kind)) else {
                    return false;
                };
                let entry_distance = vcrypto.distance(&entry_node_id.value, &key.value);
                if entry_distance >= own_distance {
                    return false;
                }

                true
            },
        ) as RoutingTableEntryFilter;
        let filters = VecDeque::from([filter]);

        let node_count = {
            let c = self.config.get();
            c.network.dht.max_find_node_count as usize
        };

        //
        let closest_nodes = self.find_closest_nodes(
            node_count,
            key,
            filters,
            // transform
            |rti, entry| {
                entry.unwrap().with(rti, |_rti, e| {
                    e.make_peer_info(RoutingDomain::PublicInternet).unwrap()
                })
            },
        );

        NetworkResult::value(closest_nodes)
    }
}

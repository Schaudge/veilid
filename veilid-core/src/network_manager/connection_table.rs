use super::*;
use futures_util::StreamExt;
use hashlink::LruCache;

///////////////////////////////////////////////////////////////////////////////
#[derive(ThisError, Debug)]
pub(in crate::network_manager) enum ConnectionTableAddError {
    #[error("Connection already added to table")]
    AlreadyExists(NetworkConnection),
    #[error("Connection address was filtered")]
    AddressFilter(NetworkConnection, AddressFilterError),
    #[error("Connection table is full")]
    TableFull(NetworkConnection),
}

impl ConnectionTableAddError {
    pub fn already_exists(conn: NetworkConnection) -> Self {
        ConnectionTableAddError::AlreadyExists(conn)
    }
    pub fn address_filter(conn: NetworkConnection, err: AddressFilterError) -> Self {
        ConnectionTableAddError::AddressFilter(conn, err)
    }
    pub fn table_full(conn: NetworkConnection) -> Self {
        ConnectionTableAddError::TableFull(conn)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ConnectionRefKind {
    AddRef,
    RemoveRef,
}

///////////////////////////////////////////////////////////////////////////////

#[derive(Debug)]
struct ConnectionTableInner {
    max_connections: Vec<usize>,
    conn_by_id: Vec<LruCache<NetworkConnectionId, NetworkConnection>>,
    protocol_index_by_id: BTreeMap<NetworkConnectionId, usize>,
    id_by_flow: BTreeMap<Flow, NetworkConnectionId>,
    ids_by_remote: BTreeMap<PeerAddress, Vec<NetworkConnectionId>>,
    address_filter: AddressFilter,
}

#[derive(Debug)]
pub(in crate::network_manager) struct ConnectionTable {
    inner: Arc<Mutex<ConnectionTableInner>>,
}

impl ConnectionTable {
    pub fn new(config: VeilidConfig, address_filter: AddressFilter) -> Self {
        let max_connections = {
            let c = config.get();
            vec![
                c.network.protocol.tcp.max_connections as usize,
                c.network.protocol.ws.max_connections as usize,
                c.network.protocol.wss.max_connections as usize,
            ]
        };
        Self {
            inner: Arc::new(Mutex::new(ConnectionTableInner {
                max_connections,
                conn_by_id: vec![
                    LruCache::new_unbounded(),
                    LruCache::new_unbounded(),
                    LruCache::new_unbounded(),
                ],
                protocol_index_by_id: BTreeMap::new(),
                id_by_flow: BTreeMap::new(),
                ids_by_remote: BTreeMap::new(),
                address_filter,
            })),
        }
    }

    fn protocol_to_index(protocol: ProtocolType) -> usize {
        match protocol {
            ProtocolType::TCP => 0,
            ProtocolType::WS => 1,
            ProtocolType::WSS => 2,
            ProtocolType::UDP => panic!("not a connection-oriented protocol"),
        }
    }

    fn index_to_protocol(idx: usize) -> ProtocolType {
        match idx {
            0 => ProtocolType::TCP,
            1 => ProtocolType::WS,
            2 => ProtocolType::WSS,
            _ => panic!("not a connection-oriented protocol"),
        }
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn join(&self) {
        let mut unord = {
            let mut inner = self.inner.lock();
            let unord = FuturesUnordered::new();
            for table in &mut inner.conn_by_id {
                for (_, mut v) in table.drain() {
                    log_net!("connection table join: {:?}", v);
                    v.close();
                    unord.push(v);
                }
            }
            inner.protocol_index_by_id.clear();
            inner.id_by_flow.clear();
            inner.ids_by_remote.clear();
            unord
        };

        while unord.next().await.is_some() {}
    }

    // Return true if there is another connection in the table using a different protocol type
    // to the same address and port with the same low level protocol type.
    // Specifically right now this checks for a TCP connection that exists to the same
    // low level TCP remote as a WS or WSS connection, since they are all low-level TCP
    #[instrument(level = "trace", skip(self), ret)]
    pub fn check_for_colliding_connection(&self, dial_info: &DialInfo) -> bool {
        let inner = self.inner.lock();

        let protocol_type = dial_info.protocol_type();
        let low_level_protocol_type = protocol_type.low_level_protocol_type();

        // check protocol types
        let mut check_protocol_types = ProtocolTypeSet::empty();
        for check_pt in ProtocolTypeSet::all().iter() {
            if check_pt != protocol_type
                && check_pt.low_level_protocol_type() == low_level_protocol_type
            {
                check_protocol_types.insert(check_pt);
            }
        }
        let socket_address = dial_info.socket_address();

        for check_pt in check_protocol_types {
            let check_pa = PeerAddress::new(socket_address, check_pt);
            if inner.ids_by_remote.contains_key(&check_pa) {
                return true;
            }
        }
        false
    }

    #[instrument(level = "trace", skip(self), ret)]
    pub fn add_connection(
        &self,
        network_connection: NetworkConnection,
    ) -> Result<Option<NetworkConnection>, ConnectionTableAddError> {
        // Get indices for network connection table
        let id = network_connection.connection_id();
        let flow = network_connection.flow();
        let protocol_index = Self::protocol_to_index(flow.protocol_type());
        let remote = flow.remote();

        let mut inner = self.inner.lock();

        // Two connections to the same flow should be rejected (soft rejection)
        if inner.id_by_flow.contains_key(&flow) {
            return Err(ConnectionTableAddError::already_exists(network_connection));
        }

        // Sanity checking this implementation (hard fails that would invalidate the representation)
        if inner.conn_by_id[protocol_index].contains_key(&id) {
            panic!("duplicate connection id: {:#?}", network_connection);
        }
        if inner.protocol_index_by_id.contains_key(&id) {
            panic!("duplicate id to protocol index: {:#?}", network_connection);
        }
        if let Some(ids) = inner.ids_by_remote.get(&flow.remote()) {
            if ids.contains(&id) {
                panic!("duplicate id by remote: {:#?}", network_connection);
            }
        }

        // Filter by ip for connection limits
        let ip_addr = flow.remote_address().ip_addr();
        match inner.address_filter.add_connection(ip_addr) {
            Ok(()) => {}
            Err(e) => {
                // Return the connection in the error to be disposed of
                return Err(ConnectionTableAddError::address_filter(
                    network_connection,
                    e,
                ));
            }
        };

        // if we have reached the maximum number of connections per protocol type
        // then drop the least recently used connection that is not protected or referenced
        let mut out_conn = None;
        if inner.conn_by_id[protocol_index].len() >= inner.max_connections[protocol_index] {
            // Find a free connection to terminate to make room
            let dead_k = {
                let Some(lruk) = inner.conn_by_id[protocol_index].iter().find_map(|(k, v)| {
                    if !v.is_in_use() && v.protected_node_ref().is_none() {
                        Some(*k)
                    } else {
                        None
                    }
                }) else {
                    // Can't make room, connection table is full
                    return Err(ConnectionTableAddError::table_full(network_connection));
                };
                lruk
            };

            let dead_conn = Self::remove_connection_records(&mut inner, dead_k);
            out_conn = Some(dead_conn);
        }

        // Add the connection to the table
        let res = inner.conn_by_id[protocol_index].insert(id, network_connection);
        assert!(res.is_none());

        // add connection records
        inner.protocol_index_by_id.insert(id, protocol_index);
        inner.id_by_flow.insert(flow, id);
        inner.ids_by_remote.entry(remote).or_default().push(id);

        Ok(out_conn)
    }

    //#[instrument(level = "trace", skip(self), ret)]
    pub fn peek_connection_by_flow(&self, flow: Flow) -> Option<ConnectionHandle> {
        if flow.protocol_type() == ProtocolType::UDP {
            return None;
        }

        let inner = self.inner.lock();

        let id = *inner.id_by_flow.get(&flow)?;
        let protocol_index = Self::protocol_to_index(flow.protocol_type());
        let out = inner.conn_by_id[protocol_index].peek(&id).unwrap();
        Some(out.get_handle())
    }

    //#[instrument(level = "trace", skip(self), ret)]
    pub fn touch_connection_by_id(&self, id: NetworkConnectionId) {
        let mut inner = self.inner.lock();
        let Some(protocol_index) = inner.protocol_index_by_id.get(&id).copied() else {
            return;
        };
        let _ = inner.conn_by_id[protocol_index].get(&id).unwrap();
    }

    //#[instrument(level = "trace", skip(self), ret)]
    pub fn ref_connection_by_id(
        &self,
        id: NetworkConnectionId,
        ref_type: ConnectionRefKind,
    ) -> bool {
        let mut inner = self.inner.lock();
        let Some(protocol_index) = inner.protocol_index_by_id.get(&id).copied() else {
            // Sometimes network connections die before we can ref/unref them
            return false;
        };
        let out = inner.conn_by_id[protocol_index].get_mut(&id).unwrap();
        match ref_type {
            ConnectionRefKind::AddRef => out.add_ref(),
            ConnectionRefKind::RemoveRef => out.remove_ref(),
        }
        true
    }

    // #[instrument(level = "trace", skip(self), ret)]
    pub fn get_best_connection_by_remote(
        &self,
        best_port: Option<u16>,
        remote: PeerAddress,
    ) -> Option<ConnectionHandle> {
        let inner = &mut *self.inner.lock();

        let all_ids_by_remote = inner.ids_by_remote.get(&remote)?;
        let protocol_index = Self::protocol_to_index(remote.protocol_type());
        if all_ids_by_remote.is_empty() {
            // no connections
            return None;
        }
        if all_ids_by_remote.len() == 1 {
            // only one connection
            let id = all_ids_by_remote[0];
            let nc = inner.conn_by_id[protocol_index].get(&id).unwrap();
            return Some(nc.get_handle());
        }
        // multiple connections, find the one that matches the best port, or the most recent
        if let Some(best_port) = best_port {
            for id in all_ids_by_remote {
                let nc = inner.conn_by_id[protocol_index].peek(id).unwrap();
                if let Some(local_addr) = nc.flow().local() {
                    if local_addr.port() == best_port {
                        let nc = inner.conn_by_id[protocol_index].get(id).unwrap();
                        return Some(nc.get_handle());
                    }
                }
            }
        }
        // just return most recent network connection if a best port match can not be found
        let best_id = *all_ids_by_remote.last().unwrap();
        let nc = inner.conn_by_id[protocol_index].get(&best_id).unwrap();
        Some(nc.get_handle())
    }

    //#[instrument(level = "trace", skip(self), ret)]
    #[allow(dead_code)]
    pub fn get_connection_ids_by_remote(&self, remote: PeerAddress) -> Vec<NetworkConnectionId> {
        let inner = self.inner.lock();
        inner
            .ids_by_remote
            .get(&remote)
            .cloned()
            .unwrap_or_default()
    }

    // pub fn drain_filter<F>(&self, mut filter: F) -> Vec<NetworkConnection>
    // where
    //     F: FnMut(Flow) -> bool,
    // {
    //     let mut inner = self.inner.lock();
    //     let mut filtered_ids = Vec::new();
    //     for cbi in &mut inner.conn_by_id {
    //         for (id, conn) in cbi {
    //             if filter(conn.flow()) {
    //                 filtered_ids.push(*id);
    //             }
    //         }
    //     }
    //     let mut filtered_connections = Vec::new();
    //     for id in filtered_ids {
    //         let conn = Self::remove_connection_records(&mut *inner, id);
    //         filtered_connections.push(conn)
    //     }
    //     filtered_connections
    // }

    pub fn connection_count(&self) -> usize {
        let inner = self.inner.lock();
        inner.conn_by_id.iter().fold(0, |acc, c| acc + c.len())
    }

    #[instrument(level = "trace", skip(inner), ret)]
    fn remove_connection_records(
        inner: &mut ConnectionTableInner,
        id: NetworkConnectionId,
    ) -> NetworkConnection {
        // protocol_index_by_id
        let protocol_index = inner.protocol_index_by_id.remove(&id).unwrap();
        // conn_by_id
        let conn = inner.conn_by_id[protocol_index].remove(&id).unwrap();
        // id_by_flow
        let flow = conn.flow();
        inner.id_by_flow.remove(&flow).unwrap();
        // ids_by_remote
        let remote = flow.remote();
        let ids = inner.ids_by_remote.get_mut(&remote).unwrap();
        for (n, elem) in ids.iter().enumerate() {
            if *elem == id {
                ids.remove(n);
                if ids.is_empty() {
                    inner.ids_by_remote.remove(&remote).unwrap();
                }
                break;
            }
        }
        // address_filter
        let ip_addr = remote.socket_addr().ip();
        inner
            .address_filter
            .remove_connection(ip_addr)
            .expect("Inconsistency in connection table");
        conn
    }

    #[instrument(level = "trace", skip(self), ret)]
    pub fn remove_connection_by_id(&self, id: NetworkConnectionId) -> Option<NetworkConnection> {
        let mut inner = self.inner.lock();

        let protocol_index = *inner.protocol_index_by_id.get(&id)?;
        if !inner.conn_by_id[protocol_index].contains_key(&id) {
            return None;
        }
        let conn = Self::remove_connection_records(&mut inner, id);
        Some(conn)
    }

    pub fn debug_print_table(&self) -> String {
        let mut out = String::new();
        let inner = self.inner.lock();
        let cur_ts = Timestamp::now();
        for t in 0..inner.conn_by_id.len() {
            out += &format!(
                "  {} Connections: ({}/{})\n",
                Self::index_to_protocol(t),
                inner.conn_by_id[t].len(),
                inner.max_connections[t]
            );

            for (_, conn) in &inner.conn_by_id[t] {
                out += &format!("    {}\n", conn.debug_print(cur_ts));
            }
        }
        out
    }
}

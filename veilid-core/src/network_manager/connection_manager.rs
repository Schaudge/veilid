use super::*;
pub(crate) use connection_table::ConnectionRefKind;
use connection_table::*;
use network_connection::*;
use stop_token::future::FutureExt;

///////////////////////////////////////////////////////////
// Connection manager

#[derive(Debug)]
enum ConnectionManagerEvent {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Accepted(ProtocolNetworkConnection),
    Dead(NetworkConnection),
}

#[derive(Debug)]
pub(crate) struct ConnectionRefScope {
    connection_manager: ConnectionManager,
    id: NetworkConnectionId,
}

impl ConnectionRefScope {
    pub fn try_new(connection_manager: ConnectionManager, id: NetworkConnectionId) -> Option<Self> {
        if !connection_manager.connection_ref(id, ConnectionRefKind::AddRef) {
            return None;
        }
        Some(Self {
            connection_manager,
            id,
        })
    }
}

impl Drop for ConnectionRefScope {
    fn drop(&mut self) {
        self.connection_manager
            .connection_ref(self.id, ConnectionRefKind::RemoveRef);
    }
}

#[derive(Debug)]
struct ConnectionManagerInner {
    next_id: NetworkConnectionId,
    sender: flume::Sender<ConnectionManagerEvent>,
    async_processor_jh: Option<MustJoinHandle<()>>,
    stop_source: Option<StopSource>,
}

struct ConnectionManagerArc {
    network_manager: NetworkManager,
    connection_initial_timeout_ms: u32,
    connection_inactivity_timeout_ms: u32,
    connection_table: ConnectionTable,
    address_lock_table: AsyncTagLockTable<SocketAddr>,
    startup_lock: StartupLock,
    inner: Mutex<Option<ConnectionManagerInner>>,
}
impl core::fmt::Debug for ConnectionManagerArc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConnectionManagerArc")
            .field("inner", &self.inner)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ConnectionManager {
    arc: Arc<ConnectionManagerArc>,
}

impl ConnectionManager {
    fn new_inner(
        stop_source: StopSource,
        sender: flume::Sender<ConnectionManagerEvent>,
        async_processor_jh: MustJoinHandle<()>,
    ) -> ConnectionManagerInner {
        ConnectionManagerInner {
            next_id: 0.into(),
            stop_source: Some(stop_source),
            sender,
            async_processor_jh: Some(async_processor_jh),
        }
    }
    fn new_arc(network_manager: NetworkManager) -> ConnectionManagerArc {
        let config = network_manager.config();
        let (connection_initial_timeout_ms, connection_inactivity_timeout_ms) = {
            let c = config.get();
            (
                c.network.connection_initial_timeout_ms,
                c.network.connection_inactivity_timeout_ms,
            )
        };
        let address_filter = network_manager.address_filter();

        ConnectionManagerArc {
            network_manager,
            connection_initial_timeout_ms,
            connection_inactivity_timeout_ms,
            connection_table: ConnectionTable::new(config, address_filter),
            address_lock_table: AsyncTagLockTable::new(),
            startup_lock: StartupLock::new(),
            inner: Mutex::new(None),
        }
    }
    pub fn new(network_manager: NetworkManager) -> Self {
        Self {
            arc: Arc::new(Self::new_arc(network_manager)),
        }
    }

    pub fn network_manager(&self) -> NetworkManager {
        self.arc.network_manager.clone()
    }

    pub fn connection_inactivity_timeout_ms(&self) -> u32 {
        self.arc.connection_inactivity_timeout_ms
    }

    pub async fn startup(&self) -> EyreResult<()> {
        let guard = self.arc.startup_lock.startup()?;

        log_net!(debug "startup connection manager");

        let mut inner = self.arc.inner.lock();
        if inner.is_some() {
            panic!("shouldn't start connection manager twice without shutting it down first");
        }

        // Create channel for async_processor to receive notifications of networking events
        let (sender, receiver) = flume::unbounded();

        // Create the stop source we'll use to stop the processor and the connection table
        let stop_source = StopSource::new();

        // Spawn the async processor
        let async_processor = spawn(
            "connection manager async processor",
            self.clone().async_processor(stop_source.token(), receiver),
        );

        // Store in the inner object
        *inner = Some(Self::new_inner(stop_source, sender, async_processor));

        guard.success();

        Ok(())
    }

    pub async fn shutdown(&self) {
        log_net!(debug "starting connection manager shutdown");
        let Ok(guard) = self.arc.startup_lock.shutdown().await else {
            log_net!(debug "connection manager is already shut down");
            return;
        };

        // Remove the inner from the lock
        let mut inner = {
            let mut inner_lock = self.arc.inner.lock();
            match inner_lock.take() {
                Some(v) => v,
                None => {
                    panic!("not started");
                }
            }
        };

        // Stop all the connections and the async processor
        log_net!(debug "stopping async processor task");
        drop(inner.stop_source.take());
        let async_processor_jh = inner.async_processor_jh.take().unwrap();
        // wait for the async processor to stop
        log_net!(debug "waiting for async processor to stop");
        async_processor_jh.await;
        // Wait for the connections to complete
        log_net!(debug "waiting for connection handlers to complete");
        self.arc.connection_table.join().await;

        guard.success();
        log_net!(debug "finished connection manager shutdown");
    }

    // Internal routine to see if we should keep this connection
    // from being LRU removed. Used on our initiated relay connections.
    fn should_protect_connection(&self, conn: &NetworkConnection) -> Option<NodeRef> {
        let netman = self.network_manager();
        let routing_table = netman.routing_table();
        let remote_address = conn.flow().remote_address().address();
        let routing_domain = routing_table.routing_domain_for_address(remote_address)?;
        let relay_node = routing_table.relay_node(routing_domain)?;
        let relay_nr = relay_node.filtered_clone(
            NodeRefFilter::new()
                .with_routing_domain(routing_domain)
                .with_address_type(conn.flow().address_type())
                .with_protocol_type(conn.flow().protocol_type()),
        );
        let dids = relay_nr.all_filtered_dial_info_details();
        for did in dids {
            if did.dial_info.address() == remote_address {
                return Some(relay_nr);
            }
        }
        None
    }

    // Internal routine to register new connection atomically.
    // Registers connection in the connection table for later access
    // and spawns a message processing loop for the connection
    //#[instrument(level = "trace", skip(self, inner), ret, err)]
    fn on_new_protocol_network_connection(
        &self,
        inner: &mut ConnectionManagerInner,
        prot_conn: ProtocolNetworkConnection,
    ) -> EyreResult<NetworkResult<ConnectionHandle>> {
        // Get next connection id to use
        let id = inner.next_id;
        inner.next_id += 1u64;
        log_net!(
            "on_new_protocol_network_connection: id={} prot_conn={:?}",
            id,
            prot_conn
        );

        // Wrap with NetworkConnection object to start the connection processing loop
        let stop_token = match &inner.stop_source {
            Some(ss) => ss.token(),
            None => bail!("not creating connection because we are stopping"),
        };

        let mut conn = NetworkConnection::from_protocol(self.clone(), stop_token, prot_conn, id);
        let handle = conn.get_handle();

        // See if this should be a protected connection
        if let Some(protect_nr) = self.should_protect_connection(&conn) {
            log_net!(debug "== PROTECTING connection: {} -> {} for node {}", id, conn.debug_print(Timestamp::now()), protect_nr);
            conn.protect(protect_nr);
        }

        // Add to the connection table
        match self.arc.connection_table.add_connection(conn) {
            Ok(None) => {
                // Connection added
            }
            Ok(Some(conn)) => {
                // Connection added and a different one LRU'd out
                // Send it to be terminated
                log_net!(debug "== LRU kill connection due to limit: {:?}", conn.debug_print(Timestamp::now()));
                let _ = inner.sender.send(ConnectionManagerEvent::Dead(conn));
            }
            Err(ConnectionTableAddError::AddressFilter(conn, e)) => {
                // Connection filtered
                let desc = conn.flow();
                let _ = inner.sender.send(ConnectionManagerEvent::Dead(conn));
                return Ok(NetworkResult::no_connection_other(format!(
                    "connection filtered: {:?} ({})",
                    desc, e
                )));
            }
            Err(ConnectionTableAddError::AlreadyExists(conn)) => {
                // Connection already exists
                let desc = conn.flow();
                log_net!(debug "== Connection already exists: {:?}", conn.debug_print(Timestamp::now()));
                let _ = inner.sender.send(ConnectionManagerEvent::Dead(conn));
                return Ok(NetworkResult::no_connection_other(format!(
                    "connection already exists: {:?}",
                    desc
                )));
            }
            Err(ConnectionTableAddError::TableFull(conn)) => {
                // Connection table is full
                let desc = conn.flow();
                log_net!(debug "== Connection table full: {:?}", conn.debug_print(Timestamp::now()));
                let _ = inner.sender.send(ConnectionManagerEvent::Dead(conn));
                return Ok(NetworkResult::no_connection_other(format!(
                    "connection table is full: {:?}",
                    desc
                )));
            }
        };
        Ok(NetworkResult::Value(handle))
    }

    // Returns a network connection if one already is established
    pub fn get_connection(&self, flow: Flow) -> Option<ConnectionHandle> {
        let Ok(_guard) = self.arc.startup_lock.enter() else {
            return None;
        };
        self.arc.connection_table.peek_connection_by_flow(flow)
    }

    // Returns a network connection if one already is established
    pub(super) fn touch_connection_by_id(&self, id: NetworkConnectionId) {
        self.arc.connection_table.touch_connection_by_id(id)
    }

    // Protects a network connection if one already is established
    fn connection_ref(&self, id: NetworkConnectionId, kind: ConnectionRefKind) -> bool {
        self.arc.connection_table.ref_connection_by_id(id, kind)
    }
    pub fn try_connection_ref_scope(&self, id: NetworkConnectionId) -> Option<ConnectionRefScope> {
        let Ok(_guard) = self.arc.startup_lock.enter() else {
            return None;
        };
        ConnectionRefScope::try_new(self.clone(), id)
    }

    /// Called when we want to create a new connection or get the current one that already exists
    /// This will kill off any connections that are in conflict with the new connection to be made
    /// in order to make room for the new connection in the system's connection table
    /// This routine needs to be atomic, or connections may exist in the table that are not established
    //#[instrument(level = "trace", skip(self), ret, err)]
    pub async fn get_or_create_connection(
        &self,
        dial_info: DialInfo,
    ) -> EyreResult<NetworkResult<ConnectionHandle>> {
        let Ok(_guard) = self.arc.startup_lock.enter() else {
            return Ok(NetworkResult::service_unavailable(
                "connection manager is not started",
            ));
        };
        let peer_address = dial_info.peer_address();
        let remote_addr = peer_address.socket_addr();
        let mut preferred_local_address = self
            .network_manager()
            .net()
            .get_preferred_local_address(&dial_info);
        let best_port = preferred_local_address.map(|pla| pla.port());

        // Async lock on the remote address for atomicity per remote
        let _lock_guard = self.arc.address_lock_table.lock_tag(remote_addr).await;

        log_net!("== get_or_create_connection dial_info={:?}", dial_info);

        // If any connection to this remote exists that has the same protocol, return it
        // Any connection will do, we don't have to match the local address but if we can
        // match the preferred port do it
        if let Some(best_existing_conn) = self
            .arc
            .connection_table
            .get_best_connection_by_remote(best_port, peer_address)
        {
            log_net!(
                "== Returning best existing connection {:?}",
                best_existing_conn
            );

            return Ok(NetworkResult::Value(best_existing_conn));
        }

        // If there is a low-level connection collision here, then we release the 'preferred local address'
        // so we can make a second connection with an ephemeral port
        if self
            .arc
            .connection_table
            .check_for_colliding_connection(&dial_info)
        {
            preferred_local_address = None;
        }

        // Attempt new connection
        let mut retry_count = 1;

        let prot_conn = network_result_try!(loop {
            let result_net_res = ProtocolNetworkConnection::connect(
                preferred_local_address,
                &dial_info,
                self.arc.connection_initial_timeout_ms,
                self.network_manager().address_filter(),
            )
            .await;
            match result_net_res {
                Ok(net_res) => {
                    if net_res.is_value() || retry_count == 0 {
                        // Successful new connection, return it
                        break net_res;
                    }
                }
                Err(e) => {
                    if retry_count == 0 {
                        return Err(e).wrap_err(format!(
                            "failed to connect: {:?} -> {:?}",
                            preferred_local_address, dial_info
                        ));
                    }
                }
            };
            log_net!(debug "get_or_create_connection retries left: {}", retry_count);
            retry_count -= 1;

            // Release the preferred local address if things can't connect due to a low-level collision we dont have a record of
            preferred_local_address = None;
            sleep(500).await;
        });

        // Add to the connection table
        let mut inner = self.arc.inner.lock();
        let inner = match &mut *inner {
            Some(v) => v,
            None => {
                bail!("shutting down");
            }
        };

        self.on_new_protocol_network_connection(inner, prot_conn)
    }

    ///////////////////////////////////////////////////////////////////////////////////////////////////////
    /// Asynchronous Event Processor

    async fn process_connection_manager_event(
        &self,
        event: ConnectionManagerEvent,
        allow_accept: bool,
    ) {
        match event {
            ConnectionManagerEvent::Accepted(prot_conn) => {
                if !allow_accept {
                    return;
                }
                let Ok(_guard) = self.arc.startup_lock.enter() else {
                    return;
                };

                // Async lock on the remote address for atomicity per remote
                let _lock_guard = self
                    .arc
                    .address_lock_table
                    .lock_tag(prot_conn.flow().remote_address().socket_addr())
                    .await;

                let mut inner = self.arc.inner.lock();
                match &mut *inner {
                    Some(inner) => {
                        // Register the connection
                        // We don't care if this fails, since nobody here asked for the inbound connection.
                        // If it does, we just drop the connection

                        let _ = self.on_new_protocol_network_connection(inner, prot_conn);
                    }
                    None => {
                        // If this somehow happens, we're shutting down
                    }
                };
            }
            ConnectionManagerEvent::Dead(mut conn) => {
                let _lock_guard = self
                    .arc
                    .address_lock_table
                    .lock_tag(conn.flow().remote_address().socket_addr())
                    .await;

                conn.close();
                conn.await;
            }
        }
    }

    async fn async_processor(
        self,
        stop_token: StopToken,
        receiver: flume::Receiver<ConnectionManagerEvent>,
    ) {
        // Process async commands
        while let Ok(Ok(event)) = receiver.recv_async().timeout_at(stop_token.clone()).await {
            self.process_connection_manager_event(event, true).await;
        }
        // Ensure receiver is drained completely
        for event in receiver.drain() {
            self.process_connection_manager_event(event, false).await;
        }
    }

    // Called by low-level network when any connection-oriented protocol connection appears
    // either from incoming connections.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) async fn on_accepted_protocol_network_connection(
        &self,
        protocol_connection: ProtocolNetworkConnection,
    ) -> EyreResult<()> {
        // Get channel sender
        let sender = {
            let mut inner = self.arc.inner.lock();
            let inner = match &mut *inner {
                Some(v) => v,
                None => {
                    // If we are shutting down, just drop this and return
                    return Ok(());
                }
            };
            inner.sender.clone()
        };

        // Inform the processor of the event
        let _ = sender
            .send_async(ConnectionManagerEvent::Accepted(protocol_connection))
            .await;
        Ok(())
    }

    // Callback from network connection receive loop when it exits
    // cleans up the entry in the connection table
    pub(super) async fn report_connection_finished(&self, connection_id: NetworkConnectionId) {
        // Get channel sender
        let sender = {
            let mut inner = self.arc.inner.lock();
            let inner = match &mut *inner {
                Some(v) => v,
                None => {
                    // If we are shutting down, just drop this and return
                    return;
                }
            };
            inner.sender.clone()
        };

        // Remove the connection
        let conn = self
            .arc
            .connection_table
            .remove_connection_by_id(connection_id);

        // Inform the processor of the event
        if let Some(conn) = conn {
            // If the connection closed while it was protected, report it on the node the connection was established on
            // In-use connections will already get reported because they will cause a 'question_lost' stat on the remote node
            if let Some(protect_nr) = conn.protected_node_ref() {
                protect_nr.report_protected_connection_dropped();
            }
            let _ = sender.send_async(ConnectionManagerEvent::Dead(conn)).await;
        }
    }

    pub async fn debug_print(&self) -> String {
        //let inner = self.arc.inner.lock();
        format!(
            "Connection Table:\n\n{}",
            self.arc.connection_table.debug_print_table()
        )
    }
}

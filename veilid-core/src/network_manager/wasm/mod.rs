mod protocol;

use super::*;

use crate::routing_table::*;
use connection_manager::*;
use protocol::ws::WebsocketProtocolHandler;
pub use protocol::*;
use std::io;

/////////////////////////////////////////////////////////////////

cfg_if! {
    if #[cfg(all(feature = "unstable-blockstore", feature="unstable-tunnels"))] {
        const PUBLIC_INTERNET_CAPABILITIES_LEN: usize = 7;
    } else if #[cfg(any(feature = "unstable-blockstore", feature="unstable-tunnels"))] {
        const PUBLIC_INTERNET_CAPABILITIES_LEN: usize = 6;
    } else  {
        const PUBLIC_INTERNET_CAPABILITIES_LEN: usize = 5;
    }
}
pub const PUBLIC_INTERNET_CAPABILITIES: [Capability; PUBLIC_INTERNET_CAPABILITIES_LEN] = [
    CAP_ROUTE,
    #[cfg(feature = "unstable-tunnels")]
    CAP_TUNNEL,
    CAP_SIGNAL,
    //CAP_RELAY,
    //CAP_VALIDATE_DIAL_INFO,
    CAP_DHT,
    CAP_DHT_WATCH,
    CAP_APPMESSAGE,
    #[cfg(feature = "unstable-blockstore")]
    CAP_BLOCKSTORE,
];

// #[cfg(feature = "unstable-blockstore")]
// const LOCAL_NETWORK_CAPABILITIES_LEN: usize = 3;
// #[cfg(not(feature = "unstable-blockstore"))]
// const LOCAL_NETWORK_CAPABILITIES_LEN: usize = 2;

// pub const LOCAL_NETWORK_CAPABILITIES: [Capability; LOCAL_NETWORK_CAPABILITIES_LEN] = [
//     //CAP_RELAY,
//     CAP_DHT,
//     CAP_DHT_WATCH,
//     CAP_APPMESSAGE,
//     #[cfg(feature = "unstable-blockstore")]
//     CAP_BLOCKSTORE,
// ];

pub const MAX_CAPABILITIES: usize = 64;

/////////////////////////////////////////////////////////////////

struct NetworkInner {
    network_needs_restart: bool,
    protocol_config: ProtocolConfig,
}

struct NetworkUnlockedInner {
    // Startup lock
    startup_lock: StartupLock,

    // Accessors
    routing_table: RoutingTable,
    network_manager: NetworkManager,
    connection_manager: ConnectionManager,
}

#[derive(Clone)]
pub(in crate::network_manager) struct Network {
    config: VeilidConfig,
    inner: Arc<Mutex<NetworkInner>>,
    unlocked_inner: Arc<NetworkUnlockedInner>,
}

impl Network {
    fn new_inner() -> NetworkInner {
        NetworkInner {
            network_needs_restart: false,
            protocol_config: Default::default(),
        }
    }

    fn new_unlocked_inner(
        network_manager: NetworkManager,
        routing_table: RoutingTable,
        connection_manager: ConnectionManager,
    ) -> NetworkUnlockedInner {
        NetworkUnlockedInner {
            startup_lock: StartupLock::new(),
            network_manager,
            routing_table,
            connection_manager,
        }
    }

    pub fn new(
        network_manager: NetworkManager,
        routing_table: RoutingTable,
        connection_manager: ConnectionManager,
    ) -> Self {
        Self {
            config: network_manager.config(),
            inner: Arc::new(Mutex::new(Self::new_inner())),
            unlocked_inner: Arc::new(Self::new_unlocked_inner(
                network_manager,
                routing_table,
                connection_manager,
            )),
        }
    }

    fn network_manager(&self) -> NetworkManager {
        self.unlocked_inner.network_manager.clone()
    }
    fn routing_table(&self) -> RoutingTable {
        self.unlocked_inner.routing_table.clone()
    }
    fn connection_manager(&self) -> ConnectionManager {
        self.unlocked_inner.connection_manager.clone()
    }

    /////////////////////////////////////////////////////////////////

    // Record DialInfo failures
    async fn record_dial_info_failure<T, F: Future<Output = EyreResult<NetworkResult<T>>>>(
        &self,
        dial_info: DialInfo,
        fut: F,
    ) -> EyreResult<NetworkResult<T>> {
        let network_result = fut.await?;
        if matches!(network_result, NetworkResult::NoConnection(_)) {
            self.network_manager()
                .address_filter()
                .set_dial_info_failed(dial_info);
        }
        Ok(network_result)
    }

    // Send data to a dial info, unbound, using a new connection from a random port
    // This creates a short-lived connection in the case of connection-oriented protocols
    // for the purpose of sending this one message.
    // This bypasses the connection table as it is not a 'node to node' connection.
    #[instrument(level="trace", target="net", err, skip(self, data), fields(data.len = data.len()))]
    pub async fn send_data_unbound_to_dial_info(
        &self,
        dial_info: DialInfo,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<()>> {
        let _guard = self.unlocked_inner.startup_lock.enter()?;

        self.record_dial_info_failure(dial_info.clone(), async move {
            let data_len = data.len();
            let timeout_ms = {
                let c = self.config.get();
                c.network.connection_initial_timeout_ms
            };

            if self
                .network_manager()
                .address_filter()
                .is_ip_addr_punished(dial_info.address().ip_addr())
            {
                return Ok(NetworkResult::no_connection_other("punished"));
            }

            match dial_info.protocol_type() {
                ProtocolType::UDP => {
                    bail!("no support for UDP protocol")
                }
                ProtocolType::TCP => {
                    bail!("no support for TCP protocol")
                }
                ProtocolType::WS | ProtocolType::WSS => {
                    let pnc = network_result_try!(WebsocketProtocolHandler::connect(
                        &dial_info, timeout_ms
                    )
                    .await
                    .wrap_err("connect failure")?);
                    network_result_try!(pnc.send(data).await.wrap_err("send failure")?);
                }
            };

            // Network accounting
            self.network_manager()
                .stats_packet_sent(dial_info.ip_addr(), ByteCount::new(data_len as u64));

            Ok(NetworkResult::Value(()))
        })
        .await
    }

    // Send data to a dial info, unbound, using a new connection from a random port
    // Waits for a specified amount of time to receive a single response
    // This creates a short-lived connection in the case of connection-oriented protocols
    // for the purpose of sending this one message.
    // This bypasses the connection table as it is not a 'node to node' connection.
    #[instrument(level="trace", target="net", err, skip(self, data), fields(data.len = data.len()))]
    pub async fn send_recv_data_unbound_to_dial_info(
        &self,
        dial_info: DialInfo,
        data: Vec<u8>,
        timeout_ms: u32,
    ) -> EyreResult<NetworkResult<Vec<u8>>> {
        let _guard = self.unlocked_inner.startup_lock.enter()?;

        self.record_dial_info_failure(dial_info.clone(), async move {
            let data_len = data.len();
            let connect_timeout_ms = {
                let c = self.config.get();
                c.network.connection_initial_timeout_ms
            };

            if self
                .network_manager()
                .address_filter()
                .is_ip_addr_punished(dial_info.address().ip_addr())
            {
                return Ok(NetworkResult::no_connection_other("punished"));
            }

            match dial_info.protocol_type() {
                ProtocolType::UDP => {
                    bail!("no support for UDP protocol")
                }
                ProtocolType::TCP => {
                    bail!("no support for TCP protocol")
                }
                ProtocolType::WS | ProtocolType::WSS => {
                    let pnc = network_result_try!(match dial_info.protocol_type() {
                        ProtocolType::UDP => unreachable!(),
                        ProtocolType::TCP => unreachable!(),
                        ProtocolType::WS | ProtocolType::WSS => {
                            WebsocketProtocolHandler::connect(&dial_info, connect_timeout_ms)
                                .await
                                .wrap_err("connect failure")?
                        }
                    });

                    network_result_try!(pnc.send(data).await.wrap_err("send failure")?);
                    self.network_manager()
                        .stats_packet_sent(dial_info.ip_addr(), ByteCount::new(data_len as u64));

                    let out =
                        network_result_try!(network_result_try!(timeout(timeout_ms, pnc.recv())
                            .await
                            .into_network_result())
                        .wrap_err("recv failure")?);

                    self.network_manager()
                        .stats_packet_rcvd(dial_info.ip_addr(), ByteCount::new(out.len() as u64));

                    Ok(NetworkResult::Value(out))
                }
            }
        })
        .await
    }

    #[instrument(level="trace", target="net", err, skip(self, data), fields(data.len = data.len()))]
    pub async fn send_data_to_existing_flow(
        &self,
        flow: Flow,
        data: Vec<u8>,
    ) -> EyreResult<SendDataToExistingFlowResult> {
        let _guard = self.unlocked_inner.startup_lock.enter()?;

        let data_len = data.len();
        match flow.protocol_type() {
            ProtocolType::UDP => {
                bail!("no support for UDP protocol")
            }
            ProtocolType::TCP => {
                bail!("no support for TCP protocol")
            }
            _ => {}
        }

        // Handle connection-oriented protocols

        // Try to send to the exact existing connection if one exists
        if let Some(conn) = self.connection_manager().get_connection(flow) {
            // connection exists, send over it
            match conn.send_async(data).await {
                ConnectionHandleSendResult::Sent => {
                    // Network accounting
                    self.network_manager().stats_packet_sent(
                        flow.remote().socket_addr().ip(),
                        ByteCount::new(data_len as u64),
                    );

                    // Data was consumed
                    return Ok(SendDataToExistingFlowResult::Sent(conn.unique_flow()));
                }
                ConnectionHandleSendResult::NotSent(data) => {
                    // Couldn't send
                    // Pass the data back out so we don't own it any more
                    return Ok(SendDataToExistingFlowResult::NotSent(data));
                }
            }
        }
        // Connection didn't exist
        // Pass the data back out so we don't own it any more
        Ok(SendDataToExistingFlowResult::NotSent(data))
    }

    // Send data directly to a dial info, possibly without knowing which node it is going to
    // Returns a flow for the connection used to send the data
    #[instrument(level="trace", target="net", err, skip(self, data), fields(data.len = data.len()))]
    pub async fn send_data_to_dial_info(
        &self,
        dial_info: DialInfo,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<UniqueFlow>> {
        let _guard = self.unlocked_inner.startup_lock.enter()?;

        self.record_dial_info_failure(dial_info.clone(), async move {
            let data_len = data.len();
            if dial_info.protocol_type() == ProtocolType::UDP {
                bail!("no support for UDP protocol");
            }
            if dial_info.protocol_type() == ProtocolType::TCP {
                bail!("no support for TCP protocol");
            }

            // Handle connection-oriented protocols
            let conn = network_result_try!(
                self.connection_manager()
                    .get_or_create_connection(dial_info.clone())
                    .await?
            );

            if let ConnectionHandleSendResult::NotSent(_) = conn.send_async(data).await {
                return Ok(NetworkResult::NoConnection(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "failed to send",
                )));
            }
            let unique_flow = conn.unique_flow();

            // Network accounting
            self.network_manager()
                .stats_packet_sent(dial_info.ip_addr(), ByteCount::new(data_len as u64));

            Ok(NetworkResult::value(unique_flow))
        })
        .await
    }

    /////////////////////////////////////////////////////////////////

    pub async fn startup_internal(&self) -> EyreResult<StartupDisposition> {
        log_net!(debug "starting network");
        // get protocol config
        let protocol_config = {
            let c = self.config.get();
            let inbound = ProtocolTypeSet::new();
            let mut outbound = ProtocolTypeSet::new();

            if c.network.protocol.ws.connect {
                outbound.insert(ProtocolType::WS);
            }
            if c.network.protocol.wss.connect {
                outbound.insert(ProtocolType::WSS);
            }

            let supported_address_types: AddressTypeSet = if is_ipv6_supported() {
                AddressType::IPV4 | AddressType::IPV6
            } else {
                AddressType::IPV4.into()
            };

            let family_global = supported_address_types;
            let family_local = supported_address_types;

            let public_internet_capabilities = {
                PUBLIC_INTERNET_CAPABILITIES
                    .iter()
                    .copied()
                    .filter(|cap| !c.capabilities.disable.contains(cap))
                    .collect::<Vec<Capability>>()
            };

            ProtocolConfig {
                outbound,
                inbound,
                family_global,
                family_local,
                local_network_capabilities: vec![],
                public_internet_capabilities,
            }
        };
        self.inner.lock().protocol_config = protocol_config.clone();

        // Start editing routing table
        let mut editor_public_internet = self
            .unlocked_inner
            .routing_table
            .edit_routing_domain(RoutingDomain::PublicInternet);

        // set up the routing table's network config
        // if we have static public dialinfo, upgrade our network class

        editor_public_internet.setup_network(
            protocol_config.outbound,
            protocol_config.inbound,
            protocol_config.family_global,
            protocol_config.public_internet_capabilities.clone(),
        );
        editor_public_internet.set_network_class(Some(NetworkClass::WebApp));

        // commit routing table edits
        editor_public_internet.commit(true).await;

        Ok(StartupDisposition::Success)
    }

    #[instrument(level = "debug", err, skip_all)]
    pub async fn startup(&self) -> EyreResult<StartupDisposition> {
        let guard = self.unlocked_inner.startup_lock.startup()?;

        match self.startup_internal().await {
            Ok(StartupDisposition::Success) => {
                info!("network started");
                guard.success();
                Ok(StartupDisposition::Success)
            }
            Ok(StartupDisposition::BindRetry) => {
                debug!("network bind retry");
                Ok(StartupDisposition::BindRetry)
            }
            Err(e) => {
                debug!("network failed to start");
                Err(e)
            }
        }
    }

    pub fn needs_restart(&self) -> bool {
        self.inner.lock().network_needs_restart
    }

    pub fn is_started(&self) -> bool {
        self.unlocked_inner.startup_lock.is_started()
    }

    #[instrument(level = "debug", skip_all)]
    pub fn restart_network(&self) {
        self.inner.lock().network_needs_restart = true;
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn shutdown(&self) {
        log_net!(debug "starting low level network shutdown");
        let Ok(guard) = self.unlocked_inner.startup_lock.shutdown().await else {
            log_net!(debug "low level network is already shut down");
            return;
        };

        // Reset state
        let routing_table = self.routing_table();

        // Drop all dial info
        routing_table
            .edit_routing_domain(RoutingDomain::PublicInternet)
            .clear_dial_info_details(None, None)
            .set_network_class(None)
            .clear_relay_node()
            .commit(true)
            .await;

        // Cancels all async background tasks by dropping join handles
        *self.inner.lock() = Self::new_inner();

        guard.success();
        log_net!(debug "finished low level network shutdown");
    }

    pub fn get_preferred_local_address(&self, _dial_info: &DialInfo) -> Option<SocketAddr> {
        None
    }

    pub fn get_preferred_local_address_by_key(
        &self,
        _pt: ProtocolType,
        _at: AddressType,
    ) -> Option<SocketAddr> {
        None
    }

    //////////////////////////////////////////

    pub fn set_needs_public_dial_info_check(
        &self,
        _punishment: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) {
        let Ok(_guard) = self.unlocked_inner.startup_lock.enter() else {
            log_net!(debug "ignoring due to not started up");
            return;
        };
    }

    pub fn needs_public_dial_info_check(&self) -> bool {
        let Ok(_guard) = self.unlocked_inner.startup_lock.enter() else {
            log_net!(debug "ignoring due to not started up");
            return false;
        };

        false
    }

    //////////////////////////////////////////
    #[instrument(level = "trace", target = "net", name = "Network::tick", skip_all, err)]
    pub(crate) async fn tick(&self) -> EyreResult<()> {
        let Ok(_guard) = self.unlocked_inner.startup_lock.enter() else {
            log_net!(debug "ignoring due to not started up");
            return Ok(());
        };

        Ok(())
    }
}

use crate::*;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

mod connection_handle;
mod connection_limits;
mod connection_manager;
mod connection_table;
mod network_connection;
mod tasks;

pub mod tests;

////////////////////////////////////////////////////////////////////////////////////////

pub use network_connection::*;

////////////////////////////////////////////////////////////////////////////////////////
use connection_handle::*;
use connection_limits::*;
use connection_manager::*;
use dht::*;
use futures_util::stream::{FuturesUnordered, StreamExt};
use hashlink::LruCache;
use intf::*;
#[cfg(not(target_arch = "wasm32"))]
use native::*;
use receipt_manager::*;
use routing_table::*;
use rpc_processor::*;
#[cfg(target_arch = "wasm32")]
use wasm::*;
use xx::*;

////////////////////////////////////////////////////////////////////////////////////////

pub const RELAY_MANAGEMENT_INTERVAL_SECS: u32 = 1;
pub const MAX_MESSAGE_SIZE: usize = MAX_ENVELOPE_SIZE;
pub const IPADDR_TABLE_SIZE: usize = 1024;
pub const IPADDR_MAX_INACTIVE_DURATION_US: u64 = 300_000_000u64; // 5 minutes
pub const GLOBAL_ADDRESS_CHANGE_DETECTION_COUNT: usize = 3;
pub const BOOT_MAGIC: &[u8; 4] = b"BOOT";

pub const BOOTSTRAP_TXT_VERSION: u8 = 0;

#[derive(Clone, Debug)]
pub struct BootstrapRecord {
    min_version: u8,
    max_version: u8,
    dial_info_details: Vec<DialInfoDetail>,
}
pub type BootstrapRecordMap = BTreeMap<DHTKey, BootstrapRecord>;

#[derive(Copy, Clone, Debug, Default)]
pub struct ProtocolConfig {
    pub outbound: ProtocolTypeSet,
    pub inbound: ProtocolTypeSet,
    pub family_global: AddressTypeSet,
    pub family_local: AddressTypeSet,
}

// Things we get when we start up and go away when we shut down
// Routing table is not in here because we want it to survive a network shutdown/startup restart
#[derive(Clone)]
struct NetworkComponents {
    net: Network,
    connection_manager: ConnectionManager,
    rpc_processor: RPCProcessor,
    receipt_manager: ReceiptManager,
}

// Statistics per address
#[derive(Clone, Default)]
pub struct PerAddressStats {
    last_seen_ts: u64,
    transfer_stats_accounting: TransferStatsAccounting,
    transfer_stats: TransferStatsDownUp,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PerAddressStatsKey(IpAddr);

impl Default for PerAddressStatsKey {
    fn default() -> Self {
        Self(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    }
}

// Statistics about the low-level network
#[derive(Clone)]
pub struct NetworkManagerStats {
    self_stats: PerAddressStats,
    per_address_stats: LruCache<PerAddressStatsKey, PerAddressStats>,
}

impl Default for NetworkManagerStats {
    fn default() -> Self {
        Self {
            self_stats: PerAddressStats::default(),
            per_address_stats: LruCache::new(IPADDR_TABLE_SIZE),
        }
    }
}

#[derive(Debug)]
struct ClientWhitelistEntry {
    last_seen_ts: u64,
}

// Mechanism required to contact another node
#[derive(Clone, Debug)]
enum ContactMethod {
    Unreachable,                       // Node is not reachable by any means
    Direct(DialInfo),                  // Contact the node directly
    SignalReverse(NodeRef, NodeRef),   // Request via signal the node connect back directly
    SignalHolePunch(NodeRef, NodeRef), // Request via signal the node negotiate a hole punch
    InboundRelay(NodeRef),             // Must use an inbound relay to reach the node
    OutboundRelay(NodeRef),            // Must use outbound relay to reach the node
}

#[derive(Copy, Clone, Debug)]
pub enum SendDataKind {
    LocalDirect,
    GlobalDirect,
    GlobalIndirect,
}

// The mutable state of the network manager
struct NetworkManagerInner {
    routing_table: Option<RoutingTable>,
    components: Option<NetworkComponents>,
    update_callback: Option<UpdateCallback>,
    stats: NetworkManagerStats,
    client_whitelist: LruCache<DHTKey, ClientWhitelistEntry>,
    relay_node: Option<NodeRef>,
    public_address_check_cache: LruCache<DHTKey, SocketAddress>,
    protocol_config: Option<ProtocolConfig>,
    public_inbound_dial_info_filter: Option<DialInfoFilter>,
    local_inbound_dial_info_filter: Option<DialInfoFilter>,
    public_outbound_dial_info_filter: Option<DialInfoFilter>,
    local_outbound_dial_info_filter: Option<DialInfoFilter>,
}

struct NetworkManagerUnlockedInner {
    // Background processes
    rolling_transfers_task: TickTask<EyreReport>,
    relay_management_task: TickTask<EyreReport>,
    bootstrap_task: TickTask<EyreReport>,
    peer_minimum_refresh_task: TickTask<EyreReport>,
    ping_validator_task: TickTask<EyreReport>,
    node_info_update_single_future: MustJoinSingleFuture<()>,
}

#[derive(Clone)]
pub struct NetworkManager {
    config: VeilidConfig,
    table_store: TableStore,
    crypto: Crypto,
    inner: Arc<Mutex<NetworkManagerInner>>,
    unlocked_inner: Arc<NetworkManagerUnlockedInner>,
}

impl NetworkManager {
    fn new_inner() -> NetworkManagerInner {
        NetworkManagerInner {
            routing_table: None,
            components: None,
            update_callback: None,
            stats: NetworkManagerStats::default(),
            client_whitelist: LruCache::new_unbounded(),
            relay_node: None,
            public_address_check_cache: LruCache::new(8),
            protocol_config: None,
            public_inbound_dial_info_filter: None,
            local_inbound_dial_info_filter: None,
            public_outbound_dial_info_filter: None,
            local_outbound_dial_info_filter: None,
        }
    }
    fn new_unlocked_inner(config: VeilidConfig) -> NetworkManagerUnlockedInner {
        let c = config.get();
        NetworkManagerUnlockedInner {
            rolling_transfers_task: TickTask::new(ROLLING_TRANSFERS_INTERVAL_SECS),
            relay_management_task: TickTask::new(RELAY_MANAGEMENT_INTERVAL_SECS),
            bootstrap_task: TickTask::new(1),
            peer_minimum_refresh_task: TickTask::new_ms(c.network.dht.min_peer_refresh_time_ms),
            ping_validator_task: TickTask::new(1),
            node_info_update_single_future: MustJoinSingleFuture::new(),
        }
    }

    pub fn new(config: VeilidConfig, table_store: TableStore, crypto: Crypto) -> Self {
        let this = Self {
            config: config.clone(),
            table_store,
            crypto,
            inner: Arc::new(Mutex::new(Self::new_inner())),
            unlocked_inner: Arc::new(Self::new_unlocked_inner(config)),
        };
        // Set rolling transfers tick task
        {
            let this2 = this.clone();
            this.unlocked_inner
                .rolling_transfers_task
                .set_routine(move |s, l, t| {
                    Box::pin(this2.clone().rolling_transfers_task_routine(s, l, t))
                });
        }
        // Set relay management tick task
        {
            let this2 = this.clone();
            this.unlocked_inner
                .relay_management_task
                .set_routine(move |s, l, t| {
                    Box::pin(this2.clone().relay_management_task_routine(s, l, t))
                });
        }
        // Set bootstrap tick task
        {
            let this2 = this.clone();
            this.unlocked_inner
                .bootstrap_task
                .set_routine(move |s, _l, _t| Box::pin(this2.clone().bootstrap_task_routine(s)));
        }
        // Set peer minimum refresh tick task
        {
            let this2 = this.clone();
            this.unlocked_inner
                .peer_minimum_refresh_task
                .set_routine(move |s, _l, _t| {
                    Box::pin(this2.clone().peer_minimum_refresh_task_routine(s))
                });
        }
        // Set ping validator tick task
        {
            let this2 = this.clone();
            this.unlocked_inner
                .ping_validator_task
                .set_routine(move |s, l, t| {
                    Box::pin(this2.clone().ping_validator_task_routine(s, l, t))
                });
        }
        this
    }
    pub fn config(&self) -> VeilidConfig {
        self.config.clone()
    }
    pub fn table_store(&self) -> TableStore {
        self.table_store.clone()
    }
    pub fn crypto(&self) -> Crypto {
        self.crypto.clone()
    }
    pub fn routing_table(&self) -> RoutingTable {
        self.inner.lock().routing_table.as_ref().unwrap().clone()
    }
    pub fn net(&self) -> Network {
        self.inner.lock().components.as_ref().unwrap().net.clone()
    }
    pub fn rpc_processor(&self) -> RPCProcessor {
        self.inner
            .lock()
            .components
            .as_ref()
            .unwrap()
            .rpc_processor
            .clone()
    }
    pub fn receipt_manager(&self) -> ReceiptManager {
        self.inner
            .lock()
            .components
            .as_ref()
            .unwrap()
            .receipt_manager
            .clone()
    }
    pub fn connection_manager(&self) -> ConnectionManager {
        self.inner
            .lock()
            .components
            .as_ref()
            .unwrap()
            .connection_manager
            .clone()
    }

    pub fn relay_node(&self) -> Option<NodeRef> {
        self.inner.lock().relay_node.clone()
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn init(&self, update_callback: UpdateCallback) -> EyreResult<()> {
        let routing_table = RoutingTable::new(self.clone());
        routing_table.init().await?;
        self.inner.lock().routing_table = Some(routing_table.clone());
        self.inner.lock().update_callback = Some(update_callback);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn terminate(&self) {
        let routing_table = {
            let mut inner = self.inner.lock();
            inner.routing_table.take()
        };
        if let Some(routing_table) = routing_table {
            routing_table.terminate().await;
        }
        self.inner.lock().update_callback = None;
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn internal_startup(&self) -> EyreResult<()> {
        trace!("NetworkManager::internal_startup begin");
        if self.inner.lock().components.is_some() {
            debug!("NetworkManager::internal_startup already started");
            return Ok(());
        }

        // Create network components
        let connection_manager = ConnectionManager::new(self.clone());
        let net = Network::new(
            self.clone(),
            self.routing_table(),
            connection_manager.clone(),
        );
        let rpc_processor = RPCProcessor::new(self.clone());
        let receipt_manager = ReceiptManager::new(self.clone());
        self.inner.lock().components = Some(NetworkComponents {
            net: net.clone(),
            connection_manager: connection_manager.clone(),
            rpc_processor: rpc_processor.clone(),
            receipt_manager: receipt_manager.clone(),
        });

        // Start network components
        connection_manager.startup().await;
        net.startup().await?;
        rpc_processor.startup().await?;
        receipt_manager.startup().await?;

        trace!("NetworkManager::internal_startup end");

        Ok(())
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn startup(&self) -> EyreResult<()> {
        if let Err(e) = self.internal_startup().await {
            self.shutdown().await;
            return Err(e);
        }

        // Store copy of protocol config and dial info filters
        {
            let mut inner = self.inner.lock();
            let pc = inner
                .components
                .as_ref()
                .unwrap()
                .net
                .get_protocol_config()
                .unwrap();

            inner.public_inbound_dial_info_filter = Some(
                DialInfoFilter::global()
                    .with_protocol_type_set(pc.inbound)
                    .with_address_type_set(pc.family_global),
            );
            inner.local_inbound_dial_info_filter = Some(
                DialInfoFilter::local()
                    .with_protocol_type_set(pc.inbound)
                    .with_address_type_set(pc.family_local),
            );
            inner.public_outbound_dial_info_filter = Some(
                DialInfoFilter::global()
                    .with_protocol_type_set(pc.outbound)
                    .with_address_type_set(pc.family_global),
            );
            inner.local_outbound_dial_info_filter = Some(
                DialInfoFilter::local()
                    .with_protocol_type_set(pc.outbound)
                    .with_address_type_set(pc.family_local),
            );

            inner.protocol_config = Some(pc);
        }

        // Inform routing table entries that our dial info has changed
        self.send_node_info_updates(true).await;

        // Inform api clients that things have changed
        self.send_network_update();

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn shutdown(&self) {
        debug!("starting network manager shutdown");

        // Cancel all tasks
        debug!("stopping rolling transfers task");
        if let Err(e) = self.unlocked_inner.rolling_transfers_task.stop().await {
            warn!("rolling_transfers_task not stopped: {}", e);
        }
        debug!("stopping relay management task");
        if let Err(e) = self.unlocked_inner.relay_management_task.stop().await {
            warn!("relay_management_task not stopped: {}", e);
        }
        debug!("stopping bootstrap task");
        if let Err(e) = self.unlocked_inner.bootstrap_task.stop().await {
            error!("bootstrap_task not stopped: {}", e);
        }
        debug!("stopping peer minimum refresh task");
        if let Err(e) = self.unlocked_inner.peer_minimum_refresh_task.stop().await {
            error!("peer_minimum_refresh_task not stopped: {}", e);
        }
        debug!("stopping ping_validator task");
        if let Err(e) = self.unlocked_inner.ping_validator_task.stop().await {
            error!("ping_validator_task not stopped: {}", e);
        }
        debug!("stopping node info update singlefuture");
        if self
            .unlocked_inner
            .node_info_update_single_future
            .join()
            .await
            .is_err()
        {
            error!("node_info_update_single_future not stopped");
        }

        // Shutdown network components if they started up
        debug!("shutting down network components");

        let components = self.inner.lock().components.clone();
        if let Some(components) = components {
            components.net.shutdown().await;
            components.rpc_processor.shutdown().await;
            components.receipt_manager.shutdown().await;
            components.connection_manager.shutdown().await;
        }

        // reset the state
        debug!("resetting network manager state");
        {
            let mut inner = self.inner.lock();
            inner.components = None;
            inner.relay_node = None;
            inner.public_inbound_dial_info_filter = None;
            inner.local_inbound_dial_info_filter = None;
            inner.public_outbound_dial_info_filter = None;
            inner.local_outbound_dial_info_filter = None;
            inner.protocol_config = None;
        }

        // send update
        debug!("sending network state update");
        self.send_network_update();

        debug!("finished network manager shutdown");
    }

    pub fn update_client_whitelist(&self, client: DHTKey) {
        let mut inner = self.inner.lock();
        match inner.client_whitelist.entry(client) {
            hashlink::lru_cache::Entry::Occupied(mut entry) => {
                entry.get_mut().last_seen_ts = intf::get_timestamp()
            }
            hashlink::lru_cache::Entry::Vacant(entry) => {
                entry.insert(ClientWhitelistEntry {
                    last_seen_ts: intf::get_timestamp(),
                });
            }
        }
    }

    #[instrument(level = "trace", skip(self), ret)]
    pub fn check_client_whitelist(&self, client: DHTKey) -> bool {
        let mut inner = self.inner.lock();

        match inner.client_whitelist.entry(client) {
            hashlink::lru_cache::Entry::Occupied(mut entry) => {
                entry.get_mut().last_seen_ts = intf::get_timestamp();
                true
            }
            hashlink::lru_cache::Entry::Vacant(_) => false,
        }
    }

    pub fn purge_client_whitelist(&self) {
        let timeout_ms = self.config.get().network.client_whitelist_timeout_ms;
        let mut inner = self.inner.lock();
        let cutoff_timestamp = intf::get_timestamp() - ((timeout_ms as u64) * 1000u64);
        // Remove clients from the whitelist that haven't been since since our whitelist timeout
        while inner
            .client_whitelist
            .peek_lru()
            .map(|v| v.1.last_seen_ts < cutoff_timestamp)
            .unwrap_or_default()
        {
            let (k, v) = inner.client_whitelist.remove_lru().unwrap();
            trace!(key=?k, value=?v, "purge_client_whitelist: remove_lru")
        }
    }

    pub fn needs_restart(&self) -> bool {
        let net = self.net();
        net.needs_restart()
    }

    pub async fn tick(&self) -> EyreResult<()> {
        let (routing_table, net, receipt_manager, protocol_config) = {
            let inner = self.inner.lock();
            let components = inner.components.as_ref().unwrap();
            let protocol_config = inner.protocol_config.as_ref().unwrap();
            (
                inner.routing_table.as_ref().unwrap().clone(),
                components.net.clone(),
                components.receipt_manager.clone(),
                protocol_config.clone(),
            )
        };

        // Run the rolling transfers task
        self.unlocked_inner.rolling_transfers_task.tick().await?;

        // Process global peer scope ticks
        // These only make sense when connected to the actual internet and not just the local network
        // Must have at least one outbound protocol enabled, and one global peer scope address family enabled
        let global_peer_scope_enabled =
            !protocol_config.outbound.is_empty() && !protocol_config.family_global.is_empty();
        if global_peer_scope_enabled {
            // Run the relay management task
            self.unlocked_inner.relay_management_task.tick().await?;

            // If routing table has no live entries, then add the bootstrap nodes to it
            let live_entry_count = routing_table.get_entry_count(BucketEntryState::Unreliable);
            if live_entry_count == 0 {
                self.unlocked_inner.bootstrap_task.tick().await?;
            }

            // If we still don't have enough peers, find nodes until we do
            let min_peer_count = {
                let c = self.config.get();
                c.network.dht.min_peer_count as usize
            };
            if live_entry_count < min_peer_count {
                self.unlocked_inner.peer_minimum_refresh_task.tick().await?;
            }

            // Ping validate some nodes to groom the table
            self.unlocked_inner.ping_validator_task.tick().await?;
        }

        // Run the routing table tick
        routing_table.tick().await?;

        // Run the low level network tick
        net.tick().await?;

        // Run the receipt manager tick
        receipt_manager.tick().await?;

        // Purge the client whitelist
        self.purge_client_whitelist();

        Ok(())
    }

    // Return what network class we are in
    pub fn get_network_class(&self) -> Option<NetworkClass> {
        if let Some(components) = &self.inner.lock().components {
            components.net.get_network_class()
        } else {
            None
        }
    }

    // Get our node's capabilities
    pub fn generate_node_status(&self) -> NodeStatus {
        let peer_info = self.routing_table().get_own_peer_info();

        let will_route = peer_info.signed_node_info.node_info.can_inbound_relay(); // xxx: eventually this may have more criteria added
        let will_tunnel = peer_info.signed_node_info.node_info.can_inbound_relay(); // xxx: we may want to restrict by battery life and network bandwidth at some point
        let will_signal = peer_info.signed_node_info.node_info.can_signal();
        let will_relay = peer_info.signed_node_info.node_info.can_inbound_relay();
        let will_validate_dial_info = peer_info
            .signed_node_info
            .node_info
            .can_validate_dial_info();

        NodeStatus {
            will_route,
            will_tunnel,
            will_signal,
            will_relay,
            will_validate_dial_info,
        }
    }

    // Return what protocols we have enabled
    pub fn get_protocol_config(&self) -> ProtocolConfig {
        let inner = self.inner.lock();
        inner.protocol_config.as_ref().unwrap().clone()
    }

    // Return a dial info filter for what we can receive
    pub fn get_inbound_dial_info_filter(&self, routing_domain: RoutingDomain) -> DialInfoFilter {
        let inner = self.inner.lock();
        match routing_domain {
            RoutingDomain::PublicInternet => inner
                .public_inbound_dial_info_filter
                .as_ref()
                .unwrap()
                .clone(),
            RoutingDomain::LocalNetwork => inner
                .local_inbound_dial_info_filter
                .as_ref()
                .unwrap()
                .clone(),
        }
    }

    // Return a dial info filter for what we can send out
    pub fn get_outbound_dial_info_filter(&self, routing_domain: RoutingDomain) -> DialInfoFilter {
        let inner = self.inner.lock();
        match routing_domain {
            RoutingDomain::PublicInternet => inner
                .public_outbound_dial_info_filter
                .as_ref()
                .unwrap()
                .clone(),
            RoutingDomain::LocalNetwork => inner
                .local_outbound_dial_info_filter
                .as_ref()
                .unwrap()
                .clone(),
        }
    }

    // Generates a multi-shot/normal receipt
    #[instrument(level = "trace", skip(self, extra_data, callback), err)]
    pub fn generate_receipt<D: AsRef<[u8]>>(
        &self,
        expiration_us: u64,
        expected_returns: u32,
        extra_data: D,
        callback: impl ReceiptCallback,
    ) -> EyreResult<Vec<u8>> {
        let receipt_manager = self.receipt_manager();
        let routing_table = self.routing_table();

        // Generate receipt and serialized form to return
        let nonce = Crypto::get_random_nonce();
        let receipt = Receipt::try_new(0, nonce, routing_table.node_id(), extra_data)?;
        let out = receipt
            .to_signed_data(&routing_table.node_id_secret())
            .wrap_err("failed to generate signed receipt")?;

        // Record the receipt for later
        let exp_ts = intf::get_timestamp() + expiration_us;
        receipt_manager.record_receipt(receipt, exp_ts, expected_returns, callback);

        Ok(out)
    }

    // Generates a single-shot/normal receipt
    #[instrument(level = "trace", skip(self, extra_data), err)]
    pub fn generate_single_shot_receipt<D: AsRef<[u8]>>(
        &self,
        expiration_us: u64,
        extra_data: D,
    ) -> EyreResult<(Vec<u8>, EventualValueFuture<ReceiptEvent>)> {
        let receipt_manager = self.receipt_manager();
        let routing_table = self.routing_table();

        // Generate receipt and serialized form to return
        let nonce = Crypto::get_random_nonce();
        let receipt = Receipt::try_new(0, nonce, routing_table.node_id(), extra_data)?;
        let out = receipt
            .to_signed_data(&routing_table.node_id_secret())
            .wrap_err("failed to generate signed receipt")?;

        // Record the receipt for later
        let exp_ts = intf::get_timestamp() + expiration_us;
        let eventual = SingleShotEventual::new(Some(ReceiptEvent::Cancelled));
        let instance = eventual.instance();
        receipt_manager.record_single_shot_receipt(receipt, exp_ts, eventual);

        Ok((out, instance))
    }

    // Process a received out-of-band receipt
    #[instrument(level = "trace", skip(self, receipt_data), ret)]
    pub async fn handle_out_of_band_receipt<R: AsRef<[u8]>>(
        &self,
        receipt_data: R,
    ) -> NetworkResult<()> {
        let receipt_manager = self.receipt_manager();

        let receipt = match Receipt::from_signed_data(receipt_data.as_ref()) {
            Err(e) => {
                return NetworkResult::invalid_message(e.to_string());
            }
            Ok(v) => v,
        };

        receipt_manager.handle_receipt(receipt, None).await
    }

    // Process a received in-band receipt
    #[instrument(level = "trace", skip(self, receipt_data), ret)]
    pub async fn handle_in_band_receipt<R: AsRef<[u8]>>(
        &self,
        receipt_data: R,
        inbound_nr: NodeRef,
    ) -> NetworkResult<()> {
        let receipt_manager = self.receipt_manager();

        let receipt = match Receipt::from_signed_data(receipt_data.as_ref()) {
            Err(e) => {
                return NetworkResult::invalid_message(e.to_string());
            }
            Ok(v) => v,
        };

        receipt_manager
            .handle_receipt(receipt, Some(inbound_nr))
            .await
    }

    // Process a received signal
    #[instrument(level = "trace", skip(self), err)]
    pub async fn handle_signal(
        &self,
        _sender_id: DHTKey,
        signal_info: SignalInfo,
    ) -> EyreResult<NetworkResult<()>> {
        match signal_info {
            SignalInfo::ReverseConnect { receipt, peer_info } => {
                let routing_table = self.routing_table();
                let rpc = self.rpc_processor();

                // Add the peer info to our routing table
                let peer_nr = match routing_table.register_node_with_signed_node_info(
                    peer_info.node_id.key,
                    peer_info.signed_node_info,
                ) {
                    None => {
                        return Ok(NetworkResult::invalid_message(
                            "unable to register reverse connect peerinfo",
                        ))
                    }
                    Some(nr) => nr,
                };

                // Make a reverse connection to the peer and send the receipt to it
                rpc.rpc_call_return_receipt(Destination::Direct(peer_nr), None, receipt)
                    .await
                    .wrap_err("rpc failure")
            }
            SignalInfo::HolePunch { receipt, peer_info } => {
                let routing_table = self.routing_table();
                let rpc = self.rpc_processor();

                // Add the peer info to our routing table
                let mut peer_nr = match routing_table.register_node_with_signed_node_info(
                    peer_info.node_id.key,
                    peer_info.signed_node_info,
                ) {
                    None => {
                        return Ok(NetworkResult::invalid_message(
                            //sender_id,
                            "unable to register hole punch connect peerinfo",
                        ));
                    }
                    Some(nr) => nr,
                };

                // Get the udp direct dialinfo for the hole punch
                let outbound_dif = self
                    .get_outbound_dial_info_filter(RoutingDomain::PublicInternet)
                    .with_protocol_type(ProtocolType::UDP);
                peer_nr.set_filter(Some(outbound_dif));
                let hole_punch_dial_info_detail = peer_nr
                    .first_filtered_dial_info_detail(Some(RoutingDomain::PublicInternet))
                    .ok_or_else(|| eyre!("No hole punch capable dialinfo found for node"))?;

                // Now that we picked a specific dialinfo, further restrict the noderef to the specific address type
                let filter = peer_nr.take_filter().unwrap();
                let filter =
                    filter.with_address_type(hole_punch_dial_info_detail.dial_info.address_type());
                peer_nr.set_filter(Some(filter));

                // Do our half of the hole punch by sending an empty packet
                // Both sides will do this and then the receipt will get sent over the punched hole
                network_result_try!(
                    self.net()
                        .send_data_to_dial_info(
                            hole_punch_dial_info_detail.dial_info.clone(),
                            Vec::new(),
                        )
                        .await?
                );

                // XXX: do we need a delay here? or another hole punch packet?

                // Return the receipt using the same dial info send the receipt to it
                rpc.rpc_call_return_receipt(Destination::Direct(peer_nr), None, receipt)
                    .await
                    .wrap_err("rpc failure")
            }
        }
    }

    // Builds an envelope for sending over the network
    #[instrument(level = "trace", skip(self, body), err)]
    fn build_envelope<B: AsRef<[u8]>>(
        &self,
        dest_node_id: DHTKey,
        version: u8,
        body: B,
    ) -> EyreResult<Vec<u8>> {
        // DH to get encryption key
        let routing_table = self.routing_table();
        let node_id = routing_table.node_id();
        let node_id_secret = routing_table.node_id_secret();

        // Get timestamp, nonce
        let ts = intf::get_timestamp();
        let nonce = Crypto::get_random_nonce();

        // Encode envelope
        let envelope = Envelope::new(version, ts, nonce, node_id, dest_node_id);
        envelope
            .to_encrypted_data(self.crypto.clone(), body.as_ref(), &node_id_secret)
            .wrap_err("envelope failed to encode")
    }

    // Called by the RPC handler when we want to issue an RPC request or response
    // node_ref is the direct destination to which the envelope will be sent
    // If 'node_id' is specified, it can be different than node_ref.node_id()
    // which will cause the envelope to be relayed
    #[instrument(level = "trace", skip(self, body), ret, err)]
    pub async fn send_envelope<B: AsRef<[u8]>>(
        &self,
        node_ref: NodeRef,
        envelope_node_id: Option<DHTKey>,
        body: B,
    ) -> EyreResult<NetworkResult<SendDataKind>> {
        let via_node_id = node_ref.node_id();
        let envelope_node_id = envelope_node_id.unwrap_or(via_node_id);

        if envelope_node_id != via_node_id {
            log_net!(
                "sending envelope to {:?} via {:?}",
                envelope_node_id,
                node_ref
            );
        } else {
            log_net!("sending envelope to {:?}", node_ref);
        }
        // Get node's min/max version and see if we can send to it
        // and if so, get the max version we can use
        let version = if let Some((node_min, node_max)) = node_ref.operate(|e| e.min_max_version())
        {
            #[allow(clippy::absurd_extreme_comparisons)]
            if node_min > MAX_VERSION || node_max < MIN_VERSION {
                bail!(
                    "can't talk to this node {} because version is unsupported: ({},{})",
                    via_node_id,
                    node_min,
                    node_max
                );
            }
            cmp::min(node_max, MAX_VERSION)
        } else {
            MAX_VERSION
        };

        // Build the envelope to send
        let out = self.build_envelope(envelope_node_id, version, body)?;

        // Send the envelope via whatever means necessary
        let send_data_kind = network_result_try!(self.send_data(node_ref.clone(), out).await?);

        // If we asked to relay from the start, then this is always indirect
        Ok(NetworkResult::value(if envelope_node_id != via_node_id {
            SendDataKind::GlobalIndirect
        } else {
            send_data_kind
        }))
    }

    // Called by the RPC handler when we want to issue an direct receipt
    #[instrument(level = "trace", skip(self, rcpt_data), err)]
    pub async fn send_out_of_band_receipt(
        &self,
        dial_info: DialInfo,
        rcpt_data: Vec<u8>,
    ) -> EyreResult<()> {
        // Do we need to validate the outgoing receipt? Probably not
        // because it is supposed to be opaque and the
        // recipient/originator does the validation
        // Also, in the case of an old 'version', returning the receipt
        // should not be subject to our ability to decode it

        // Send receipt directly
        network_result_value_or_log!(debug self
            .net()
            .send_data_unbound_to_dial_info(dial_info, rcpt_data)
            .await? => {
                return Ok(());
            }
        );
        Ok(())
    }

    #[instrument(level = "trace", skip(self), ret)]
    fn get_contact_method_public(&self, target_node_ref: NodeRef) -> ContactMethod {
        // Scope noderef down to protocols we can do outbound
        let public_outbound_dif = self.get_outbound_dial_info_filter(RoutingDomain::PublicInternet);
        let target_node_ref = target_node_ref.filtered_clone(public_outbound_dif.clone());

        // Get the best match internet dial info if we have it
        let opt_target_public_did =
            target_node_ref.first_filtered_dial_info_detail(Some(RoutingDomain::PublicInternet));
        if let Some(target_public_did) = opt_target_public_did {
            // Do we need to signal before going inbound?
            if !target_public_did.class.requires_signal() {
                // Go direct without signaling
                return ContactMethod::Direct(target_public_did.dial_info);
            }

            // Get the target's inbound relay, it must have one or it is not reachable
            // Note that .relay() never returns our own node. We can't relay to ourselves.
            if let Some(inbound_relay_nr) = target_node_ref.relay() {
                // Scope down to protocols we can do outbound
                let inbound_relay_nr = inbound_relay_nr.filtered_clone(public_outbound_dif.clone());
                // Can we reach the inbound relay?
                if inbound_relay_nr
                    .first_filtered_dial_info_detail(Some(RoutingDomain::PublicInternet))
                    .is_some()
                {
                    // Can we receive anything inbound ever?
                    let our_network_class =
                        self.get_network_class().unwrap_or(NetworkClass::Invalid);
                    if matches!(our_network_class, NetworkClass::InboundCapable) {
                        let routing_table = self.routing_table();

                        ///////// Reverse connection

                        // Get the best match dial info for an reverse inbound connection
                        let reverse_dif = self
                            .get_inbound_dial_info_filter(RoutingDomain::PublicInternet)
                            .filtered(target_node_ref.node_info_outbound_filter());
                        if let Some(reverse_did) = routing_table.first_filtered_dial_info_detail(
                            Some(RoutingDomain::PublicInternet),
                            &reverse_dif,
                        ) {
                            // Can we receive a direct reverse connection?
                            if !reverse_did.class.requires_signal() {
                                return ContactMethod::SignalReverse(
                                    inbound_relay_nr,
                                    target_node_ref,
                                );
                            }
                        }

                        ///////// UDP hole-punch

                        // Does the target have a direct udp dialinfo we can reach?
                        let udp_target_nr = target_node_ref.filtered_clone(
                            DialInfoFilter::global().with_protocol_type(ProtocolType::UDP),
                        );
                        let target_has_udp_dialinfo = udp_target_nr
                            .first_filtered_dial_info_detail(Some(RoutingDomain::PublicInternet))
                            .is_some();

                        // Does the self node have a direct udp dialinfo the target can reach?
                        let inbound_udp_dif = self
                            .get_inbound_dial_info_filter(RoutingDomain::PublicInternet)
                            .filtered(target_node_ref.node_info_outbound_filter())
                            .filtered(
                                DialInfoFilter::global().with_protocol_type(ProtocolType::UDP),
                            );
                        let self_has_udp_dialinfo = routing_table
                            .first_filtered_dial_info_detail(
                                Some(RoutingDomain::PublicInternet),
                                &inbound_udp_dif,
                            )
                            .is_some();

                        // Does the target and ourselves have a udp dialinfo that they can reach?
                        if target_has_udp_dialinfo && self_has_udp_dialinfo {
                            return ContactMethod::SignalHolePunch(inbound_relay_nr, udp_target_nr);
                        }
                        // Otherwise we have to inbound relay
                    }

                    return ContactMethod::InboundRelay(inbound_relay_nr);
                }
            }
        }
        // If the other node is not inbound capable at all, it needs to have an inbound relay
        else if let Some(target_inbound_relay_nr) = target_node_ref.relay() {
            // Can we reach the full relay?
            if target_inbound_relay_nr
                .first_filtered_dial_info_detail(Some(RoutingDomain::PublicInternet))
                .is_some()
            {
                return ContactMethod::InboundRelay(target_inbound_relay_nr);
            }
        }

        ContactMethod::Unreachable
    }

    #[instrument(level = "trace", skip(self), ret)]
    fn get_contact_method_local(&self, target_node_ref: NodeRef) -> ContactMethod {
        // Scope noderef down to protocols we can do outbound
        let local_outbound_dif = self.get_outbound_dial_info_filter(RoutingDomain::LocalNetwork);
        let target_node_ref = target_node_ref.filtered_clone(local_outbound_dif);

        // Get the best matching local direct dial info if we have it
        // ygjghhtiygiukuymyg
        if target_node_ref.is_filter_dead() {
            return ContactMethod::Unreachable;
        }
        let opt_target_local_did =
            target_node_ref.first_filtered_dial_info_detail(Some(RoutingDomain::LocalNetwork));
        if let Some(target_local_did) = opt_target_local_did {
            return ContactMethod::Direct(target_local_did.dial_info);
        }
        return ContactMethod::Unreachable;
    }

    // Figure out how to reach a node
    #[instrument(level = "trace", skip(self), ret)]
    fn get_contact_method(&self, target_node_ref: NodeRef) -> ContactMethod {
        // Try local first
        let out = self.get_contact_method_local(target_node_ref.clone());
        if !matches!(out, ContactMethod::Unreachable) {
            return out;
        }

        // Try public next
        let out = self.get_contact_method_public(target_node_ref.clone());
        if !matches!(out, ContactMethod::Unreachable) {
            return out;
        }

        // If we can't reach the node by other means, try our outbound relay if we have one
        if let Some(relay_node) = self.relay_node() {
            return ContactMethod::OutboundRelay(relay_node);
        }

        // Otherwise, we can't reach this node
        debug!("unable to reach node {:?}", target_node_ref);
        ContactMethod::Unreachable
    }

    // Send a reverse connection signal and wait for the return receipt over it
    // Then send the data across the new connection
    #[instrument(level = "trace", skip(self, data), err)]
    pub async fn do_reverse_connect(
        &self,
        relay_nr: NodeRef,
        target_nr: NodeRef,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<()>> {
        // Build a return receipt for the signal
        let receipt_timeout =
            ms_to_us(self.config.get().network.reverse_connection_receipt_time_ms);
        let (receipt, eventual_value) = self.generate_single_shot_receipt(receipt_timeout, [])?;

        // Get our peer info
        let peer_info = self.routing_table().get_own_peer_info();

        // Issue the signal
        let rpc = self.rpc_processor();
        network_result_try!(rpc
            .rpc_call_signal(
                Destination::Relay(relay_nr.clone(), target_nr.node_id()),
                None,
                SignalInfo::ReverseConnect { receipt, peer_info },
            )
            .await
            .wrap_err("failed to send signal")?);

        // Wait for the return receipt
        let inbound_nr = match eventual_value.await.take_value().unwrap() {
            ReceiptEvent::ReturnedOutOfBand => {
                bail!("reverse connect receipt should be returned in-band");
            }
            ReceiptEvent::ReturnedInBand { inbound_noderef } => inbound_noderef,
            ReceiptEvent::Expired => {
                //bail!("reverse connect receipt expired from {:?}", target_nr);
                return Ok(NetworkResult::timeout());
            }
            ReceiptEvent::Cancelled => {
                bail!("reverse connect receipt cancelled from {:?}", target_nr);
            }
        };

        // We expect the inbound noderef to be the same as the target noderef
        // if they aren't the same, we should error on this and figure out what then hell is up
        if target_nr != inbound_nr {
            bail!("unexpected noderef mismatch on reverse connect");
        }

        // And now use the existing connection to send over
        if let Some(descriptor) = inbound_nr.last_connection().await {
            match self
                .net()
                .send_data_to_existing_connection(descriptor, data)
                .await?
            {
                None => Ok(NetworkResult::value(())),
                Some(_) => Ok(NetworkResult::no_connection_other(
                    "unable to send over reverse connection",
                )),
            }
        } else {
            bail!("no reverse connection available")
        }
    }

    // Send a hole punch signal and do a negotiating ping and wait for the return receipt
    // Then send the data across the new connection
    #[instrument(level = "trace", skip(self, data), err)]
    pub async fn do_hole_punch(
        &self,
        relay_nr: NodeRef,
        target_nr: NodeRef,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<()>> {
        // Ensure we are filtered down to UDP (the only hole punch protocol supported today)
        assert!(target_nr
            .filter_ref()
            .map(|dif| dif.protocol_set == ProtocolTypeSet::only(ProtocolType::UDP))
            .unwrap_or_default());

        // Build a return receipt for the signal
        let receipt_timeout = ms_to_us(self.config.get().network.hole_punch_receipt_time_ms);
        let (receipt, eventual_value) = self.generate_single_shot_receipt(receipt_timeout, [])?;
        // Get our peer info
        let peer_info = self.routing_table().get_own_peer_info();

        // Get the udp direct dialinfo for the hole punch
        let hole_punch_did = target_nr
            .first_filtered_dial_info_detail(Some(RoutingDomain::PublicInternet))
            .ok_or_else(|| eyre!("No hole punch capable dialinfo found for node"))?;

        // Do our half of the hole punch by sending an empty packet
        // Both sides will do this and then the receipt will get sent over the punched hole
        network_result_try!(
            self.net()
                .send_data_to_dial_info(hole_punch_did.dial_info, Vec::new())
                .await?
        );

        // Issue the signal
        let rpc = self.rpc_processor();
        network_result_try!(rpc
            .rpc_call_signal(
                Destination::Relay(relay_nr.clone(), target_nr.node_id()),
                None,
                SignalInfo::HolePunch { receipt, peer_info },
            )
            .await
            .wrap_err("failed to send signal")?);

        // Wait for the return receipt
        let inbound_nr = match eventual_value.await.take_value().unwrap() {
            ReceiptEvent::ReturnedOutOfBand => {
                bail!("hole punch receipt should be returned in-band");
            }
            ReceiptEvent::ReturnedInBand { inbound_noderef } => inbound_noderef,
            ReceiptEvent::Expired => {
                bail!("hole punch receipt expired from {}", target_nr);
            }
            ReceiptEvent::Cancelled => {
                bail!("hole punch receipt cancelled from {}", target_nr);
            }
        };

        // We expect the inbound noderef to be the same as the target noderef
        // if they aren't the same, we should error on this and figure out what then hell is up
        if target_nr != inbound_nr {
            bail!(
                "unexpected noderef mismatch on hole punch {}, expected {}",
                inbound_nr,
                target_nr
            );
        }

        // And now use the existing connection to send over
        if let Some(descriptor) = inbound_nr.last_connection().await {
            match self
                .net()
                .send_data_to_existing_connection(descriptor, data)
                .await?
            {
                None => Ok(NetworkResult::value(())),
                Some(_) => Ok(NetworkResult::no_connection_other(
                    "unable to send over hole punch",
                )),
            }
        } else {
            bail!("no hole punch available")
        }
    }

    // Send raw data to a node
    //
    // We may not have dial info for a node, but have an existing connection for it
    // because an inbound connection happened first, and no FindNodeQ has happened to that
    // node yet to discover its dial info. The existing connection should be tried first
    // in this case.
    //
    // Sending to a node requires determining a NetworkClass compatible mechanism
    //
    pub fn send_data(
        &self,
        node_ref: NodeRef,
        data: Vec<u8>,
    ) -> SendPinBoxFuture<EyreResult<NetworkResult<SendDataKind>>> {
        let this = self.clone();
        Box::pin(async move {
            // First try to send data to the last socket we've seen this peer on
            let data = if let Some(descriptor) = node_ref.last_connection().await {
                match this
                    .net()
                    .send_data_to_existing_connection(descriptor, data)
                    .await?
                {
                    None => {
                        return Ok(
                            if descriptor.matches_peer_scope(PeerScopeSet::only(PeerScope::Local)) {
                                NetworkResult::value(SendDataKind::LocalDirect)
                            } else {
                                NetworkResult::value(SendDataKind::GlobalDirect)
                            },
                        );
                    }
                    Some(d) => d,
                }
            } else {
                data
            };

            // If we don't have last_connection, try to reach out to the peer via its dial info
            let contact_method = this.get_contact_method(node_ref.clone());
            log_net!(
                "send_data via {:?} to dialinfo {:?}",
                contact_method,
                node_ref
            );
            match contact_method {
                ContactMethod::OutboundRelay(relay_nr) | ContactMethod::InboundRelay(relay_nr) => {
                    network_result_try!(this.send_data(relay_nr, data).await?);
                    Ok(NetworkResult::value(SendDataKind::GlobalIndirect))
                }
                ContactMethod::Direct(dial_info) => {
                    let send_data_kind = if dial_info.is_local() {
                        SendDataKind::LocalDirect
                    } else {
                        SendDataKind::GlobalDirect
                    };
                    network_result_try!(this.net().send_data_to_dial_info(dial_info, data).await?);
                    Ok(NetworkResult::value(send_data_kind))
                }
                ContactMethod::SignalReverse(relay_nr, target_node_ref) => {
                    network_result_try!(
                        this.do_reverse_connect(relay_nr, target_node_ref, data)
                            .await?
                    );
                    Ok(NetworkResult::value(SendDataKind::GlobalDirect))
                }
                ContactMethod::SignalHolePunch(relay_nr, target_node_ref) => {
                    network_result_try!(this.do_hole_punch(relay_nr, target_node_ref, data).await?);
                    Ok(NetworkResult::value(SendDataKind::GlobalDirect))
                }
                ContactMethod::Unreachable => Ok(NetworkResult::no_connection_other(
                    "Can't send to this node",
                )),
            }
        })
    }

    // Direct bootstrap request handler (separate fallback mechanism from cheaper TXT bootstrap mechanism)
    #[instrument(level = "trace", skip(self), ret, err)]
    async fn handle_boot_request(
        &self,
        descriptor: ConnectionDescriptor,
    ) -> EyreResult<NetworkResult<()>> {
        let routing_table = self.routing_table();

        // Get a bunch of nodes with the various
        let bootstrap_nodes = routing_table.find_bootstrap_nodes_filtered(2);

        // Serialize out peer info
        let bootstrap_peerinfo: Vec<PeerInfo> = bootstrap_nodes
            .iter()
            .filter_map(|b| b.peer_info())
            .collect();
        let json_bytes = serialize_json(bootstrap_peerinfo).as_bytes().to_vec();

        // Reply with a chunk of signed routing table
        match self
            .net()
            .send_data_to_existing_connection(descriptor, json_bytes)
            .await?
        {
            None => {
                // Bootstrap reply was sent
                Ok(NetworkResult::value(()))
            }
            Some(_) => Ok(NetworkResult::no_connection_other(
                "bootstrap reply could not be sent",
            )),
        }
    }

    // Direct bootstrap request
    #[instrument(level = "trace", err, skip(self))]
    pub async fn boot_request(&self, dial_info: DialInfo) -> EyreResult<Vec<PeerInfo>> {
        let timeout_ms = {
            let c = self.config.get();
            c.network.rpc.timeout_ms
        };
        // Send boot magic to requested peer address
        let data = BOOT_MAGIC.to_vec();
        let out_data: Vec<u8> = match self
            .net()
            .send_recv_data_unbound_to_dial_info(dial_info, data, timeout_ms)
            .await?
        {
            NetworkResult::Value(v) => v,
            _ => return Ok(Vec::new()),
        };

        let bootstrap_peerinfo: Vec<PeerInfo> =
            deserialize_json(std::str::from_utf8(&out_data).wrap_err("bad utf8 in boot peerinfo")?)
                .wrap_err("failed to deserialize boot peerinfo")?;

        Ok(bootstrap_peerinfo)
    }

    // Called when a packet potentially containing an RPC envelope is received by a low-level
    // network protocol handler. Processes the envelope, authenticates and decrypts the RPC message
    // and passes it to the RPC handler
    async fn on_recv_envelope(
        &self,
        data: &[u8],
        descriptor: ConnectionDescriptor,
    ) -> EyreResult<bool> {
        let root = span!(
            parent: None,
            Level::TRACE,
            "on_recv_envelope",
            "data.len" = data.len(),
            "descriptor" = ?descriptor
        );
        let _root_enter = root.enter();

        log_net!(
            "envelope of {} bytes received from {:?}",
            data.len(),
            descriptor
        );

        // Network accounting
        self.stats_packet_rcvd(descriptor.remote_address().to_ip_addr(), data.len() as u64);

        // If this is a zero length packet, just drop it, because these are used for hole punching
        // and possibly other low-level network connectivity tasks and will never require
        // more processing or forwarding
        if data.len() == 0 {
            return Ok(true);
        }

        // Ensure we can read the magic number
        if data.len() < 4 {
            log_net!(debug "short packet".green());
            return Ok(false);
        }

        // Is this a direct bootstrap request instead of an envelope?
        if data[0..4] == *BOOT_MAGIC {
            network_result_value_or_log!(debug self.handle_boot_request(descriptor).await? => {});
            return Ok(true);
        }

        // Is this an out-of-band receipt instead of an envelope?
        if data[0..4] == *RECEIPT_MAGIC {
            network_result_value_or_log!(debug self.handle_out_of_band_receipt(data).await => {});
            return Ok(true);
        }

        // Decode envelope header (may fail signature validation)
        let envelope = Envelope::from_signed_data(data).wrap_err("envelope failed to decode")?;

        // Get routing table and rpc processor
        let (routing_table, rpc) = {
            let inner = self.inner.lock();
            (
                inner.routing_table.as_ref().unwrap().clone(),
                inner.components.as_ref().unwrap().rpc_processor.clone(),
            )
        };

        // Get timestamp range
        let (tsbehind, tsahead) = {
            let c = self.config.get();
            (
                c.network.rpc.max_timestamp_behind_ms.map(ms_to_us),
                c.network.rpc.max_timestamp_ahead_ms.map(ms_to_us),
            )
        };

        // Validate timestamp isn't too old
        let ts = intf::get_timestamp();
        let ets = envelope.get_timestamp();
        if let Some(tsbehind) = tsbehind {
            if tsbehind > 0 && (ts > ets && ts - ets > tsbehind) {
                bail!(
                    "envelope time was too far in the past: {}ms ",
                    timestamp_to_secs(ts - ets) * 1000f64
                );
            }
        }
        if let Some(tsahead) = tsahead {
            if tsahead > 0 && (ts < ets && ets - ts > tsahead) {
                bail!(
                    "envelope time was too far in the future: {}ms",
                    timestamp_to_secs(ets - ts) * 1000f64
                );
            }
        }

        // Peek at header and see if we need to relay this
        // If the recipient id is not our node id, then it needs relaying
        let sender_id = envelope.get_sender_id();
        let recipient_id = envelope.get_recipient_id();
        if recipient_id != routing_table.node_id() {
            // See if the source node is allowed to resolve nodes
            // This is a costly operation, so only outbound-relay permitted
            // nodes are allowed to do this, for example PWA users

            let some_relay_nr = if self.check_client_whitelist(sender_id) {
                // Full relay allowed, do a full resolve_node
                rpc.resolve_node(recipient_id).await.wrap_err(
                    "failed to resolve recipient node for relay, dropping outbound relayed packet",
                )?
            } else {
                // If this is not a node in the client whitelist, only allow inbound relay
                // which only performs a lightweight lookup before passing the packet back out

                // See if we have the node in our routing table
                // We should, because relays are chosen by nodes that have established connectivity and
                // should be mutually in each others routing tables. The node needing the relay will be
                // pinging this node regularly to keep itself in the routing table
                routing_table.lookup_node_ref(recipient_id)
            };

            if let Some(relay_nr) = some_relay_nr {
                // Relay the packet to the desired destination
                log_net!("relaying {} bytes to {}", data.len(), relay_nr);
                network_result_value_or_log!(debug self.send_data(relay_nr, data.to_vec())
                    .await
                    .wrap_err("failed to forward envelope")? => {
                        return Ok(false);
                    }
                );
            }
            // Inform caller that we dealt with the envelope, but did not process it locally
            return Ok(false);
        }

        // DH to get decryption key (cached)
        let node_id_secret = routing_table.node_id_secret();

        // Decrypt the envelope body
        // xxx: punish nodes that send messages that fail to decrypt eventually
        let body = envelope
            .decrypt_body(self.crypto(), data, &node_id_secret)
            .wrap_err("failed to decrypt envelope body")?;

        // Cache the envelope information in the routing table
        let source_noderef = match routing_table.register_node_with_existing_connection(
            envelope.get_sender_id(),
            descriptor,
            ts,
        ) {
            None => {
                // If the node couldn't be registered just skip this envelope,
                // the error will have already been logged
                return Ok(false);
            }
            Some(v) => v,
        };
        source_noderef.operate_mut(|e| e.set_min_max_version(envelope.get_min_max_version()));

        // xxx: deal with spoofing and flooding here?

        // Pass message to RPC system
        rpc.enqueue_message(envelope, body, source_noderef)?;

        // Inform caller that we dealt with the envelope locally
        Ok(true)
    }

    // Callbacks from low level network for statistics gathering
    pub fn stats_packet_sent(&self, addr: IpAddr, bytes: u64) {
        let inner = &mut *self.inner.lock();
        inner
            .stats
            .self_stats
            .transfer_stats_accounting
            .add_up(bytes);
        inner
            .stats
            .per_address_stats
            .entry(PerAddressStatsKey(addr))
            .or_insert(PerAddressStats::default())
            .transfer_stats_accounting
            .add_up(bytes);
    }

    pub fn stats_packet_rcvd(&self, addr: IpAddr, bytes: u64) {
        let inner = &mut *self.inner.lock();
        inner
            .stats
            .self_stats
            .transfer_stats_accounting
            .add_down(bytes);
        inner
            .stats
            .per_address_stats
            .entry(PerAddressStatsKey(addr))
            .or_insert(PerAddressStats::default())
            .transfer_stats_accounting
            .add_down(bytes);
    }

    // Get stats
    pub fn get_stats(&self) -> NetworkManagerStats {
        let inner = self.inner.lock();
        inner.stats.clone()
    }

    fn get_veilid_state_inner(inner: &NetworkManagerInner) -> VeilidStateNetwork {
        if inner.components.is_some() && inner.components.as_ref().unwrap().net.is_started() {
            VeilidStateNetwork {
                started: true,
                bps_down: inner.stats.self_stats.transfer_stats.down.average,
                bps_up: inner.stats.self_stats.transfer_stats.up.average,
            }
        } else {
            VeilidStateNetwork {
                started: false,
                bps_down: 0,
                bps_up: 0,
            }
        }
    }
    pub fn get_veilid_state(&self) -> VeilidStateNetwork {
        let inner = self.inner.lock();
        Self::get_veilid_state_inner(&*inner)
    }

    fn send_network_update(&self) {
        let (update_cb, state) = {
            let inner = self.inner.lock();
            let update_cb = inner.update_callback.clone();
            if update_cb.is_none() {
                return;
            }
            let state = Self::get_veilid_state_inner(&*inner);
            (update_cb.unwrap(), state)
        };
        update_cb(VeilidUpdate::Network(state));
    }

    // Determine if a local IP address has changed
    // this means we should restart the low level network and and recreate all of our dial info
    // Wait until we have received confirmation from N different peers
    pub async fn report_local_socket_address(
        &self,
        _socket_address: SocketAddress,
        _reporting_peer: NodeRef,
    ) {
        // XXX: Nothing here yet.
    }

    // Determine if a global IP address has changed
    // this means we should recreate our public dial info if it is not static and rediscover it
    // Wait until we have received confirmation from N different peers
    pub async fn report_global_socket_address(
        &self,
        socket_address: SocketAddress,
        reporting_peer: NodeRef,
    ) {
        let (net, routing_table) = {
            let mut inner = self.inner.lock();

            // Store the reported address
            inner
                .public_address_check_cache
                .insert(reporting_peer.node_id(), socket_address);

            let net = inner.components.as_ref().unwrap().net.clone();
            let routing_table = inner.routing_table.as_ref().unwrap().clone();
            (net, routing_table)
        };
        let network_class = net.get_network_class().unwrap_or(NetworkClass::Invalid);

        // Determine if our external address has likely changed
        let needs_public_address_detection =
            if matches!(network_class, NetworkClass::InboundCapable) {
                // Get current external ip/port from registered global dialinfo
                let current_addresses: BTreeSet<SocketAddress> = routing_table
                    .all_filtered_dial_info_details(
                        Some(RoutingDomain::PublicInternet),
                        &DialInfoFilter::all(),
                    )
                    .iter()
                    .map(|did| did.dial_info.socket_address())
                    .collect();

                // If we are inbound capable, but start to see inconsistent socket addresses from multiple reporting peers
                // then we zap the network class and re-detect it
                let inner = self.inner.lock();
                let mut inconsistencies = 0;
                let mut changed = false;
                // Iteration goes from most recent to least recent node/address pair
                for (_, a) in &inner.public_address_check_cache {
                    if !current_addresses.contains(a) {
                        inconsistencies += 1;
                        if inconsistencies >= GLOBAL_ADDRESS_CHANGE_DETECTION_COUNT {
                            changed = true;
                            break;
                        }
                    }
                }
                changed
            } else {
                // If we are currently outbound only, we don't have any public dial info
                // but if we are starting to see consistent socket address from multiple reporting peers
                // then we may be become inbound capable, so zap the network class so we can re-detect it and any public dial info

                let inner = self.inner.lock();
                let mut consistencies = 0;
                let mut consistent = false;
                let mut current_address = Option::<SocketAddress>::None;
                // Iteration goes from most recent to least recent node/address pair
                for (_, a) in &inner.public_address_check_cache {
                    if let Some(current_address) = current_address {
                        if current_address == *a {
                            consistencies += 1;
                            if consistencies >= GLOBAL_ADDRESS_CHANGE_DETECTION_COUNT {
                                consistent = true;
                                break;
                            }
                        }
                    } else {
                        current_address = Some(*a);
                    }
                }
                consistent
            };

        if needs_public_address_detection {
            // Reset the address check cache now so we can start detecting fresh
            let mut inner = self.inner.lock();
            inner.public_address_check_cache.clear();

            // Reset the network class and dial info so we can re-detect it
            routing_table.clear_dial_info_details(RoutingDomain::PublicInternet);
            net.reset_network_class();
        }
    }

    // Inform routing table entries that our dial info has changed
    pub async fn send_node_info_updates(&self, all: bool) {
        let this = self.clone();

        // Run in background only once
        let _ = self
            .clone()
            .unlocked_inner
            .node_info_update_single_future
            .single_spawn(async move {
                // Only update if we actually have a valid network class
                if matches!(
                    this.get_network_class().unwrap_or(NetworkClass::Invalid),
                    NetworkClass::Invalid
                ) {
                    trace!(
                        "not sending node info update because our network class is not yet valid"
                    );
                    return;
                }

                // Get the list of refs to all nodes to update
                let cur_ts = intf::get_timestamp();
                let node_refs = this.routing_table().get_nodes_needing_updates(cur_ts, all);

                // Send the updates
                log_net!(debug "Sending node info updates to {} nodes", node_refs.len());
                let mut unord = FuturesUnordered::new();
                for nr in node_refs {
                    let rpc = this.rpc_processor();
                    unord.push(async move {
                        // Update the node
                        if let Err(e) = rpc
                            .rpc_call_node_info_update(Destination::Direct(nr.clone()), None)
                            .await
                        {
                            // Not fatal, but we should be able to see if this is happening
                            trace!("failed to send node info update to {:?}: {}", nr, e);
                            return;
                        }

                        // Mark the node as updated
                        nr.set_seen_our_node_info();
                    });
                }

                // Wait for futures to complete
                while unord.next().await.is_some() {}

                log_rtab!(debug "Finished sending node updates");
            })
            .await;
    }
}

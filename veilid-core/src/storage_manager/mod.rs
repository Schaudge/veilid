mod debug;
mod get_value;
mod inspect_value;
mod record_store;
mod set_value;
mod storage_manager_inner;
mod tasks;
mod types;
mod watch_value;

use super::*;
use network_manager::*;
use record_store::*;
use routing_table::*;
use rpc_processor::*;
use storage_manager_inner::*;

pub use record_store::{WatchParameters, WatchResult};
pub use types::*;

/// The maximum size of a single subkey
const MAX_SUBKEY_SIZE: usize = ValueData::MAX_LEN;
/// The maximum total size of all subkeys of a record
const MAX_RECORD_DATA_SIZE: usize = 1_048_576;
/// Frequency to flush record stores to disk
const FLUSH_RECORD_STORES_INTERVAL_SECS: u32 = 1;
/// Frequency to check for offline subkeys writes to send to the network
const OFFLINE_SUBKEY_WRITES_INTERVAL_SECS: u32 = 5;
/// Frequency to send ValueChanged notifications to the network
const SEND_VALUE_CHANGES_INTERVAL_SECS: u32 = 1;
/// Frequency to check for dead nodes and routes for client-side active watches
const CHECK_ACTIVE_WATCHES_INTERVAL_SECS: u32 = 1;
/// Frequency to check for expired server-side watched records
const CHECK_WATCHED_RECORDS_INTERVAL_SECS: u32 = 1;

#[derive(Debug, Clone)]
/// A single 'value changed' message to send
struct ValueChangedInfo {
    target: Target,
    key: TypedKey,
    subkeys: ValueSubkeyRangeSet,
    count: u32,
    watch_id: u64,
    value: Option<Arc<SignedValueData>>,
}

struct StorageManagerUnlockedInner {
    config: VeilidConfig,
    crypto: Crypto,
    table_store: TableStore,
    #[cfg(feature = "unstable-blockstore")]
    block_store: BlockStore,

    // Background processes
    flush_record_stores_task: TickTask<EyreReport>,
    offline_subkey_writes_task: TickTask<EyreReport>,
    send_value_changes_task: TickTask<EyreReport>,
    check_active_watches_task: TickTask<EyreReport>,
    check_watched_records_task: TickTask<EyreReport>,

    // Anonymous watch keys
    anonymous_watch_keys: TypedKeyPairGroup,
}

#[derive(Clone)]
pub(crate) struct StorageManager {
    unlocked_inner: Arc<StorageManagerUnlockedInner>,
    inner: Arc<AsyncMutex<StorageManagerInner>>,
}

impl StorageManager {
    fn new_unlocked_inner(
        config: VeilidConfig,
        crypto: Crypto,
        table_store: TableStore,
        #[cfg(feature = "unstable-blockstore")] block_store: BlockStore,
    ) -> StorageManagerUnlockedInner {
        // Generate keys to use for anonymous watches
        let mut anonymous_watch_keys = TypedKeyPairGroup::new();
        for ck in VALID_CRYPTO_KINDS {
            let vcrypto = crypto.get(ck).unwrap();
            let kp = vcrypto.generate_keypair();
            anonymous_watch_keys.add(TypedKeyPair::new(ck, kp));
        }

        StorageManagerUnlockedInner {
            config,
            crypto,
            table_store,
            #[cfg(feature = "unstable-blockstore")]
            block_store,
            flush_record_stores_task: TickTask::new(
                "flush_record_stores_task",
                FLUSH_RECORD_STORES_INTERVAL_SECS,
            ),
            offline_subkey_writes_task: TickTask::new(
                "offline_subkey_writes_task",
                OFFLINE_SUBKEY_WRITES_INTERVAL_SECS,
            ),
            send_value_changes_task: TickTask::new(
                "send_value_changes_task",
                SEND_VALUE_CHANGES_INTERVAL_SECS,
            ),
            check_active_watches_task: TickTask::new(
                "check_active_watches_task",
                CHECK_ACTIVE_WATCHES_INTERVAL_SECS,
            ),
            check_watched_records_task: TickTask::new(
                "check_watched_records_task",
                CHECK_WATCHED_RECORDS_INTERVAL_SECS,
            ),

            anonymous_watch_keys,
        }
    }
    fn new_inner(unlocked_inner: Arc<StorageManagerUnlockedInner>) -> StorageManagerInner {
        StorageManagerInner::new(unlocked_inner)
    }

    pub fn new(
        config: VeilidConfig,
        crypto: Crypto,
        table_store: TableStore,
        #[cfg(feature = "unstable-blockstore")] block_store: BlockStore,
    ) -> StorageManager {
        let unlocked_inner = Arc::new(Self::new_unlocked_inner(
            config,
            crypto,
            table_store,
            #[cfg(feature = "unstable-blockstore")]
            block_store,
        ));
        let this = StorageManager {
            unlocked_inner: unlocked_inner.clone(),
            inner: Arc::new(AsyncMutex::new(Self::new_inner(unlocked_inner))),
        };

        this.setup_tasks();

        this
    }

    #[instrument(level = "debug", skip_all, err)]
    pub async fn init(&self, update_callback: UpdateCallback) -> EyreResult<()> {
        log_stor!(debug "startup storage manager");

        let mut inner = self.inner.lock().await;
        inner.init(self.clone(), update_callback).await?;

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn terminate(&self) {
        log_stor!(debug "starting storage manager shutdown");

        {
            let mut inner = self.inner.lock().await;
            inner.terminate().await;
        }

        // Cancel all tasks
        self.cancel_tasks().await;

        // Release the storage manager
        {
            let mut inner = self.inner.lock().await;
            *inner = Self::new_inner(self.unlocked_inner.clone());
        }

        log_stor!(debug "finished storage manager shutdown");
    }

    pub async fn set_rpc_processor(&self, opt_rpc_processor: Option<RPCProcessor>) {
        let mut inner = self.inner.lock().await;
        inner.opt_rpc_processor = opt_rpc_processor
    }

    pub async fn set_routing_table(&self, opt_routing_table: Option<RoutingTable>) {
        let mut inner = self.inner.lock().await;
        inner.opt_routing_table = opt_routing_table
    }

    async fn lock(&self) -> VeilidAPIResult<AsyncMutexGuardArc<StorageManagerInner>> {
        let inner = asyncmutex_lock_arc!(&self.inner);
        if !inner.initialized {
            apibail_not_initialized!();
        }
        Ok(inner)
    }

    fn online_ready_inner(inner: &StorageManagerInner) -> Option<RPCProcessor> {
        if let Some(rpc_processor) = { inner.opt_rpc_processor.clone() } {
            if let Some(network_class) = rpc_processor
                .routing_table()
                .get_network_class(RoutingDomain::PublicInternet)
            {
                // If our PublicInternet network class is valid we're ready to talk
                if network_class != NetworkClass::Invalid {
                    Some(rpc_processor)
                } else {
                    None
                }
            } else {
                // If we haven't gotten a network class yet we shouldnt try to use the DHT
                None
            }
        } else {
            // If we aren't attached, we won't have an rpc processor
            None
        }
    }

    async fn online_writes_ready(&self) -> EyreResult<Option<RPCProcessor>> {
        let inner = self.lock().await?;
        Ok(Self::online_ready_inner(&inner))
    }

    async fn has_offline_subkey_writes(&self) -> EyreResult<bool> {
        let inner = self.lock().await?;
        Ok(!inner.offline_subkey_writes.is_empty())
    }

    /// Get the set of nodes in our active watches
    pub async fn get_active_watch_nodes(&self) -> Vec<NodeRef> {
        let inner = self.inner.lock().await;
        inner
            .opened_records
            .values()
            .filter_map(|v| v.active_watch().map(|aw| aw.watch_node))
            .collect()
    }

    /// Create a local record from scratch with a new owner key, open it, and return the opened descriptor
    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn create_record(
        &self,
        kind: CryptoKind,
        schema: DHTSchema,
        safety_selection: SafetySelection,
    ) -> VeilidAPIResult<DHTRecordDescriptor> {
        let mut inner = self.lock().await?;
        schema.validate()?;

        // Create a new owned local record from scratch
        let (key, owner) = inner
            .create_new_owned_local_record(kind, schema, safety_selection)
            .await?;

        // Now that the record is made we should always succeed to open the existing record
        // The initial writer is the owner of the record
        inner
            .open_existing_record(key, Some(owner), safety_selection)
            .await
            .map(|r| r.unwrap())
    }

    /// Open an existing local record if it exists, and if it doesnt exist locally, try to pull it from the network and open it and return the opened descriptor
    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn open_record(
        &self,
        key: TypedKey,
        writer: Option<KeyPair>,
        safety_selection: SafetySelection,
    ) -> VeilidAPIResult<DHTRecordDescriptor> {
        let mut inner = self.lock().await?;

        // See if we have a local record already or not
        if let Some(res) = inner
            .open_existing_record(key, writer, safety_selection)
            .await?
        {
            return Ok(res);
        }

        // No record yet, try to get it from the network

        // Get rpc processor and drop mutex so we don't block while getting the value from the network
        let Some(rpc_processor) = Self::online_ready_inner(&inner) else {
            apibail_try_again!("offline, try again later");
        };

        // Drop the mutex so we dont block during network access
        drop(inner);

        // No last descriptor, no last value
        // Use the safety selection we opened the record with
        let subkey: ValueSubkey = 0;
        let res_rx = self
            .outbound_get_value(
                rpc_processor,
                key,
                subkey,
                safety_selection,
                GetResult::default(),
            )
            .await?;
        // Wait for the first result
        let Ok(result) = res_rx.recv_async().await else {
            apibail_internal!("failed to receive results");
        };
        let result = result?;

        // If we got nothing back, the key wasn't found
        if result.get_result.opt_value.is_none() && result.get_result.opt_descriptor.is_none() {
            // No result
            apibail_key_not_found!(key);
        };
        let opt_last_seq = result
            .get_result
            .opt_value
            .as_ref()
            .map(|s| s.value_data().seq());

        // Reopen inner to store value we just got
        let mut inner = self.lock().await?;

        // Check again to see if we have a local record already or not
        // because waiting for the outbound_get_value action could result in the key being opened
        // via some parallel process

        if let Some(res) = inner
            .open_existing_record(key, writer, safety_selection)
            .await?
        {
            return Ok(res);
        }

        // Open the new record
        let out = inner
            .open_new_record(key, writer, subkey, result.get_result, safety_selection)
            .await;

        if out.is_ok() {
            if let Some(last_seq) = opt_last_seq {
                self.process_deferred_outbound_get_value_result_inner(
                    &mut inner, res_rx, key, subkey, last_seq,
                );
            }
        }
        out
    }

    /// Close an opened local record
    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn close_record(&self, key: TypedKey) -> VeilidAPIResult<()> {
        let (opt_opened_record, opt_rpc_processor) = {
            let mut inner = self.lock().await?;
            (inner.close_record(key)?, Self::online_ready_inner(&inner))
        };

        // Send a one-time cancel request for the watch if we have one and we're online
        if let Some(opened_record) = opt_opened_record {
            if let Some(active_watch) = opened_record.active_watch() {
                if let Some(rpc_processor) = opt_rpc_processor {
                    // Use the safety selection we opened the record with
                    // Use the writer we opened with as the 'watcher' as well
                    let opt_owvresult = match self
                        .outbound_watch_value_cancel(
                            rpc_processor,
                            key,
                            ValueSubkeyRangeSet::full(),
                            opened_record.safety_selection(),
                            opened_record.writer().cloned(),
                            active_watch.id,
                            active_watch.watch_node,
                        )
                        .await
                    {
                        Ok(v) => v,
                        Err(e) => {
                            log_stor!(debug
                                "close record watch cancel failed: {}", e
                            );
                            None
                        }
                    };
                    if let Some(owvresult) = opt_owvresult {
                        if owvresult.expiration_ts.as_u64() != 0 {
                            log_stor!(debug
                                "close record watch cancel should have zero expiration"
                            );
                        }
                    } else {
                        log_stor!(debug "close record watch cancel unsuccessful");
                    }
                } else {
                    log_stor!(debug "skipping last-ditch watch cancel because we are offline");
                }
            }
        }

        Ok(())
    }

    /// Delete a local record
    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn delete_record(&self, key: TypedKey) -> VeilidAPIResult<()> {
        // Ensure the record is closed
        self.close_record(key).await?;

        // Get record from the local store
        let mut inner = self.lock().await?;
        let Some(local_record_store) = inner.local_record_store.as_mut() else {
            apibail_not_initialized!();
        };

        // Remove the record from the local store
        local_record_store.delete_record(key).await
    }

    /// Get the value of a subkey from an opened local record
    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn get_value(
        &self,
        key: TypedKey,
        subkey: ValueSubkey,
        force_refresh: bool,
    ) -> VeilidAPIResult<Option<ValueData>> {
        let mut inner = self.lock().await?;
        let safety_selection = {
            let Some(opened_record) = inner.opened_records.get(&key) else {
                apibail_generic!("record not open");
            };
            opened_record.safety_selection()
        };

        // See if the requested subkey is our local record store
        let last_get_result = inner.handle_get_local_value(key, subkey, true).await?;

        // Return the existing value if we have one unless we are forcing a refresh
        if !force_refresh {
            if let Some(last_get_result_value) = last_get_result.opt_value {
                return Ok(Some(last_get_result_value.value_data().clone()));
            }
        }

        // Refresh if we can

        // Get rpc processor and drop mutex so we don't block while getting the value from the network
        let Some(rpc_processor) = Self::online_ready_inner(&inner) else {
            // Return the existing value if we have one if we aren't online
            if let Some(last_get_result_value) = last_get_result.opt_value {
                return Ok(Some(last_get_result_value.value_data().clone()));
            }
            apibail_try_again!("offline, try again later");
        };

        // Drop the lock for network access
        drop(inner);

        // May have last descriptor / value
        // Use the safety selection we opened the record with
        let opt_last_seq = last_get_result
            .opt_value
            .as_ref()
            .map(|v| v.value_data().seq());
        let res_rx = self
            .outbound_get_value(
                rpc_processor,
                key,
                subkey,
                safety_selection,
                last_get_result,
            )
            .await?;

        // Wait for the first result
        let Ok(result) = res_rx.recv_async().await else {
            apibail_internal!("failed to receive results");
        };
        let result = result?;
        let partial = result.fanout_result.kind.is_partial();

        // Process the returned result
        let out = self
            .process_outbound_get_value_result(key, subkey, opt_last_seq, result)
            .await?;

        if let Some(out) = &out {
            // If there's more to process, do it in the background
            if partial {
                let mut inner = self.lock().await?;
                self.process_deferred_outbound_get_value_result_inner(
                    &mut inner,
                    res_rx,
                    key,
                    subkey,
                    out.seq(),
                );
            }
        }

        Ok(out)
    }

    /// Set the value of a subkey on an opened local record
    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn set_value(
        &self,
        key: TypedKey,
        subkey: ValueSubkey,
        data: Vec<u8>,
        writer: Option<KeyPair>,
    ) -> VeilidAPIResult<Option<ValueData>> {
        let mut inner = self.lock().await?;

        // Get cryptosystem
        let Some(vcrypto) = self.unlocked_inner.crypto.get(key.kind) else {
            apibail_generic!("unsupported cryptosystem");
        };

        let (safety_selection, opt_writer) = {
            let Some(opened_record) = inner.opened_records.get(&key) else {
                apibail_generic!("record not open");
            };
            (
                opened_record.safety_selection(),
                opened_record.writer().cloned(),
            )
        };

        // Use the specified writer, or if not specified, the default writer when the record was opened
        let opt_writer = writer.or(opt_writer);

        // If we don't have a writer then we can't write
        let Some(writer) = opt_writer else {
            apibail_generic!("value is not writable");
        };

        // See if the subkey we are modifying has a last known local value
        let last_get_result = inner.handle_get_local_value(key, subkey, true).await?;

        // Get the descriptor and schema for the key
        let Some(descriptor) = last_get_result.opt_descriptor else {
            apibail_generic!("must have a descriptor");
        };
        let schema = descriptor.schema()?;

        // Make new subkey data
        let value_data = if let Some(last_signed_value_data) = last_get_result.opt_value {
            if last_signed_value_data.value_data().data() == data
                && last_signed_value_data.value_data().writer() == &writer.key
            {
                // Data and writer is the same, nothing is changing,
                // just return that we set it, but no network activity needs to happen
                return Ok(None);
            }
            let seq = last_signed_value_data.value_data().seq();
            ValueData::new_with_seq(seq + 1, data, writer.key)?
        } else {
            ValueData::new(data, writer.key)?
        };

        // Validate with schema
        if !schema.check_subkey_value_data(descriptor.owner(), subkey, &value_data) {
            // Validation failed, ignore this value
            apibail_generic!("failed schema validation");
        }

        // Sign the new value data with the writer
        let signed_value_data = Arc::new(SignedValueData::make_signature(
            value_data,
            descriptor.owner(),
            subkey,
            vcrypto,
            writer.secret,
        )?);

        // Write the value locally first
        log_stor!(debug "Writing subkey locally: {}:{} len={}", key, subkey, signed_value_data.value_data().data().len() );
        inner
            .handle_set_local_value(
                key,
                subkey,
                signed_value_data.clone(),
                WatchUpdateMode::NoUpdate,
            )
            .await?;

        // Get rpc processor and drop mutex so we don't block while getting the value from the network
        let Some(rpc_processor) = Self::online_ready_inner(&inner) else {
            log_stor!(debug "Writing subkey offline: {}:{} len={}", key, subkey, signed_value_data.value_data().data().len() );
            // Add to offline writes to flush
            inner.add_offline_subkey_write(key, subkey, safety_selection);
            return Ok(None);
        };

        // Drop the lock for network access
        drop(inner);

        log_stor!(debug "Writing subkey to the network: {}:{} len={}", key, subkey, signed_value_data.value_data().data().len() );

        // Use the safety selection we opened the record with
        let res_rx = match self
            .outbound_set_value(
                rpc_processor,
                key,
                subkey,
                safety_selection,
                signed_value_data.clone(),
                descriptor,
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                // Failed to write, try again later
                let mut inner = self.lock().await?;
                inner.add_offline_subkey_write(key, subkey, safety_selection);
                return Err(e);
            }
        };

        // Wait for the first result
        let Ok(result) = res_rx.recv_async().await else {
            apibail_internal!("failed to receive results");
        };
        let result = result?;
        let partial = result.fanout_result.kind.is_partial();

        // Process the returned result
        let out = self
            .process_outbound_set_value_result(
                key,
                subkey,
                signed_value_data.value_data().clone(),
                safety_selection,
                result,
            )
            .await?;

        // If there's more to process, do it in the background
        if partial {
            let mut inner = self.lock().await?;
            self.process_deferred_outbound_set_value_result_inner(
                &mut inner,
                res_rx,
                key,
                subkey,
                out.clone()
                    .unwrap_or_else(|| signed_value_data.value_data().clone()),
                safety_selection,
            );
        }

        Ok(out)
    }

    /// Create,update or cancel an outbound watch to a DHT value
    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn watch_values(
        &self,
        key: TypedKey,
        subkeys: ValueSubkeyRangeSet,
        expiration: Timestamp,
        count: u32,
    ) -> VeilidAPIResult<Timestamp> {
        let inner = self.lock().await?;

        // Get the safety selection and the writer we opened this record
        // and whatever active watch id and watch node we may have in case this is a watch update
        let (safety_selection, opt_writer, opt_watch_id, opt_watch_node) = {
            let Some(opened_record) = inner.opened_records.get(&key) else {
                apibail_generic!("record not open");
            };
            (
                opened_record.safety_selection(),
                opened_record.writer().cloned(),
                opened_record.active_watch().map(|aw| aw.id),
                opened_record.active_watch().map(|aw| aw.watch_node.clone()),
            )
        };

        // Rewrite subkey range if empty to full
        let subkeys = if subkeys.is_empty() {
            ValueSubkeyRangeSet::full()
        } else {
            subkeys
        };

        // Get the schema so we can truncate the watch to the number of subkeys
        let schema = if let Some(lrs) = inner.local_record_store.as_ref() {
            let Some(schema) = lrs.peek_record(key, |r| r.schema()) else {
                apibail_generic!("no local record found");
            };
            schema
        } else {
            apibail_not_initialized!();
        };
        let subkeys = schema.truncate_subkeys(&subkeys, None);

        // Get rpc processor and drop mutex so we don't block while requesting the watch from the network
        let Some(rpc_processor) = Self::online_ready_inner(&inner) else {
            apibail_try_again!("offline, try again later");
        };

        // Drop the lock for network access
        drop(inner);

        // Use the safety selection we opened the record with
        // Use the writer we opened with as the 'watcher' as well
        let opt_owvresult = self
            .outbound_watch_value(
                rpc_processor,
                key,
                subkeys.clone(),
                expiration,
                count,
                safety_selection,
                opt_writer,
                opt_watch_id,
                opt_watch_node,
            )
            .await?;
        // If we did not get a valid response assume nothing changed
        let Some(owvresult) = opt_owvresult else {
            apibail_try_again!("did not get a valid response");
        };

        // Clear any existing watch if the watch succeeded or got cancelled
        let mut inner = self.lock().await?;
        let Some(opened_record) = inner.opened_records.get_mut(&key) else {
            apibail_generic!("record not open");
        };
        opened_record.clear_active_watch();

        // Get the minimum expiration timestamp we will accept
        let (rpc_timeout_us, max_watch_expiration_us) = {
            let c = self.unlocked_inner.config.get();
            (
                TimestampDuration::from(ms_to_us(c.network.rpc.timeout_ms)),
                TimestampDuration::from(ms_to_us(c.network.dht.max_watch_expiration_ms)),
            )
        };
        let cur_ts = get_timestamp();
        let min_expiration_ts = cur_ts + rpc_timeout_us.as_u64();
        let max_expiration_ts = if expiration.as_u64() == 0 {
            cur_ts + max_watch_expiration_us.as_u64()
        } else {
            expiration.as_u64()
        };

        // If the expiration time is less than our minimum expiration time (or zero) consider this watch inactive
        let mut expiration_ts = owvresult.expiration_ts;
        if expiration_ts.as_u64() < min_expiration_ts {
            return Ok(Timestamp::new(0));
        }

        // If the expiration time is greater than our maximum expiration time, clamp our local watch so we ignore extra valuechanged messages
        if expiration_ts.as_u64() > max_expiration_ts {
            expiration_ts = Timestamp::new(max_expiration_ts);
        }

        // If we requested a cancellation, then consider this watch cancelled
        if count == 0 {
            // Expiration returned should be zero if we requested a cancellation
            if expiration_ts.as_u64() != 0 {
                log_stor!(debug "got active watch despite asking for a cancellation");
            }
            return Ok(Timestamp::new(0));
        }

        // Keep a record of the watch
        opened_record.set_active_watch(ActiveWatch {
            id: owvresult.watch_id,
            expiration_ts,
            watch_node: owvresult.watch_node,
            opt_value_changed_route: owvresult.opt_value_changed_route,
            subkeys,
            count,
        });

        Ok(owvresult.expiration_ts)
    }

    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn cancel_watch_values(
        &self,
        key: TypedKey,
        subkeys: ValueSubkeyRangeSet,
    ) -> VeilidAPIResult<bool> {
        let (subkeys, active_watch) = {
            let inner = self.lock().await?;
            let Some(opened_record) = inner.opened_records.get(&key) else {
                apibail_generic!("record not open");
            };

            // See what watch we have currently if any
            let Some(active_watch) = opened_record.active_watch() else {
                // If we didn't have an active watch, then we can just return false because there's nothing to do here
                return Ok(false);
            };

            // Rewrite subkey range if empty to full
            let subkeys = if subkeys.is_empty() {
                ValueSubkeyRangeSet::full()
            } else {
                subkeys
            };

            // Reduce the subkey range
            let new_subkeys = active_watch.subkeys.difference(&subkeys);

            (new_subkeys, active_watch)
        };

        // If we have no subkeys left, then set the count to zero to indicate a full cancellation
        let count = if subkeys.is_empty() {
            0
        } else {
            active_watch.count
        };

        // Update the watch. This just calls through to the above watch_values() function
        // This will update the active_watch so we don't need to do that in this routine.
        let expiration_ts = self
            .watch_values(key, subkeys, active_watch.expiration_ts, count)
            .await?;

        // A zero expiration time returned from watch_value() means the watch is done
        // or no subkeys are left, and the watch is no longer active
        if expiration_ts.as_u64() == 0 {
            // Return false indicating the watch is completely gone
            return Ok(false);
        }

        // Return true because the the watch was changed
        Ok(true)
    }

    /// Inspect an opened DHT record for its subkey sequence numbers
    #[instrument(level = "trace", target = "stor", skip_all)]
    pub async fn inspect_record(
        &self,
        key: TypedKey,
        subkeys: ValueSubkeyRangeSet,
        scope: DHTReportScope,
    ) -> VeilidAPIResult<DHTRecordReport> {
        let subkeys = if subkeys.is_empty() {
            ValueSubkeyRangeSet::full()
        } else {
            subkeys
        };

        let mut inner = self.lock().await?;
        let safety_selection = {
            let Some(opened_record) = inner.opened_records.get(&key) else {
                apibail_generic!("record not open");
            };
            opened_record.safety_selection()
        };

        // See if the requested record is our local record store
        let mut local_inspect_result = inner
            .handle_inspect_local_value(key, subkeys.clone(), true)
            .await?;

        #[allow(clippy::unnecessary_cast)]
        {
            assert!(
                local_inspect_result.subkeys.len() as u64 == local_inspect_result.seqs.len() as u64,
                "mismatch between local subkeys returned and sequence number list returned"
            );
        }
        assert!(
            local_inspect_result.subkeys.is_subset(&subkeys),
            "more subkeys returned locally than requested"
        );

        // Get the offline subkeys for this record still only returning the ones we're inspecting
        let offline_subkey_writes = inner
            .offline_subkey_writes
            .get(&key)
            .map(|o| o.subkeys.clone())
            .unwrap_or_default()
            .intersect(&subkeys);

        // If this is the maximum scope we're interested in, return the report
        if matches!(scope, DHTReportScope::Local) {
            return Ok(DHTRecordReport::new(
                local_inspect_result.subkeys,
                offline_subkey_writes,
                local_inspect_result.seqs,
                vec![],
            ));
        }

        // Get rpc processor and drop mutex so we don't block while getting the value from the network
        let Some(rpc_processor) = Self::online_ready_inner(&inner) else {
            apibail_try_again!("offline, try again later");
        };

        // Drop the lock for network access
        drop(inner);

        // If we're simulating a set, increase the previous sequence number we have by 1
        if matches!(scope, DHTReportScope::UpdateSet) {
            for seq in &mut local_inspect_result.seqs {
                *seq = seq.overflowing_add(1).0;
            }
        }

        // Get the inspect record report from the network
        let result = self
            .outbound_inspect_value(
                rpc_processor,
                key,
                subkeys,
                safety_selection,
                if matches!(scope, DHTReportScope::SyncGet | DHTReportScope::SyncSet) {
                    InspectResult::default()
                } else {
                    local_inspect_result.clone()
                },
                matches!(scope, DHTReportScope::UpdateSet | DHTReportScope::SyncSet),
            )
            .await?;

        // Sanity check before zip
        #[allow(clippy::unnecessary_cast)]
        {
            assert_eq!(
                result.inspect_result.subkeys.len() as u64,
                result.fanout_results.len() as u64,
                "mismatch between subkeys returned and fanout results returned"
            );
        }
        if !local_inspect_result.subkeys.is_empty() && !result.inspect_result.subkeys.is_empty() {
            assert_eq!(
                result.inspect_result.subkeys.len(),
                local_inspect_result.subkeys.len(),
                "mismatch between local subkeys returned and network results returned"
            );
        }

        // Keep the list of nodes that returned a value for later reference
        let mut inner = self.lock().await?;
        let results_iter = result
            .inspect_result
            .subkeys
            .iter()
            .zip(result.fanout_results.iter());

        inner.process_fanout_results(key, results_iter, false);

        Ok(DHTRecordReport::new(
            result.inspect_result.subkeys,
            offline_subkey_writes,
            local_inspect_result.seqs,
            result.inspect_result.seqs,
        ))
    }

    // Send single value change out to the network
    #[instrument(level = "trace", target = "stor", skip(self), err)]
    async fn send_value_change(&self, vc: ValueChangedInfo) -> VeilidAPIResult<()> {
        let rpc_processor = {
            let inner = self.inner.lock().await;
            if let Some(rpc_processor) = Self::online_ready_inner(&inner) {
                rpc_processor.clone()
            } else {
                apibail_try_again!("network is not available");
            }
        };

        let dest = rpc_processor
            .resolve_target_to_destination(
                vc.target,
                SafetySelection::Unsafe(Sequencing::NoPreference),
            )
            .await
            .map_err(VeilidAPIError::from)?;

        network_result_value_or_log!(rpc_processor
            .rpc_call_value_changed(dest, vc.key, vc.subkeys.clone(), vc.count, vc.watch_id, vc.value.map(|v| (*v).clone()) )
            .await
            .map_err(VeilidAPIError::from)? => [format!(": dest={:?} vc={:?}", dest, vc)] {});

        Ok(())
    }

    // Send a value change up through the callback
    #[instrument(level = "trace", target = "stor", skip(self, value), err)]
    async fn update_callback_value_change(
        &self,
        key: TypedKey,
        subkeys: ValueSubkeyRangeSet,
        count: u32,
        value: Option<ValueData>,
    ) -> Result<(), VeilidAPIError> {
        let opt_update_callback = {
            let inner = self.lock().await?;
            inner.update_callback.clone()
        };

        if let Some(update_callback) = opt_update_callback {
            update_callback(VeilidUpdate::ValueChange(Box::new(VeilidValueChange {
                key,
                subkeys,
                count,
                value,
            })));
        }
        Ok(())
    }

    #[instrument(level = "trace", target = "stor", skip_all)]
    fn check_fanout_set_offline(
        &self,
        key: TypedKey,
        subkey: ValueSubkey,
        fanout_result: &FanoutResult,
    ) -> bool {
        match fanout_result.kind {
            FanoutResultKind::Partial => false,
            FanoutResultKind::Timeout => {
                log_stor!(debug "timeout in set_value, adding offline subkey: {}:{}", key, subkey);
                true
            }
            FanoutResultKind::Exhausted => {
                let get_consensus =
                    self.unlocked_inner.config.get().network.dht.get_value_count as usize;
                let value_node_count = fanout_result.value_nodes.len();
                if value_node_count < get_consensus {
                    log_stor!(debug "exhausted with insufficient consensus ({}<{}), adding offline subkey: {}:{}", 
                        value_node_count, get_consensus,
                        key, subkey);
                    true
                } else {
                    false
                }
            }
            FanoutResultKind::Finished => false,
        }
    }
}

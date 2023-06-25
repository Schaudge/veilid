use super::*;

impl NetworkManager {
    /// Send raw data to a node
    ///
    /// We may not have dial info for a node, but have an existing connection for it
    /// because an inbound connection happened first, and no FindNodeQ has happened to that
    /// node yet to discover its dial info. The existing connection should be tried first
    /// in this case, if it matches the node ref's filters and no more permissive connection
    /// could be established.
    ///
    /// Sending to a node requires determining a NetworkClass compatible mechanism
    pub fn send_data(
        &self,
        target_node_ref: NodeRef,
        data: Vec<u8>,
    ) -> SendPinBoxFuture<EyreResult<NetworkResult<SendDataKind>>> {
        let this = self.clone();
        Box::pin(
            async move {
                // Get the best way to contact this node
                let contact_method = this.get_node_contact_method(target_node_ref.clone())?;

                // If we need to relay, do it
                let (contact_method, node_ref, relayed) = match contact_method {
                    NodeContactMethod::OutboundRelay(relay_nr)
                    | NodeContactMethod::InboundRelay(relay_nr) => {
                        let cm = this.get_node_contact_method(relay_nr.clone())?;
                        (cm, relay_nr, true)
                    }
                    cm => (cm, target_node_ref.clone(), false),
                };

                // Try the contact method
                let sdk = match contact_method {
                    NodeContactMethod::OutboundRelay(relay_nr)
                    | NodeContactMethod::InboundRelay(relay_nr) => {
                        // Relay loop or multiple relays
                        bail!(
                            "Relay loop or multiple relays detected: {} -> {} -> {}",
                            target_node_ref,
                            node_ref,
                            relay_nr
                        );
                    }
                    NodeContactMethod::Direct(dial_info) => {
                        network_result_try!(
                            this.send_data_ncm_direct(node_ref, dial_info, data).await?
                        )
                    }
                    NodeContactMethod::SignalReverse(relay_nr, target_node_ref) => {
                        network_result_try!(
                            this.send_data_ncm_signal_reverse(relay_nr, target_node_ref, data)
                                .await?
                        )
                    }
                    NodeContactMethod::SignalHolePunch(relay_nr, target_node_ref) => {
                        network_result_try!(
                            this.send_data_ncm_signal_hole_punch(relay_nr, target_node_ref, data)
                                .await?
                        )
                    }
                    NodeContactMethod::Existing => {
                        network_result_try!(
                            this.send_data_ncm_existing(target_node_ref, data).await?
                        )
                    }
                    NodeContactMethod::Unreachable => {
                        network_result_try!(
                            this.send_data_ncm_unreachable(target_node_ref, data)
                                .await?
                        )
                    }
                };

                if relayed {
                    return Ok(NetworkResult::value(SendDataKind::Indirect));
                }
                Ok(NetworkResult::value(sdk))
            }
            .instrument(trace_span!("send_data")),
        )
    }

    /// Send data using NodeContactMethod::Existing
    async fn send_data_ncm_existing(
        &self,
        target_node_ref: NodeRef,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<SendDataKind>> {
        // First try to send data to the last socket we've seen this peer on
        let Some(connection_descriptor) = target_node_ref.last_connection() else {
            return Ok(NetworkResult::no_connection_other(
                "should have found an existing connection",
            ));
        };

        if self
            .net()
            .send_data_to_existing_connection(connection_descriptor, data)
            .await?
            .is_some()
        {
            return Ok(NetworkResult::no_connection_other(
                "failed to send to existing connection",
            ));
        }

        // Update timestamp for this last connection since we just sent to it
        target_node_ref.set_last_connection(connection_descriptor, get_aligned_timestamp());

        Ok(NetworkResult::value(SendDataKind::Existing(
            connection_descriptor,
        )))
    }

    /// Send data using NodeContactMethod::Unreachable
    async fn send_data_ncm_unreachable(
        &self,
        target_node_ref: NodeRef,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<SendDataKind>> {
        // Try to send data to the last socket we've seen this peer on
        let Some(connection_descriptor) = target_node_ref.last_connection() else {
            return Ok(NetworkResult::no_connection_other(
                "Node is not reachable and has no existing connection",
            ));
        };

        if self
            .net()
            .send_data_to_existing_connection(connection_descriptor, data)
            .await?
            .is_some()
        {
            return Ok(NetworkResult::no_connection_other(
                "failed to send to existing connection",
            ));
        }

        // Update timestamp for this last connection since we just sent to it
        target_node_ref.set_last_connection(connection_descriptor, get_aligned_timestamp());

        Ok(NetworkResult::value(SendDataKind::Existing(
            connection_descriptor,
        )))
    }

    /// Send data using NodeContactMethod::SignalReverse
    async fn send_data_ncm_signal_reverse(
        &self,
        relay_nr: NodeRef,
        target_node_ref: NodeRef,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<SendDataKind>> {
        // First try to send data to the last socket we've seen this peer on
        let data = if let Some(connection_descriptor) = target_node_ref.last_connection() {
            match self
                .net()
                .send_data_to_existing_connection(connection_descriptor, data)
                .await?
            {
                None => {
                    // Update timestamp for this last connection since we just sent to it
                    target_node_ref
                        .set_last_connection(connection_descriptor, get_aligned_timestamp());

                    return Ok(NetworkResult::value(SendDataKind::Existing(
                        connection_descriptor,
                    )));
                }
                Some(data) => {
                    // Couldn't send data to existing connection
                    // so pass the data back out
                    data
                }
            }
        } else {
            // No last connection
            data
        };

        let connection_descriptor = network_result_try!(
            self.do_reverse_connect(relay_nr, target_node_ref, data)
                .await?
        );
        Ok(NetworkResult::value(SendDataKind::Direct(
            connection_descriptor,
        )))
    }

    /// Send data using NodeContactMethod::SignalHolePunch
    async fn send_data_ncm_signal_hole_punch(
        &self,
        relay_nr: NodeRef,
        target_node_ref: NodeRef,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<SendDataKind>> {
        // First try to send data to the last socket we've seen this peer on
        let data = if let Some(connection_descriptor) = target_node_ref.last_connection() {
            match self
                .net()
                .send_data_to_existing_connection(connection_descriptor, data)
                .await?
            {
                None => {
                    // Update timestamp for this last connection since we just sent to it
                    target_node_ref
                        .set_last_connection(connection_descriptor, get_aligned_timestamp());

                    return Ok(NetworkResult::value(SendDataKind::Existing(
                        connection_descriptor,
                    )));
                }
                Some(data) => {
                    // Couldn't send data to existing connection
                    // so pass the data back out
                    data
                }
            }
        } else {
            // No last connection
            data
        };

        let connection_descriptor =
            network_result_try!(self.do_hole_punch(relay_nr, target_node_ref, data).await?);
        Ok(NetworkResult::value(SendDataKind::Direct(
            connection_descriptor,
        )))
    }

    /// Send data using NodeContactMethod::Direct
    async fn send_data_ncm_direct(
        &self,
        node_ref: NodeRef,
        dial_info: DialInfo,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<SendDataKind>> {
        // Since we have the best dial info already, we can find a connection to use by protocol type
        let node_ref = node_ref.filtered_clone(NodeRefFilter::from(dial_info.make_filter()));

        // First try to send data to the last socket we've seen this peer on
        let data = if let Some(connection_descriptor) = node_ref.last_connection() {
            match self
                .net()
                .send_data_to_existing_connection(connection_descriptor, data)
                .await?
            {
                None => {
                    // Update timestamp for this last connection since we just sent to it
                    node_ref.set_last_connection(connection_descriptor, get_aligned_timestamp());

                    return Ok(NetworkResult::value(SendDataKind::Existing(
                        connection_descriptor,
                    )));
                }
                Some(d) => d,
            }
        } else {
            data
        };

        // New direct connection was necessary for this dial info
        let connection_descriptor =
            network_result_try!(self.net().send_data_to_dial_info(dial_info, data).await?);

        // If we connected to this node directly, save off the last connection so we can use it again
        node_ref.set_last_connection(connection_descriptor, get_aligned_timestamp());

        Ok(NetworkResult::value(SendDataKind::Direct(
            connection_descriptor,
        )))
    }

    /// Figure out how to reach a node from our own node over the best routing domain and reference the nodes we want to access
    /// Uses NodeRefs to ensure nodes are referenced, this is not a part of 'RoutingTable' because RoutingTable is not
    /// allowed to use NodeRefs due to recursive locking
    pub(crate) fn get_node_contact_method(
        &self,
        target_node_ref: NodeRef,
    ) -> EyreResult<NodeContactMethod> {
        let routing_table = self.routing_table();

        // Figure out the best routing domain to get the contact method over
        let routing_domain = match target_node_ref.best_routing_domain() {
            Some(rd) => rd,
            None => {
                log_net!("no routing domain for node {:?}", target_node_ref);
                return Ok(NodeContactMethod::Unreachable);
            }
        };

        // Get cache key
        let ncm_key = NodeContactMethodCacheKey {
            own_node_info_ts: routing_table.get_own_node_info_ts(routing_domain),
            target_node_info_ts: target_node_ref.node_info_ts(routing_domain),
            target_node_ref_filter: target_node_ref.filter_ref().cloned(),
        };
        if let Some(ncm) = self.inner.lock().node_contact_method_cache.get(&ncm_key) {
            return Ok(ncm.clone());
        }

        // Node A is our own node
        // Use whatever node info we've calculated so far
        let peer_a = routing_table.get_best_effort_own_peer_info(routing_domain);

        // Node B is the target node
        let peer_b = match target_node_ref.make_peer_info(routing_domain) {
            Some(ni) => ni,
            None => {
                log_net!("no node info for node {:?}", target_node_ref);
                return Ok(NodeContactMethod::Unreachable);
            }
        };

        // Dial info filter comes from the target node ref
        let dial_info_filter = target_node_ref.dial_info_filter();
        let sequencing = target_node_ref.sequencing();

        // If the node has had lost questions or failures to send, prefer sequencing
        // to improve reliability. The node may be experiencing UDP fragmentation drops
        // or other firewalling issues and may perform better with TCP.
        // let unreliable = target_node_ref.peer_stats().rpc_stats.failed_to_send > 2 || target_node_ref.peer_stats().rpc_stats.recent_lost_answers > 2;
        // if unreliable && sequencing < Sequencing::PreferOrdered {
        //     log_net!(debug "Node contact failing over to Ordered for {}", target_node_ref.to_string().cyan());
        //     sequencing = Sequencing::PreferOrdered;
        // }

        // Get the best contact method with these parameters from the routing domain
        let cm = routing_table.get_contact_method(
            routing_domain,
            &peer_a,
            &peer_b,
            dial_info_filter,
            sequencing,
        );

        // Translate the raw contact method to a referenced contact method
        let ncm = match cm {
            ContactMethod::Unreachable => NodeContactMethod::Unreachable,
            ContactMethod::Existing => NodeContactMethod::Existing,
            ContactMethod::Direct(di) => NodeContactMethod::Direct(di),
            ContactMethod::SignalReverse(relay_key, target_key) => {
                let mut relay_nr = routing_table
                    .lookup_and_filter_noderef(relay_key, routing_domain.into(), dial_info_filter)?
                    .ok_or_else(|| eyre!("couldn't look up relay"))?;
                if !target_node_ref.node_ids().contains(&target_key) {
                    bail!("signalreverse target noderef didn't match target key: {:?} != {} for relay {}", target_node_ref, target_key, relay_key );
                }
                relay_nr.set_sequencing(sequencing);
                NodeContactMethod::SignalReverse(relay_nr, target_node_ref)
            }
            ContactMethod::SignalHolePunch(relay_key, target_key) => {
                let mut relay_nr = routing_table
                    .lookup_and_filter_noderef(relay_key, routing_domain.into(), dial_info_filter)?
                    .ok_or_else(|| eyre!("couldn't look up relay"))?;
                if !target_node_ref.node_ids().contains(&target_key) {
                    bail!("signalholepunch target noderef didn't match target key: {:?} != {} for relay {}", target_node_ref, target_key, relay_key );
                }
                relay_nr.set_sequencing(sequencing);

                // if any other protocol were possible here we could update this and do_hole_punch
                // but tcp hole punch is very very unreliable it seems
                let udp_target_node_ref = target_node_ref
                    .filtered_clone(NodeRefFilter::new().with_protocol_type(ProtocolType::UDP));

                NodeContactMethod::SignalHolePunch(relay_nr, udp_target_node_ref)
            }
            ContactMethod::InboundRelay(relay_key) => {
                let mut relay_nr = routing_table
                    .lookup_and_filter_noderef(relay_key, routing_domain.into(), dial_info_filter)?
                    .ok_or_else(|| eyre!("couldn't look up relay"))?;
                relay_nr.set_sequencing(sequencing);
                NodeContactMethod::InboundRelay(relay_nr)
            }
            ContactMethod::OutboundRelay(relay_key) => {
                let mut relay_nr = routing_table
                    .lookup_and_filter_noderef(relay_key, routing_domain.into(), dial_info_filter)?
                    .ok_or_else(|| eyre!("couldn't look up relay"))?;
                relay_nr.set_sequencing(sequencing);
                NodeContactMethod::OutboundRelay(relay_nr)
            }
        };

        // Cache this
        self.inner
            .lock()
            .node_contact_method_cache
            .insert(ncm_key, ncm.clone(), |_, _| {});
        Ok(ncm)
    }

    /// Send a reverse connection signal and wait for the return receipt over it
    /// Then send the data across the new connection
    /// Only usable for PublicInternet routing domain
    #[cfg_attr(
        feature = "verbose-tracing",
        instrument(level = "trace", skip(self, data), err)
    )]
    async fn do_reverse_connect(
        &self,
        relay_nr: NodeRef,
        target_nr: NodeRef,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<ConnectionDescriptor>> {
        // Build a return receipt for the signal
        let receipt_timeout = ms_to_us(
            self.unlocked_inner
                .config
                .get()
                .network
                .reverse_connection_receipt_time_ms,
        );
        let (receipt, eventual_value) = self.generate_single_shot_receipt(receipt_timeout, [])?;

        // Get target routing domain
        let Some(routing_domain) = target_nr.best_routing_domain() else {
            return Ok(NetworkResult::no_connection_other("No routing domain for target"));
        };

        // Get our peer info
        let Some(peer_info) = self
            .routing_table()
            .get_own_peer_info(routing_domain) else {
            return Ok(NetworkResult::no_connection_other("Own peer info not available"));
        };

        // Issue the signal
        let rpc = self.rpc_processor();
        network_result_try!(rpc
            .rpc_call_signal(
                Destination::relay(relay_nr, target_nr.clone()),
                SignalInfo::ReverseConnect { receipt, peer_info },
            )
            .await
            .wrap_err("failed to send signal")?);

        // Wait for the return receipt
        let inbound_nr = match eventual_value.await.take_value().unwrap() {
            ReceiptEvent::ReturnedPrivate { private_route: _ }
            | ReceiptEvent::ReturnedOutOfBand
            | ReceiptEvent::ReturnedSafety => {
                return Ok(NetworkResult::invalid_message(
                    "reverse connect receipt should be returned in-band",
                ));
            }
            ReceiptEvent::ReturnedInBand { inbound_noderef } => inbound_noderef,
            ReceiptEvent::Expired => {
                return Ok(NetworkResult::timeout());
            }
            ReceiptEvent::Cancelled => {
                return Ok(NetworkResult::no_connection_other(format!(
                    "reverse connect receipt cancelled from {}",
                    target_nr
                )))
            }
        };

        // We expect the inbound noderef to be the same as the target noderef
        // if they aren't the same, we should error on this and figure out what then hell is up
        if !target_nr.same_entry(&inbound_nr) {
            bail!("unexpected noderef mismatch on reverse connect");
        }

        // And now use the existing connection to send over
        if let Some(descriptor) = inbound_nr.last_connection() {
            match self
                .net()
                .send_data_to_existing_connection(descriptor, data)
                .await?
            {
                None => Ok(NetworkResult::value(descriptor)),
                Some(_) => Ok(NetworkResult::no_connection_other(
                    "unable to send over reverse connection",
                )),
            }
        } else {
            bail!("no reverse connection available")
        }
    }

    /// Send a hole punch signal and do a negotiating ping and wait for the return receipt
    /// Then send the data across the new connection
    /// Only usable for PublicInternet routing domain
    #[cfg_attr(
        feature = "verbose-tracing",
        instrument(level = "trace", skip(self, data), err)
    )]
    async fn do_hole_punch(
        &self,
        relay_nr: NodeRef,
        target_nr: NodeRef,
        data: Vec<u8>,
    ) -> EyreResult<NetworkResult<ConnectionDescriptor>> {
        // Ensure we are filtered down to UDP (the only hole punch protocol supported today)
        assert!(target_nr
            .filter_ref()
            .map(|nrf| nrf.dial_info_filter.protocol_type_set
                == ProtocolTypeSet::only(ProtocolType::UDP))
            .unwrap_or_default());

        // Build a return receipt for the signal
        let receipt_timeout = ms_to_us(
            self.unlocked_inner
                .config
                .get()
                .network
                .hole_punch_receipt_time_ms,
        );
        let (receipt, eventual_value) = self.generate_single_shot_receipt(receipt_timeout, [])?;

        // Get target routing domain
        let Some(routing_domain) = target_nr.best_routing_domain() else {
            return Ok(NetworkResult::no_connection_other("No routing domain for target"));
        };

        // Get our peer info
        let Some(peer_info) = self
            .routing_table()
            .get_own_peer_info(routing_domain) else {
                return Ok(NetworkResult::no_connection_other("Own peer info not available"));
            };

        // Get the udp direct dialinfo for the hole punch
        let hole_punch_did = target_nr
            .first_filtered_dial_info_detail()
            .ok_or_else(|| eyre!("No hole punch capable dialinfo found for node"))?;

        // Do our half of the hole punch by sending an empty packet
        // Both sides will do this and then the receipt will get sent over the punched hole
        // Don't bother storing the returned connection descriptor as the 'last connection' because the other side of the hole
        // punch should come through and create a real 'last connection' for us if this succeeds
        network_result_try!(
            self.net()
                .send_data_to_dial_info(hole_punch_did.dial_info, Vec::new())
                .await?
        );

        // Issue the signal
        let rpc = self.rpc_processor();
        network_result_try!(rpc
            .rpc_call_signal(
                Destination::relay(relay_nr, target_nr.clone()),
                SignalInfo::HolePunch { receipt, peer_info },
            )
            .await
            .wrap_err("failed to send signal")?);

        // Wait for the return receipt
        let inbound_nr = match eventual_value.await.take_value().unwrap() {
            ReceiptEvent::ReturnedPrivate { private_route: _ }
            | ReceiptEvent::ReturnedOutOfBand
            | ReceiptEvent::ReturnedSafety => {
                return Ok(NetworkResult::invalid_message(
                    "hole punch receipt should be returned in-band",
                ));
            }
            ReceiptEvent::ReturnedInBand { inbound_noderef } => inbound_noderef,
            ReceiptEvent::Expired => {
                return Ok(NetworkResult::timeout());
            }
            ReceiptEvent::Cancelled => {
                return Ok(NetworkResult::no_connection_other(format!(
                    "hole punch receipt cancelled from {}",
                    target_nr
                )))
            }
        };

        // We expect the inbound noderef to be the same as the target noderef
        // if they aren't the same, we should error on this and figure out what then hell is up
        if !target_nr.same_entry(&inbound_nr) {
            bail!(
                "unexpected noderef mismatch on hole punch {}, expected {}",
                inbound_nr,
                target_nr
            );
        }

        // And now use the existing connection to send over
        if let Some(descriptor) = inbound_nr.last_connection() {
            match self
                .net()
                .send_data_to_existing_connection(descriptor, data)
                .await?
            {
                None => Ok(NetworkResult::value(descriptor)),
                Some(_) => Ok(NetworkResult::no_connection_other(
                    "unable to send over hole punch",
                )),
            }
        } else {
            bail!("no hole punch available")
        }
    }
}

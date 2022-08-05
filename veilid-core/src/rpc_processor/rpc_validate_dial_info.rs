use super::*;

impl RPCProcessor {
    // Can only be sent directly, not via relays or routes
    #[instrument(level = "trace", skip(self), ret, err)]
    pub async fn rpc_call_validate_dial_info(
        self,
        peer: NodeRef,
        dial_info: DialInfo,
        redirect: bool,
    ) -> Result<bool, RPCError> {
        let network_manager = self.network_manager();
        let receipt_time = ms_to_us(
            self.config
                .get()
                .network
                .dht
                .validate_dial_info_receipt_time_ms,
        );

        // Generate receipt and waitable eventual so we can see if we get the receipt back
        let (receipt, eventual_value) = network_manager
            .generate_single_shot_receipt(receipt_time, [])
            .map_err(RPCError::internal)?;

        let validate_dial_info = RPCOperationValidateDialInfo {
            dial_info,
            receipt,
            redirect,
        };
        let statement = RPCStatement::new(RPCStatementDetail::ValidateDialInfo(validate_dial_info));

        // Send the validate_dial_info request
        // This can only be sent directly, as relays can not validate dial info
        network_result_value_or_log!(debug self.statement(Destination::Direct(peer), statement, None)
            .await? => {
                return Ok(false);
            }
        );

        // Wait for receipt
        match eventual_value.await.take_value().unwrap() {
            ReceiptEvent::ReturnedInBand { inbound_noderef: _ } => {
                log_net!(debug "validate_dial_info receipt should be returned out-of-band".green());
                Ok(false)
            }
            ReceiptEvent::ReturnedOutOfBand => {
                log_net!(debug "validate_dial_info receipt returned");
                Ok(true)
            }
            ReceiptEvent::Expired => {
                log_net!(debug "validate_dial_info receipt expired".green());
                Ok(false)
            }
            ReceiptEvent::Cancelled => {
                Err(RPCError::internal("receipt was dropped before expiration"))
            }
        }
    }

    #[instrument(level = "trace", skip(self, msg), fields(msg.operation.op_id), err)]
    pub(crate) async fn process_validate_dial_info(&self, msg: RPCMessage) -> Result<(), RPCError> {
        // Get the statement
        let RPCOperationValidateDialInfo {
            dial_info,
            receipt,
            redirect,
        } = match msg.operation.into_kind() {
            RPCOperationKind::Statement(s) => match s.into_detail() {
                RPCStatementDetail::ValidateDialInfo(s) => s,
                _ => panic!("not a validate dial info"),
            },
            _ => panic!("not a statement"),
        };

        // Redirect this request if we are asked to
        if redirect {
            // Find peers capable of validating this dial info
            // We filter on the -outgoing- protocol capability status not the node's dial info
            // Use the address type though, to ensure we reach an ipv6 capable node if this is
            // an ipv6 address
            let routing_table = self.routing_table();
            let sender_id = msg.header.envelope.get_sender_id();
            let node_count = {
                let c = self.config.get();
                c.network.dht.max_find_node_count as usize
            };

            // Filter on nodes that can validate dial info, and can reach a specific dial info
            let outbound_dial_info_entry_filter =
                RoutingTable::make_outbound_dial_info_entry_filter(dial_info.clone());
            let will_validate_dial_info_filter = |e: &BucketEntryInner| {
                if let Some(status) = &e.peer_stats().status {
                    status.will_validate_dial_info
                } else {
                    true
                }
            };
            let filter = RoutingTable::combine_filters(
                outbound_dial_info_entry_filter,
                will_validate_dial_info_filter,
            );

            // Find nodes matching filter to redirect this to
            let peers = routing_table.find_fast_public_nodes_filtered(node_count, filter);
            if peers.is_empty() {
                return Err(RPCError::internal(format!(
                    "no peers able to reach dialinfo '{:?}'",
                    dial_info
                )));
            }
            for peer in peers {
                // Ensure the peer is not the one asking for the validation
                if peer.node_id() == sender_id {
                    continue;
                }

                // Make a copy of the request, without the redirect flag
                let validate_dial_info = RPCOperationValidateDialInfo {
                    dial_info: dial_info.clone(),
                    receipt: receipt.clone(),
                    redirect: false,
                };
                let statement =
                    RPCStatement::new(RPCStatementDetail::ValidateDialInfo(validate_dial_info));

                // Send the validate_dial_info request
                // This can only be sent directly, as relays can not validate dial info
                network_result_value_or_log!(debug self.statement(Destination::Direct(peer), statement, None)
                    .await? => {
                        return Ok(());
                    }
                );
            }
            return Ok(());
        };

        // Otherwise send a return receipt directly
        // Possibly from an alternate port
        let network_manager = self.network_manager();
        network_manager
            .send_out_of_band_receipt(dial_info.clone(), receipt)
            .await
            .map_err(RPCError::network)?;

        //        tracing::Span::current().record("res", &tracing::field::display(res));

        Ok(())
    }
}

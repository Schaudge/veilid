use super::*;

impl RPCProcessor {
    // Send StatusQ RPC request, receive StatusA answer
    // Can be sent via relays, but not via routes
    #[instrument(level = "trace", skip(self), ret, err)]
    pub async fn rpc_call_status(self, peer: NodeRef) -> Result<Answer<SenderInfo>, RPCError> {
        let node_status = self.network_manager().generate_node_status();
        let status_q = RPCOperationStatusQ { node_status };
        let respond_to = self.make_respond_to_sender(peer.clone());
        let question = RPCQuestion::new(respond_to, RPCQuestionDetail::StatusQ(status_q));

        // Send the info request
        let waitable_reply = self
            .question(Destination::Direct(peer.clone()), question, None)
            .await?;

        // Note what kind of ping this was and to what peer scope
        let send_data_kind = waitable_reply.send_data_kind;

        // Wait for reply
        let (msg, latency) = self.wait_for_reply(waitable_reply).await?;

        // Get the right answer type
        let status_a = match msg.operation.into_kind() {
            RPCOperationKind::Answer(a) => match a.into_detail() {
                RPCAnswerDetail::StatusA(a) => a,
                _ => return Err(RPCError::invalid_format("not a status answer")),
            },
            _ => return Err(RPCError::invalid_format("not an answer")),
        };

        // Update latest node status in routing table
        peer.operate_mut(|e| {
            e.update_node_status(status_a.node_status.clone());
        });

        // Report sender_info IP addresses to network manager
        if let Some(socket_address) = status_a.sender_info.socket_address {
            match send_data_kind {
                SendDataKind::LocalDirect => {
                    self.network_manager()
                        .report_local_socket_address(socket_address, peer)
                        .await;
                }
                SendDataKind::GlobalDirect => {
                    self.network_manager()
                        .report_global_socket_address(socket_address, peer)
                        .await;
                }
                SendDataKind::GlobalIndirect => {
                    // Do nothing in this case, as the socket address returned here would be for any node other than ours
                }
            }
        }

        Ok(Answer::new(latency, status_a.sender_info))
    }

    pub(crate) async fn process_status_q(&self, msg: RPCMessage) -> Result<(), RPCError> {
        let peer_noderef = msg.header.peer_noderef.clone();

        // Get the question
        let status_q = match msg.operation.kind() {
            RPCOperationKind::Question(q) => match q.detail() {
                RPCQuestionDetail::StatusQ(q) => q,
                _ => panic!("not a status question"),
            },
            _ => panic!("not a question"),
        };

        // update node status for the requesting node to our routing table
        if let Some(sender_nr) = msg.opt_sender_nr.clone() {
            // Update latest node status in routing table for the statusq sender
            sender_nr.operate_mut(|e| {
                e.update_node_status(status_q.node_status.clone());
            });
        }

        // Make status answer
        let node_status = self.network_manager().generate_node_status();
        let sender_info = Self::generate_sender_info(peer_noderef).await;
        let status_a = RPCOperationStatusA {
            node_status,
            sender_info,
        };

        // Send status answer
        self.answer(
            msg,
            RPCAnswer::new(RPCAnswerDetail::StatusA(status_a)),
            None,
        )
        .await
    }
}

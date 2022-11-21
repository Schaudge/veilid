use super::*;

impl RPCProcessor {
    #[instrument(level = "trace", skip(self, msg), fields(msg.operation.op_id, res), ret, err)]
    pub(crate) async fn process_cancel_tunnel_q(&self, msg: RPCMessage) -> Result<NetworkResult<()>, RPCError> {
        //        tracing::Span::current().record("res", &tracing::field::display(res));

        Err(RPCError::unimplemented("process_cancel_tunnel_q"))
    }
}

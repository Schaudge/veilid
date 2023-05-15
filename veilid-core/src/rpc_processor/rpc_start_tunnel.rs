use super::*;

impl RPCProcessor {
    #[instrument(level = "trace", skip(self, _msg), fields(_msg.operation.op_id), ret, err)]
    pub(crate) async fn process_start_tunnel_q(
        &self,
        _msg: RPCMessage,
    ) -> Result<NetworkResult<()>, RPCError> {
        Err(RPCError::unimplemented("process_start_tunnel_q"))
    }
}

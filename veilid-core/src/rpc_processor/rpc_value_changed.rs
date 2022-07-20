use super::*;

impl RPCProcessor {
    #[instrument(level = "trace", skip(self, msg), fields(msg.operation.op_id), err)]
    pub(crate) async fn process_value_changed(&self, msg: RPCMessage) -> Result<(), RPCError> {
        //        tracing::Span::current().record("res", &tracing::field::display(res));

        Err(RPCError::unimplemented("process_value_changed"))
    }
}

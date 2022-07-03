use crate::*;
use rpc_processor::*;

#[derive(Debug, Clone)]
pub struct RPCOperationSignal {
    signal_info: SignalInfo,
}

impl RPCOperationSignal {
    pub fn decode(
        reader: &veilid_capnp::operation_signal::Reader,
    ) -> Result<RPCOperationSignal, RPCError> {
        let signal_info = decode_signal_info(reader)?;
        Ok(RPCOperationSignal { signal_info })
    }
    pub fn encode(
        &self,
        builder: &mut veilid_capnp::operation_signal::Builder,
    ) -> Result<(), RPCError> {
        encode_signal_info(&self.signal_info, &mut builder)?;
        Ok(())
    }
}

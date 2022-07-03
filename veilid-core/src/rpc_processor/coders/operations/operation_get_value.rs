use crate::*;
use rpc_processor::*;

#[derive(Debug, Clone)]
pub struct RPCOperationGetValueQ {
    key: ValueKey,
}

impl RPCOperationGetValueQ {
    pub fn decode(
        reader: &veilid_capnp::operation_get_value_q::Reader,
    ) -> Result<RPCOperationGetValueQ, RPCError> {
        let k_reader = reader.get_key().map_err(map_error_capnp_error!())?;
        let key = decode_value_key(&k_reader)?;
        Ok(RPCOperationGetValueQ { key })
    }
    pub fn encode(
        &self,
        builder: &mut veilid_capnp::operation_get_value_q::Builder,
    ) -> Result<(), RPCError> {
        let k_builder = builder.init_key();
        encode_value_key(&self.key, &mut k_builder)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum RPCOperationGetValueA {
    Data(ValueData),
    Peers(Vec<PeerInfo>),
}

impl RPCOperationGetValueA {
    pub fn decode(
        reader: &veilid_capnp::operation_get_value_a::Reader,
    ) -> Result<RPCOperationGetValueA, RPCError> {
        match reader.which().map_err(map_error_capnp_notinschema!())? {
            veilid_capnp::operation_get_value_a::Which::Data(r) => {
                let data = decode_value_data(&r.map_err(map_error_capnp_error!())?)?;
                Ok(RPCOperationGetValueA::Data(data))
            }
            veilid_capnp::operation_get_value_a::Which::Peers(r) => {
                let peers_reader = r.map_err(map_error_capnp_error!())?;
                let mut peers = Vec::<PeerInfo>::with_capacity(
                    peers_reader
                        .len()
                        .try_into()
                        .map_err(map_error_internal!("too many peers"))?,
                );
                for p in peers_reader.iter() {
                    let peer_info = decode_peer_info(&p, true)?;
                    peers.push(peer_info);
                }

                Ok(RPCOperationGetValueA::Peers(peers))
            }
        }
    }
    pub fn encode(
        &self,
        builder: &mut veilid_capnp::operation_get_value_a::Builder,
    ) -> Result<(), RPCError> {
        match self {
            RPCOperationGetValueA::Data(data) => {
                let d_builder = builder.init_data();
                encode_value_data(&data, &mut d_builder)?;
            }
            RPCOperationGetValueA::Peers(peers) => {
                let mut peers_builder = builder.init_peers(
                    peers
                        .len()
                        .try_into()
                        .map_err(map_error_internal!("invalid peers list length"))?,
                );
                for (i, peer) in peers.iter().enumerate() {
                    let mut pi_builder = peers_builder.reborrow().get(i as u32);
                    encode_peer_info(peer, &mut pi_builder)?;
                }
            }
        }

        Ok(())
    }
}

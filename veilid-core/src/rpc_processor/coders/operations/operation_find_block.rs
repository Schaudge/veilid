use crate::*;
use rpc_processor::*;

#[derive(Debug, Clone)]
pub struct RPCOperationFindBlockQ {
    block_id: DHTKey,
}

impl RPCOperationFindBlockQ {
    pub fn decode(
        reader: &veilid_capnp::operation_find_block_q::Reader,
    ) -> Result<RPCOperationFindBlockQ, RPCError> {
        let bi_reader = reader.get_block_id().map_err(map_error_capnp_error!())?;
        let block_id = decode_block_id(&bi_reader);

        Ok(RPCOperationFindBlockQ { block_id })
    }
    pub fn encode(
        &self,
        builder: &mut veilid_capnp::operation_find_block_q::Builder,
    ) -> Result<(), RPCError> {
        let bi_builder = builder.init_block_id();
        encode_block_id(&self.block_id, &mut bi_builder)?;

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RPCOperationFindBlockA {
    data: Vec<u8>,
    suppliers: Vec<PeerInfo>,
    peers: Vec<PeerInfo>,
}

impl RPCOperationFindBlockA {
    pub fn decode(
        reader: &veilid_capnp::operation_find_block_a::Reader,
    ) -> Result<RPCOperationFindBlockA, RPCError> {
        let data = reader
            .get_data()
            .map_err(map_error_capnp_error!())?
            .to_vec();

        let suppliers_reader = reader.get_suppliers().map_err(map_error_capnp_error!())?;
        let mut suppliers = Vec::<PeerInfo>::with_capacity(
            suppliers_reader
                .len()
                .try_into()
                .map_err(map_error_internal!("too many suppliers"))?,
        );
        for s in suppliers_reader.iter() {
            let peer_info = decode_peer_info(&s, true)?;
            suppliers.push(peer_info);
        }

        let peers_reader = reader.get_peers().map_err(map_error_capnp_error!())?;
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

        Ok(RPCOperationFindBlockA {
            data,
            suppliers,
            peers,
        })
    }

    pub fn encode(
        &self,
        builder: &mut veilid_capnp::operation_find_block_a::Builder,
    ) -> Result<(), RPCError> {
        builder.set_data(&self.data);

        let mut suppliers_builder = builder.init_suppliers(
            self.suppliers
                .len()
                .try_into()
                .map_err(map_error_internal!("invalid suppliers list length"))?,
        );
        for (i, peer) in self.suppliers.iter().enumerate() {
            let mut pi_builder = suppliers_builder.reborrow().get(i as u32);
            encode_peer_info(peer, &mut pi_builder)?;
        }

        let mut peers_builder = builder.init_peers(
            self.peers
                .len()
                .try_into()
                .map_err(map_error_internal!("invalid peers list length"))?,
        );
        for (i, peer) in self.peers.iter().enumerate() {
            let mut pi_builder = peers_builder.reborrow().get(i as u32);
            encode_peer_info(peer, &mut pi_builder)?;
        }

        Ok(())
    }
}

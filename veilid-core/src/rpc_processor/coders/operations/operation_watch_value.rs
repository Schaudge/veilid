use super::*;

const MAX_WATCH_VALUE_Q_SUBKEYS_LEN: usize = 512;
const MAX_WATCH_VALUE_A_PEERS_LEN: usize = 20;

#[derive(Debug, Clone)]
pub(in crate::rpc_processor) struct RPCOperationWatchValueQ {
    key: TypedKey,
    subkeys: ValueSubkeyRangeSet,
    expiration: u64,
    count: u32,
    opt_watch_signature: Option<(PublicKey, Signature)>,
}

impl RPCOperationWatchValueQ {
    #[allow(dead_code)]
    pub fn new(
        key: TypedKey,
        subkeys: ValueSubkeyRangeSet,
        expiration: u64,
        count: u32,
        opt_watcher: Option<KeyPair>,
        vcrypto: CryptoSystemVersion,
    ) -> Result<Self, RPCError> {
        // Needed because RangeSetBlaze uses different types here all the time
        #[allow(clippy::unnecessary_cast)]
        let subkeys_len = subkeys.len() as usize;

        if subkeys_len > MAX_WATCH_VALUE_Q_SUBKEYS_LEN {
            return Err(RPCError::protocol("WatchValueQ subkeys length too long"));
        }

        let opt_watch_signature = if let Some(watcher) = opt_watcher {
            let signature_data = Self::make_signature_data(&key, &subkeys, expiration, count);
            let signature = vcrypto
                .sign(&watcher.key, &watcher.secret, &signature_data)
                .map_err(RPCError::protocol)?;
            Some((watcher.key, signature))
        } else {
            None
        };

        Ok(Self {
            key,
            subkeys,
            expiration,
            count,
            opt_watch_signature,
        })
    }

    // signature covers: key, subkeys, expiration, count, using watcher key
    fn make_signature_data(
        key: &TypedKey,
        subkeys: &ValueSubkeyRangeSet,
        expiration: u64,
        count: u32,
    ) -> Vec<u8> {
        // Needed because RangeSetBlaze uses different types here all the time
        #[allow(clippy::unnecessary_cast)]
        let subkeys_len = subkeys.len() as usize;

        let mut sig_data = Vec::with_capacity(PUBLIC_KEY_LENGTH + 4 + (subkeys_len * 8) + 8 + 4);
        sig_data.extend_from_slice(&key.kind.0);
        sig_data.extend_from_slice(&key.value.bytes);
        for sk in subkeys.ranges() {
            sig_data.extend_from_slice(&sk.start().to_le_bytes());
            sig_data.extend_from_slice(&sk.end().to_le_bytes());
        }
        sig_data.extend_from_slice(&expiration.to_le_bytes());
        sig_data.extend_from_slice(&count.to_le_bytes());
        sig_data
    }

    pub fn validate(&mut self, validate_context: &RPCValidateContext) -> Result<(), RPCError> {
        let Some(vcrypto) = validate_context.crypto.get(self.key.kind) else {
            return Err(RPCError::protocol("unsupported cryptosystem"));
        };

        if let Some(watch_signature) = self.opt_watch_signature {
            let sig_data =
                Self::make_signature_data(&self.key, &self.subkeys, self.expiration, self.count);
            vcrypto
                .verify(&watch_signature.0, &sig_data, &watch_signature.1)
                .map_err(RPCError::protocol)?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn key(&self) -> &TypedKey {
        &self.key
    }

    #[allow(dead_code)]
    pub fn subkeys(&self) -> &ValueSubkeyRangeSet {
        &self.subkeys
    }

    #[allow(dead_code)]
    pub fn expiration(&self) -> u64 {
        self.expiration
    }

    #[allow(dead_code)]
    pub fn count(&self) -> u32 {
        self.count
    }

    #[allow(dead_code)]
    pub fn opt_watch_signature(&self) -> Option<&(PublicKey, Signature)> {
        self.opt_watch_signature.as_ref()
    }

    #[allow(dead_code)]
    pub fn destructure(
        self,
    ) -> (
        TypedKey,
        ValueSubkeyRangeSet,
        u64,
        u32,
        Option<(PublicKey, Signature)>,
    ) {
        (
            self.key,
            self.subkeys,
            self.expiration,
            self.count,
            self.opt_watch_signature,
        )
    }

    pub fn decode(
        reader: &veilid_capnp::operation_watch_value_q::Reader,
    ) -> Result<Self, RPCError> {
        let k_reader = reader.get_key().map_err(RPCError::protocol)?;
        let key = decode_typed_key(&k_reader)?;

        let sk_reader = reader.get_subkeys().map_err(RPCError::protocol)?;
        if sk_reader.len() as usize > MAX_WATCH_VALUE_Q_SUBKEYS_LEN {
            return Err(RPCError::protocol("WatchValueQ subkeys length too long"));
        }
        let mut subkeys = ValueSubkeyRangeSet::new();
        for skr in sk_reader.iter() {
            let vskr = (skr.get_start(), skr.get_end());
            if vskr.0 > vskr.1 {
                return Err(RPCError::protocol("invalid subkey range"));
            }
            if let Some(lvskr) = subkeys.last() {
                if lvskr >= vskr.0 {
                    return Err(RPCError::protocol(
                        "subkey range out of order or not merged",
                    ));
                }
            }
            subkeys.ranges_insert(vskr.0..=vskr.1);
        }

        let expiration = reader.get_expiration();
        let count = reader.get_count();

        let opt_watch_signature = if reader.has_watcher() {
            let w_reader = reader.get_watcher().map_err(RPCError::protocol)?;
            let watcher = decode_key256(&w_reader);

            let s_reader = reader.get_signature().map_err(RPCError::protocol)?;
            let signature = decode_signature512(&s_reader);

            Some((watcher, signature))
        } else {
            None
        };

        Ok(Self {
            key,
            subkeys,
            expiration,
            count,
            opt_watch_signature,
        })
    }

    pub fn encode(
        &self,
        builder: &mut veilid_capnp::operation_watch_value_q::Builder,
    ) -> Result<(), RPCError> {
        let mut k_builder = builder.reborrow().init_key();
        encode_typed_key(&self.key, &mut k_builder);

        let mut sk_builder = builder.reborrow().init_subkeys(
            self.subkeys
                .len()
                .try_into()
                .map_err(RPCError::map_internal("invalid subkey range list length"))?,
        );
        for (i, skr) in self.subkeys.ranges().enumerate() {
            let mut skr_builder = sk_builder.reborrow().get(i as u32);
            skr_builder.set_start(*skr.start());
            skr_builder.set_end(*skr.end());
        }
        builder.set_expiration(self.expiration);
        builder.set_count(self.count);

        if let Some(watch_signature) = self.opt_watch_signature {
            let mut w_builder = builder.reborrow().init_watcher();
            encode_key256(&watch_signature.0, &mut w_builder);

            let mut s_builder = builder.reborrow().init_signature();
            encode_signature512(&watch_signature.1, &mut s_builder);
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(in crate::rpc_processor) struct RPCOperationWatchValueA {
    expiration: u64,
    peers: Vec<PeerInfo>,
}

impl RPCOperationWatchValueA {
    #[allow(dead_code)]
    pub fn new(expiration: u64, peers: Vec<PeerInfo>) -> Result<Self, RPCError> {
        if peers.len() > MAX_WATCH_VALUE_A_PEERS_LEN {
            return Err(RPCError::protocol("WatchValueA peers length too long"));
        }
        Ok(Self { expiration, peers })
    }

    pub fn validate(&mut self, validate_context: &RPCValidateContext) -> Result<(), RPCError> {
        PeerInfo::validate_vec(&mut self.peers, validate_context.crypto.clone());
        Ok(())
    }

    #[allow(dead_code)]
    pub fn expiration(&self) -> u64 {
        self.expiration
    }
    #[allow(dead_code)]
    pub fn peers(&self) -> &[PeerInfo] {
        &self.peers
    }
    #[allow(dead_code)]
    pub fn destructure(self) -> (u64, Vec<PeerInfo>) {
        (self.expiration, self.peers)
    }

    pub fn decode(
        reader: &veilid_capnp::operation_watch_value_a::Reader,
    ) -> Result<Self, RPCError> {
        let expiration = reader.get_expiration();
        let peers_reader = reader.get_peers().map_err(RPCError::protocol)?;
        if peers_reader.len() as usize > MAX_WATCH_VALUE_A_PEERS_LEN {
            return Err(RPCError::protocol("WatchValueA peers length too long"));
        }
        let mut peers = Vec::<PeerInfo>::with_capacity(
            peers_reader
                .len()
                .try_into()
                .map_err(RPCError::map_internal("too many peers"))?,
        );
        for p in peers_reader.iter() {
            let peer_info = decode_peer_info(&p)?;
            peers.push(peer_info);
        }

        Ok(Self { expiration, peers })
    }
    pub fn encode(
        &self,
        builder: &mut veilid_capnp::operation_watch_value_a::Builder,
    ) -> Result<(), RPCError> {
        builder.set_expiration(self.expiration);

        let mut peers_builder = builder.reborrow().init_peers(
            self.peers
                .len()
                .try_into()
                .map_err(RPCError::map_internal("invalid peers list length"))?,
        );
        for (i, peer) in self.peers.iter().enumerate() {
            let mut pi_builder = peers_builder.reborrow().get(i as u32);
            encode_peer_info(peer, &mut pi_builder)?;
        }

        Ok(())
    }
}

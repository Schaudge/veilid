use super::*;

#[derive(
    Clone,
    Default,
    PartialEq,
    Eq,
    Debug,
    Serialize,
    Deserialize,
    RkyvArchive,
    RkyvSerialize,
    RkyvDeserialize,
)]
#[archive_attr(repr(C), derive(CheckBytes))]
pub struct NodeInfo {
    network_class: NetworkClass,
    #[with(RkyvEnumSet)]
    outbound_protocols: ProtocolTypeSet,
    #[with(RkyvEnumSet)]
    address_types: AddressTypeSet,
    envelope_support: Vec<u8>,
    crypto_support: Vec<CryptoKind>,
    dial_info_detail_list: Vec<DialInfoDetail>,
}

impl NodeInfo {
    pub fn new(
        network_class: NetworkClass,
        outbound_protocols: ProtocolTypeSet,
        address_types: AddressTypeSet,
        envelope_support: Vec<u8>,
        crypto_support: Vec<CryptoKind>,
        dial_info_detail_list: Vec<DialInfoDetail>,
    ) -> Self {
        Self {
            network_class,
            outbound_protocols,
            address_types,
            envelope_support,
            crypto_support,
            dial_info_detail_list,
        }
    }

    pub fn network_class(&self) -> NetworkClass {
        self.network_class
    }
    pub fn outbound_protocols(&self) -> ProtocolTypeSet {
        self.outbound_protocols
    }
    pub fn address_types(&self) -> AddressTypeSet {
        self.address_types
    }
    pub fn envelope_support(&self) -> &[u8] {
        &self.envelope_support
    }
    pub fn crypto_support(&self) -> &[CryptoKind] {
        &self.crypto_support
    }
    pub fn dial_info_detail_list(&self) -> &[DialInfoDetail] {
        &self.dial_info_detail_list
    }

    pub fn first_filtered_dial_info_detail<S, F>(
        &self,
        sort: Option<S>,
        filter: F,
    ) -> Option<DialInfoDetail>
    where
        S: Fn(&DialInfoDetail, &DialInfoDetail) -> std::cmp::Ordering,
        F: Fn(&DialInfoDetail) -> bool,
    {
        if let Some(sort) = sort {
            let mut dids = self.dial_info_detail_list.clone();
            dids.sort_by(sort);
            for did in dids {
                if filter(&did) {
                    return Some(did);
                }
            }
        } else {
            for did in &self.dial_info_detail_list {
                if filter(did) {
                    return Some(did.clone());
                }
            }
        };
        None
    }

    pub fn all_filtered_dial_info_details<S, F>(
        &self,
        sort: Option<S>,
        filter: F,
    ) -> Vec<DialInfoDetail>
    where
        S: Fn(&DialInfoDetail, &DialInfoDetail) -> std::cmp::Ordering,
        F: Fn(&DialInfoDetail) -> bool,
    {
        let mut dial_info_detail_list = Vec::new();

        if let Some(sort) = sort {
            let mut dids = self.dial_info_detail_list.clone();
            dids.sort_by(sort);
            for did in dids {
                if filter(&did) {
                    dial_info_detail_list.push(did);
                }
            }
        } else {
            for did in &self.dial_info_detail_list {
                if filter(did) {
                    dial_info_detail_list.push(did.clone());
                }
            }
        };
        dial_info_detail_list
    }

    /// Does this node has some dial info
    pub fn has_dial_info(&self) -> bool {
        !self.dial_info_detail_list.is_empty()
    }

    /// Is some relay required either for signal or inbound relay or outbound relay?
    pub fn requires_relay(&self) -> bool {
        match self.network_class {
            NetworkClass::InboundCapable => {
                for did in &self.dial_info_detail_list {
                    if did.class.requires_relay() {
                        return true;
                    }
                }
            }
            NetworkClass::OutboundOnly => {
                return true;
            }
            NetworkClass::WebApp => {
                return true;
            }
            NetworkClass::Invalid => {}
        }
        false
    }

    /// Can this node assist with signalling? Yes but only if it doesn't require signalling, itself.
    pub fn can_signal(&self) -> bool {
        // Must be inbound capable
        if !matches!(self.network_class, NetworkClass::InboundCapable) {
            return false;
        }
        // Do any of our dial info require signalling? if so, we can't offer signalling
        for did in &self.dial_info_detail_list {
            if did.class.requires_signal() {
                return false;
            }
        }
        true
    }

    /// Can this node relay be an inbound relay?
    pub fn can_inbound_relay(&self) -> bool {
        // For now this is the same
        self.can_signal()
    }

    /// Is this node capable of validating dial info
    pub fn can_validate_dial_info(&self) -> bool {
        // For now this is the same
        self.can_signal()
    }
}

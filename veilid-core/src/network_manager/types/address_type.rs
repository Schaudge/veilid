use super::*;

#[allow(clippy::derived_hash_with_manual_eq)]
#[derive(
    Debug,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    RkyvArchive,
    RkyvSerialize,
    RkyvDeserialize,
    EnumSetType,
)]
#[enumset(repr = "u8")]
#[archive_attr(repr(u8), derive(CheckBytes))]
pub enum AddressType {
    IPV4,
    IPV6,
}
pub type AddressTypeSet = EnumSet<AddressType>;

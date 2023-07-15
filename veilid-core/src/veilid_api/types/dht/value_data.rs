use super::*;

#[derive(Clone, Default, PartialOrd, PartialEq, Eq, Ord, Serialize, Deserialize, JsonSchema)]
pub struct ValueData {
    /// An increasing sequence number to time-order the DHT record changes
    seq: ValueSeqNum,

    /// The contents of a DHT Record
    #[serde(with = "as_human_base64")]
    #[schemars(with = "String")]
    data: Vec<u8>,

    /// The public identity key of the writer of the data
    #[schemars(with = "String")]
    writer: PublicKey,
}
impl ValueData {
    pub const MAX_LEN: usize = 32768;

    pub fn new(data: Vec<u8>, writer: PublicKey) -> Self {
        assert!(data.len() <= Self::MAX_LEN);
        Self {
            seq: 0,
            data,
            writer,
        }
    }
    pub fn new_with_seq(seq: ValueSeqNum, data: Vec<u8>, writer: PublicKey) -> Self {
        assert!(data.len() <= Self::MAX_LEN);
        Self { seq, data, writer }
    }

    pub fn seq(&self) -> ValueSeqNum {
        self.seq
    }

    pub fn writer(&self) -> &PublicKey {
        &self.writer
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn total_size(&self) -> usize {
        mem::size_of::<Self>() + self.data.len()
    }
}

impl fmt::Debug for ValueData {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("ValueData")
            .field("seq", &self.seq)
            .field("data", &print_data(&self.data, None))
            .field("writer", &self.writer)
            .finish()
    }
}

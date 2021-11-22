use crate::xx::*;
use core::cmp::{Eq, Ord, Ordering, PartialEq, PartialOrd};
use core::convert::{TryFrom, TryInto};
use core::fmt;
use hex;

use crate::veilid_rng::*;
use ed25519_dalek::{Keypair, PublicKey, Signature};
use serde::{Deserialize, Serialize};

use data_encoding::BASE64URL_NOPAD;
use digest::generic_array::typenum::U64;
use digest::{Digest, Output};
use generic_array::GenericArray;

//////////////////////////////////////////////////////////////////////

#[allow(dead_code)]
pub const DHT_KEY_LENGTH: usize = 32;
#[allow(dead_code)]
pub const DHT_KEY_LENGTH_ENCODED: usize = 43;
#[allow(dead_code)]
pub const DHT_KEY_SECRET_LENGTH: usize = 32;
#[allow(dead_code)]
pub const DHT_KEY_SECRET_LENGTH_ENCODED: usize = 43;
#[allow(dead_code)]
pub const DHT_SIGNATURE_LENGTH: usize = 64;
#[allow(dead_code)]
pub const DHT_SIGNATURE_LENGTH_ENCODED: usize = 86;

//////////////////////////////////////////////////////////////////////

macro_rules! byte_array_type {
    ($name:ident, $size:expr) => {
        #[derive(Clone, Copy)]
        pub struct $name {
            pub bytes: [u8; $size],
            pub valid: bool,
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let s: String;
                if self.valid {
                    s = self.encode();
                } else {
                    s = "".to_owned();
                }
                s.serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                if s == "" {
                    return Ok($name::default());
                }
                $name::try_decode(s.as_str()).map_err(|e| serde::de::Error::custom(e))
            }
        }

        impl $name {
            pub fn new(bytes: [u8; $size]) -> Self {
                Self {
                    bytes: bytes,
                    valid: true,
                }
            }

            pub fn try_from_vec(v: Vec<u8>) -> Result<Self, String> {
                let mut this = Self {
                    bytes: [0u8; $size],
                    valid: true,
                };

                if v.len() != $size {
                    return Err(format!(
                        "Expected a Vec of length {} but it was {}",
                        $size,
                        v.len()
                    ));
                }

                for n in 0..v.len() {
                    this.bytes[n] = v[n];
                }

                Ok(this)
            }

            pub fn bit(&self, index: usize) -> bool {
                assert!(index < ($size * 8));
                let bi = index / 8;
                let ti = 7 - (index % 8);
                ((self.bytes[bi] >> ti) & 1) != 0
            }

            pub fn first_nonzero_bit(&self) -> Option<usize> {
                for i in 0..$size {
                    let b = self.bytes[i];
                    if b != 0 {
                        for n in 0..8 {
                            if ((b >> (7 - n)) & 1u8) != 0u8 {
                                return Some((i * 8) + n);
                            }
                        }
                        panic!("wtf")
                    }
                }
                None
            }

            pub fn nibble(&self, index: usize) -> u8 {
                assert!(index < ($size * 2));
                let bi = index / 2;
                if index & 1 == 0 {
                    (self.bytes[bi] >> 4) & 0xFu8
                } else {
                    self.bytes[bi] & 0xFu8
                }
            }

            pub fn first_nonzero_nibble(&self) -> Option<(usize, u8)> {
                for i in 0..($size * 2) {
                    let n = self.nibble(i);
                    if n != 0 {
                        return Some((i, n));
                    }
                }
                None
            }

            pub fn encode(&self) -> String {
                assert!(self.valid);
                BASE64URL_NOPAD.encode(&self.bytes)
            }

            pub fn try_decode(input: &str) -> Result<Self, String> {
                let mut bytes = [0u8; $size];

                let res = BASE64URL_NOPAD.decode_len(input.len());
                match res {
                    Ok(v) => {
                        if v != $size {
                            return Err("Incorrect length in decode".to_owned());
                        }
                    }
                    Err(_) => {
                        return Err("Failed to decode".to_owned());
                    }
                }

                let res = BASE64URL_NOPAD.decode_mut(input.as_bytes(), &mut bytes);
                match res {
                    Ok(_) => Ok(Self::new(bytes)),
                    Err(_) => Err("Failed to decode".to_owned()),
                }
            }
        }
        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &$name) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
        impl Ord for $name {
            fn cmp(&self, other: &$name) -> Ordering {
                if !self.valid && !other.valid {
                    return Ordering::Equal;
                }
                if !self.valid && other.valid {
                    return Ordering::Less;
                }
                if self.valid && !other.valid {
                    return Ordering::Greater;
                }

                for n in 0..$size {
                    if self.bytes[n] < other.bytes[n] {
                        return Ordering::Less;
                    }
                    if self.bytes[n] > other.bytes[n] {
                        return Ordering::Greater;
                    }
                }
                Ordering::Equal
            }
        }
        impl PartialEq<$name> for $name {
            fn eq(&self, other: &$name) -> bool {
                if self.valid != other.valid {
                    return false;
                }
                for n in 0..$size {
                    if self.bytes[n] != other.bytes[n] {
                        return false;
                    }
                }
                true
            }
        }
        impl Eq for $name {}
        impl Default for $name {
            fn default() -> Self {
                let mut this = $name::new([0u8; $size]);
                this.valid = false;
                this
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", String::from(self))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "("))?;
                write!(f, "{}", String::from(self))?;
                write!(f, ")")
            }
        }

        impl From<&$name> for String {
            fn from(value: &$name) -> Self {
                if !value.valid {
                    return "".to_owned();
                }
                let mut s = String::new();
                for n in 0..($size / 8) {
                    let b: [u8; 8] = value.bytes[n * 8..(n + 1) * 8].try_into().unwrap();
                    s.push_str(hex::encode(b).as_str());
                }
                s
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                $name::try_from(value.as_str())
            }
        }

        impl TryFrom<&str> for $name {
            type Error = String;
            fn try_from(value: &str) -> Result<Self, Self::Error> {
                let mut out = $name::default();
                if value == "" {
                    return Ok(out);
                }
                if value.len() != ($size * 2) {
                    return Err(concat!(stringify!($name), " is incorrect length").to_owned());
                }
                match hex::decode_to_slice(value, &mut out.bytes) {
                    Ok(_) => {
                        out.valid = true;
                        Ok(out)
                    }
                    Err(err) => Err(format!("{}", err)),
                }
            }
        }
    };
}

byte_array_type!(DHTKey, DHT_KEY_LENGTH);
byte_array_type!(DHTKeySecret, DHT_KEY_SECRET_LENGTH);
byte_array_type!(DHTSignature, DHT_SIGNATURE_LENGTH);
byte_array_type!(DHTKeyDistance, DHT_KEY_LENGTH);

/////////////////////////////////////////

struct Blake3Digest512 {
    dig: blake3::Hasher,
}

impl Digest for Blake3Digest512 {
    type OutputSize = U64;

    fn new() -> Self {
        Self {
            dig: blake3::Hasher::new(),
        }
    }

    fn update(&mut self, data: impl AsRef<[u8]>) {
        self.dig.update(data.as_ref());
    }

    fn chain(mut self, data: impl AsRef<[u8]>) -> Self
    where
        Self: Sized,
    {
        self.update(data);
        self
    }

    fn finalize(self) -> Output<Self> {
        let mut b = [0u8; 64];
        self.dig.finalize_xof().fill(&mut b);
        let mut out = GenericArray::<u8, U64>::default();
        for n in 0..64 {
            out[n] = b[n];
        }
        out
    }

    fn finalize_reset(&mut self) -> Output<Self> {
        let mut b = [0u8; 64];
        self.dig.finalize_xof().fill(&mut b);
        let mut out = GenericArray::<u8, U64>::default();
        for n in 0..64 {
            out[n] = b[n];
        }
        self.reset();
        out
    }

    fn reset(&mut self) {
        self.dig.reset();
    }

    fn output_size() -> usize {
        64
    }

    fn digest(data: &[u8]) -> Output<Self> {
        let mut dig = blake3::Hasher::new();
        dig.update(data);
        let mut b = [0u8; 64];
        dig.finalize_xof().fill(&mut b);
        let mut out = GenericArray::<u8, U64>::default();
        for n in 0..64 {
            out[n] = b[n];
        }
        out
    }
}

/////////////////////////////////////////

pub fn generate_secret() -> (DHTKey, DHTKeySecret) {
    let mut csprng = VeilidRng {};
    let keypair = Keypair::generate(&mut csprng);
    let dht_key = DHTKey::new(keypair.public.to_bytes());
    let dht_key_secret = DHTKeySecret::new(keypair.secret.to_bytes());

    (dht_key, dht_key_secret)
}

pub fn sign(
    dht_key: &DHTKey,
    dht_key_secret: &DHTKeySecret,
    data: &[u8],
) -> Result<DHTSignature, String> {
    assert!(dht_key.valid);
    assert!(dht_key_secret.valid);

    let mut kpb: [u8; DHT_KEY_SECRET_LENGTH + DHT_KEY_LENGTH] =
        [0u8; DHT_KEY_SECRET_LENGTH + DHT_KEY_LENGTH];

    kpb[..DHT_KEY_SECRET_LENGTH].copy_from_slice(&dht_key_secret.bytes);
    kpb[DHT_KEY_SECRET_LENGTH..].copy_from_slice(&dht_key.bytes);
    let keypair = Keypair::from_bytes(&kpb).map_err(|_| "Keypair is invalid".to_owned())?;

    let mut dig = Blake3Digest512::new();
    dig.update(data);

    let sig = keypair
        .sign_prehashed(dig, None)
        .map_err(|_| "Signature failed".to_owned())?;

    let dht_sig = DHTSignature::new(sig.to_bytes().clone());
    Ok(dht_sig)
}

pub fn verify(dht_key: &DHTKey, data: &[u8], signature: &DHTSignature) -> Result<(), String> {
    assert!(dht_key.valid);
    assert!(signature.valid);
    let pk =
        PublicKey::from_bytes(&dht_key.bytes).map_err(|_| "Public key is invalid".to_owned())?;
    let sig =
        Signature::from_bytes(&signature.bytes).map_err(|_| "Signature is invalid".to_owned())?;

    let mut dig = Blake3Digest512::new();
    dig.update(data);

    pk.verify_prehashed(dig, None, &sig)
        .map_err(|_| "Verification failed".to_owned())?;
    Ok(())
}

pub fn generate_hash(data: &[u8]) -> DHTKey {
    DHTKey::new(*blake3::hash(data).as_bytes())
}

pub fn validate_hash(data: &[u8], dht_key: &DHTKey) -> bool {
    assert!(dht_key.valid);
    let bytes = *blake3::hash(data).as_bytes();

    bytes == dht_key.bytes
}

pub fn validate_key(dht_key: &DHTKey, dht_key_secret: &DHTKeySecret) -> bool {
    let data = vec![0u8; 512];
    let sig = match sign(&dht_key, &dht_key_secret, &data) {
        Ok(s) => s,
        Err(_) => {
            return false;
        }
    };
    verify(&dht_key, &data, &sig).is_ok()
}

pub fn distance(key1: &DHTKey, key2: &DHTKey) -> DHTKeyDistance {
    assert!(key1.valid);
    assert!(key2.valid);
    let mut bytes = [0u8; DHT_KEY_LENGTH];

    for n in 0..DHT_KEY_LENGTH {
        bytes[n] = key1.bytes[n] ^ key2.bytes[n];
    }

    DHTKeyDistance::new(bytes)
}

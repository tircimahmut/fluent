use crate::{
    keccak256,
    revm_primitives::{Bytecode as RevmBytecode, Bytes},
    GenesisAccount, B256, KECCAK_EMPTY, U256,
};
use byteorder::{BigEndian, ReadBytesExt};
use bytes::Buf;
use reth_codecs::{main_codec, Compact};
use revm_primitives::JumpTable;
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use fluentbase_genesis::devnet::{KECCAK_HASH_KEY, POSEIDON_HASH_KEY};
use fluentbase_poseidon::poseidon_hash;
use revm_primitives::{POSEIDON_EMPTY};

/// An Ethereum account.
#[main_codec]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Account {
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the account's bytecode.
    pub bytecode_hash: Option<B256>,
    /// Hash of the rWASM bytecode
    pub rwasm_hash: Option<B256>,
}

impl Account {
    /// Whether the account has bytecode.
    pub fn has_bytecode(&self) -> bool {
        self.bytecode_hash.is_some() && self.rwasm_hash.is_some()
    }

    /// After SpuriousDragon empty account is defined as account with nonce == 0 && balance == 0 &&
    /// bytecode = None (or hash is [`KECCAK_EMPTY`]).
    pub fn is_empty(&self) -> bool {
        self.nonce == 0 &&
            self.balance.is_zero() &&
            self.bytecode_hash.map_or(true, |hash| hash == KECCAK_EMPTY) &&
            self.rwasm_hash.map_or(true, |hash| hash == POSEIDON_EMPTY)
    }

    /// Makes an [Account] from [GenesisAccount] type
    pub fn from_genesis_account(value: &GenesisAccount) -> Self {
        let bytecode_hash = value.storage
            .as_ref()
            .and_then(|s| s.get(&KECCAK_HASH_KEY))
            .cloned()
            .or_else(|| {
                value.code.as_ref().map(|bytes| keccak256(bytes.as_ref()))
            });
        let rwasm_hash = value.storage
            .as_ref()
            .and_then(|s| s.get(&POSEIDON_HASH_KEY))
            .cloned()
            .or_else(|| {
                value.code.as_ref().map(|bytes| B256::from(poseidon_hash(bytes.as_ref())))
            });
        Self {
            // nonce must exist, so we default to zero when converting a genesis account
            nonce: value.nonce.unwrap_or_default(),
            balance: value.balance,
            bytecode_hash: value.code.as_ref().map(keccak256),
            rwasm_hash,
        }
    }

    /// Returns an account bytecode's hash.
    /// In case of no bytecode, returns [`KECCAK_EMPTY`].
    pub fn get_bytecode_hash(&self) -> B256 {
        self.bytecode_hash.unwrap_or(KECCAK_EMPTY)
    }
}

/// Bytecode for an account.
///
/// A wrapper around [`revm::primitives::Bytecode`][RevmBytecode] with encoding/decoding support.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bytecode(pub RevmBytecode);

impl Bytecode {
    /// Create new bytecode from raw bytes.
    ///
    /// No analysis will be performed.
    pub fn new_raw(bytes: Bytes) -> Self {
        Self(RevmBytecode::new_raw(bytes))
    }
}

impl Deref for Bytecode {
    type Target = RevmBytecode;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Compact for Bytecode {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let bytecode = &self.0.bytecode()[..];
        buf.put_u32(bytecode.len() as u32);
        buf.put_slice(bytecode);
        let len = match &self.0 {
            RevmBytecode::LegacyRaw(_) => {
                buf.put_u8(0);
                1
            }
            // `1` has been removed.
            RevmBytecode::LegacyAnalyzed(analyzed) => {
                buf.put_u8(2);
                buf.put_u64(analyzed.original_len() as u64);
                let map = analyzed.jump_table().as_slice();
                buf.put_slice(map);
                1 + 8 + map.len()
            }
            RevmBytecode::Eof(_) => {
                // buf.put_u8(3);
                // TODO(EOF)
                todo!("EOF")
            }
        };
        len + bytecode.len() + 4
    }

    // # Panics
    //
    // A panic will be triggered if a bytecode variant of 1 or greater than 2 is passed from the
    // database.
    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let len = buf.read_u32::<BigEndian>().expect("could not read bytecode length");
        let bytes = Bytes::from(buf.copy_to_bytes(len as usize));
        let variant = buf.read_u8().expect("could not read bytecode variant");
        let decoded = match variant {
            0 => Self(RevmBytecode::new_raw(bytes)),
            1 => unreachable!("Junk data in database: checked Bytecode variant was removed"),
            2 => Self(unsafe {
                RevmBytecode::new_analyzed(
                    bytes,
                    buf.read_u64::<BigEndian>().unwrap() as usize,
                    JumpTable::from_slice(buf),
                )
            }),
            // TODO(EOF)
            3 => todo!("EOF"),
            _ => unreachable!("Junk data in database: unknown Bytecode variant"),
        };
        (decoded, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex_literal::hex;
    use revm_primitives::LegacyAnalyzedBytecode;

    #[test]
    fn test_account() {
        let mut buf = vec![];
        let mut acc = Account::default();
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 2);

        acc.balance = U256::from(2);
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 3);

        acc.nonce = 2;
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 4);
    }

    #[test]
    fn test_empty_account() {
        let mut acc = Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None, rwasm_hash: None };
        // Nonce 0, balance 0, and bytecode hash set to None is considered empty.
        assert!(acc.is_empty());

        acc.bytecode_hash = Some(KECCAK_EMPTY);
        // Nonce 0, balance 0, and bytecode hash set to KECCAK_EMPTY is considered empty.
        assert!(acc.is_empty());

        acc.balance = U256::from(2);
        // Non-zero balance makes it non-empty.
        assert!(!acc.is_empty());

        acc.balance = U256::ZERO;
        acc.nonce = 10;
        // Non-zero nonce makes it non-empty.
        assert!(!acc.is_empty());

        acc.nonce = 0;
        acc.bytecode_hash = Some(B256::from(U256::ZERO));
        // Non-empty bytecode hash makes it non-empty.
        assert!(!acc.is_empty());
    }

    #[test]
    fn test_bytecode() {
        let mut buf = vec![];
        let bytecode = Bytecode::new_raw(Bytes::default());
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 5);

        let mut buf = vec![];
        let bytecode = Bytecode::new_raw(Bytes::from(&hex!("ffff")));
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 7);

        let mut buf = vec![];
        let bytecode = Bytecode(RevmBytecode::LegacyAnalyzed(LegacyAnalyzedBytecode::new(
            Bytes::from(&hex!("ffff")),
            2,
            JumpTable::from_slice(&[0]),
        )));
        let len = bytecode.clone().to_compact(&mut buf);
        assert_eq!(len, 16);

        let (decoded, remainder) = Bytecode::from_compact(&buf, len);
        assert_eq!(decoded, bytecode);
        assert!(remainder.is_empty());
    }
}

//! A throwaway transparent key for one rehearsal. The node mines its block
//! rewards to this key, so projects can sign and send real transactions.

use anyhow::{Context, Result};
use ripemd::Ripemd160;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::{Digest, Sha256};

/// Base58Check prefixes for a P2PKH address and a secret key. Regtest uses
/// the testnet values (`zcash_protocol/src/constants/regtest.rs`).
const P2PKH_PREFIX: [u8; 2] = [0x1d, 0x25];
const SECRET_KEY_PREFIX: u8 = 0xef;

pub struct FundedKey {
    /// P2PKH `tm…` address.
    pub address: String,
    /// The secret key in WIF, for the compressed public key.
    pub wif: String,
}

impl FundedKey {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).context("reading OS randomness")?;
        let secret = SecretKey::from_slice(&bytes).context("random bytes are not a secret key")?;
        Ok(Self::from_secret(&secret))
    }

    fn from_secret(secret: &SecretKey) -> Self {
        let public = PublicKey::from_secret_key(&Secp256k1::signing_only(), secret).serialize();
        let hash = Ripemd160::digest(Sha256::digest(public));
        let address = bs58::encode([&P2PKH_PREFIX[..], &hash].concat())
            .with_check()
            .into_string();
        let wif =
            bs58::encode([&[SECRET_KEY_PREFIX][..], &secret.secret_bytes(), &[0x01]].concat())
                .with_check()
                .into_string();
        Self { address, wif }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_address_and_wif_for_secret_key_one() {
        let mut one = [0u8; 32];
        one[31] = 1;
        let key = FundedKey::from_secret(&SecretKey::from_slice(&one).unwrap());
        // HASH160 of the compressed generator point, a well-known value.
        let decoded = bs58::decode(&key.address)
            .with_check(None)
            .into_vec()
            .unwrap();
        assert_eq!(
            decoded,
            [
                &P2PKH_PREFIX[..],
                &hex("751e76e8199196d454941c45d1b3a323f1433bd6")
            ]
            .concat()
        );
        assert!(key.address.starts_with("tm"));
        assert_eq!(
            key.wif,
            "cMahea7zqjxrtgAbB7LSGbcQUr1uX1ojuat9jZodMN87JcbXMTcA"
        );
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}

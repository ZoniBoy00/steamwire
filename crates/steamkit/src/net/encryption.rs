//! Channel encryption used by the TCP and UDP transports.

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, BlockModeDecrypt, BlockModeEncrypt, InnerIvInit, KeyInit};
use hmac::{Hmac, Mac};
use rand::Rng;
use rsa::pkcs8::DecodePublicKey;
use sha1::Sha1;

use crate::enums::EUniverse;
use crate::error::{Error, Result};

type HmacSha1 = Hmac<Sha1>;
type Block = aes::cipher::Block<aes::Aes256>;

/// Steam's public key for the public universe (DER `SubjectPublicKeyInfo`),
/// from SteamKit2's `KeyDictionary`.
const PUBLIC_UNIVERSE_KEY: [u8; 160] = [
    0x30, 0x81, 0x9D, 0x30, 0x0D, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01, 0x05, 0x00, 0x03,
    0x81, 0x8B, 0x00, 0x30, 0x81, 0x87, 0x02, 0x81, 0x81, 0x00, 0xDF, 0xEC, 0x1A, 0xD6, 0x2C, 0x10, 0x66, 0x2C, 0x17,
    0x35, 0x3A, 0x14, 0xB0, 0x7C, 0x59, 0x11, 0x7F, 0x9D, 0xD3, 0xD8, 0x2B, 0x7A, 0xE3, 0xE0, 0x15, 0xCD, 0x19, 0x1E,
    0x46, 0xE8, 0x7B, 0x87, 0x74, 0xA2, 0x18, 0x46, 0x31, 0xA9, 0x03, 0x14, 0x79, 0x82, 0x8E, 0xE9, 0x45, 0xA2, 0x49,
    0x12, 0xA9, 0x23, 0x68, 0x73, 0x89, 0xCF, 0x69, 0xA1, 0xB1, 0x61, 0x46, 0xBD, 0xC1, 0xBE, 0xBF, 0xD6, 0x01, 0x1B,
    0xD8, 0x81, 0xD4, 0xDC, 0x90, 0xFB, 0xFE, 0x4F, 0x52, 0x73, 0x66, 0xCB, 0x95, 0x70, 0xD7, 0xC5, 0x8E, 0xBA, 0x1C,
    0x7A, 0x33, 0x75, 0xA1, 0x62, 0x34, 0x46, 0xBB, 0x60, 0xB7, 0x80, 0x68, 0xFA, 0x13, 0xA7, 0x7A, 0x8A, 0x37, 0x4B,
    0x9E, 0xC6, 0xF4, 0x5D, 0x5F, 0x3A, 0x99, 0xF9, 0x9E, 0xC4, 0x3A, 0xE9, 0x63, 0xA2, 0xBB, 0x88, 0x19, 0x28, 0xE0,
    0xE7, 0x14, 0xC0, 0x42, 0x89, 0x02, 0x01, 0x11,
];

/// The universe's public key, used in the TCP encryption handshake.
pub fn universe_public_key(universe: EUniverse) -> Option<&'static [u8]> {
    match universe {
        EUniverse::Public => Some(&PUBLIC_UNIVERSE_KEY),
        _ => None,
    }
}

/// RSA-OAEP (SHA-1) with a DER public key.
pub fn rsa_encrypt_oaep_sha1(public_key_der: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let key = rsa::PublicKey::from_public_key_der(public_key_der)
        .map_err(|e| Error::Crypto(format!("invalid RSA public key: {e}")))?;
    key.oaep_encrypt::<Sha1>(data).map_err(|e| Error::Crypto(format!("RSA encryption failed: {e}")))
}

/// AES-256-CBC with an HMAC-SHA1 derived IV: Steam's channel encryption.
///
/// A packet is `AES-ECB(iv) || AES-CBC(iv, plaintext)`, where
/// `iv = HMAC-SHA1(secret, random3 || plaintext)[..13] || random3` and the
/// secret is the first 16 bytes of the session key.
#[derive(Clone)]
pub struct ChannelCipher {
    cipher: aes::Aes256,
    mac: HmacSha1,
}

impl std::fmt::Debug for ChannelCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelCipher { .. }")
    }
}

const IV_HASH_LEN: usize = 13;

impl ChannelCipher {
    /// Creates the cipher from a 32-byte session key.
    pub fn new(session_key: &[u8; 32]) -> Self {
        let cipher = aes::Aes256::new_from_slice(session_key).expect("32-byte key");
        let mac = <HmacSha1 as KeyInit>::new_from_slice(&session_key[..16]).expect("any key length");
        Self { cipher, mac }
    }

    fn iv_mac(&self, iv: &[u8; 16], plaintext: &[u8]) -> HmacSha1 {
        let mut mac = self.mac.clone();
        mac.update(&iv[IV_HASH_LEN..]);
        mac.update(plaintext);
        mac
    }

    /// Encrypts one packet.
    pub fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        let mut iv = [0u8; 16];
        rand::rng().fill_bytes(&mut iv[IV_HASH_LEN..]);
        let tag = self.iv_mac(&iv, plaintext).finalize().into_bytes();
        iv[..IV_HASH_LEN].copy_from_slice(&tag[..IV_HASH_LEN]);

        let mut out = vec![0u8; 16 + (plaintext.len() / 16 + 1) * 16];
        let mut block = Block::default();
        block.copy_from_slice(&iv);
        self.cipher.encrypt_block(&mut block);
        out[..16].copy_from_slice(&block);

        let iv_block = Block::from(iv);
        let len = cbc::Encryptor::<aes::Aes256>::inner_iv_init(self.cipher.clone(), &iv_block)
            .encrypt_padded_b2b::<Pkcs7>(plaintext, &mut out[16..])
            .expect("output sized for padding")
            .len();
        out.truncate(16 + len);
        out
    }

    /// Decrypts and authenticates one packet.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 32 || !data.len().is_multiple_of(16) {
            return Err(Error::Crypto("bad encrypted packet length".into()));
        }
        let mut block = Block::default();
        block.copy_from_slice(&data[..16]);
        self.cipher.decrypt_block(&mut block);
        let iv: [u8; 16] = block.into();

        let mut out = vec![0u8; data.len() - 16];
        let len = cbc::Decryptor::<aes::Aes256>::inner_iv_init(self.cipher.clone(), &block)
            .decrypt_padded_b2b::<Pkcs7>(&data[16..], &mut out)
            .map_err(|_| Error::Crypto("bad padding".into()))?
            .len();
        out.truncate(len);

        self.iv_mac(&iv, &out)
            .verify_truncated_left(&iv[..IV_HASH_LEN])
            .map_err(|_| Error::Crypto("HMAC mismatch".into()))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::crypto::random_bytes;

    #[test]
    fn channel_round_trip() {
        let cipher = ChannelCipher::new(&random_bytes());
        for len in [0usize, 1, 15, 16, 17, 1000] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let enc = cipher.encrypt(&data);
            assert_eq!(enc.len() % 16, 0);
            assert_eq!(cipher.decrypt(&enc).unwrap(), data);
        }
    }

    #[test]
    fn channel_rejects_tampering() {
        let cipher = ChannelCipher::new(&random_bytes());
        let mut enc = cipher.encrypt(b"hello steam");
        let last = enc.len() - 20;
        enc[last] ^= 1;
        assert!(cipher.decrypt(&enc).is_err());
    }

    #[test]
    fn universe_key_parses() {
        let blob = rsa_encrypt_oaep_sha1(universe_public_key(EUniverse::Public).unwrap(), &[7; 48]).unwrap();
        assert_eq!(blob.len(), 128);
    }
}

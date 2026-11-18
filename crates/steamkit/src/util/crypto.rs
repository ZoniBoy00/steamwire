//! Crypto helpers shared by the handlers, the CDN code and the transports.

use rand::Rng;

use crate::error::{Error, Result};

/// RSA PKCS#1 v1.5 with a hex modulus and exponent, as returned by
/// `Authentication.GetPasswordRSAPublicKey`.
pub fn rsa_encrypt_pkcs1_hex(modulus_hex: &str, exponent_hex: &str, data: &[u8]) -> Result<Vec<u8>> {
    let modulus = hex::decode(modulus_hex).map_err(|e| Error::Crypto(e.to_string()))?;
    let exponent = hex::decode(exponent_hex).map_err(|e| Error::Crypto(e.to_string()))?;
    let key = rsa::PublicKey::new(
        rsa::BoxedUint::from_be_slice_vartime(&modulus),
        rsa::BoxedUint::from_be_slice_vartime(&exponent),
    )
    .map_err(|e| Error::Crypto(format!("invalid RSA public key: {e}")))?;
    key.pkcs1v15_encrypt(data).map_err(|e| Error::Crypto(format!("RSA encryption failed: {e}")))
}

/// Fills a buffer with random bytes.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::rng().fill_bytes(&mut buf);
    buf
}

/// Decrypts data in Steam's symmetric format: the first 16 bytes are the IV,
/// encrypted with AES-256-ECB; the rest is AES-256-CBC with PKCS#7 padding
/// (SteamKit2's `CryptoHelper.SymmetricDecrypt`).
pub fn symmetric_decrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    use aes::cipher::block_padding::Pkcs7;
    use aes::cipher::{BlockCipherDecrypt, BlockModeDecrypt, InnerIvInit, KeyInit};

    if key.len() != 32 {
        return Err(Error::Crypto(format!("expected a 32 byte key, got {}", key.len())));
    }
    if data.len() < 32 || !data.len().is_multiple_of(16) {
        return Err(Error::Crypto(format!("bad encrypted length {}", data.len())));
    }
    let cipher = aes::Aes256::new_from_slice(key).map_err(|e| Error::Crypto(e.to_string()))?;
    let mut iv = aes::Block::default();
    iv.copy_from_slice(&data[..16]);
    cipher.decrypt_block(&mut iv);

    let mut out = vec![0u8; data.len() - 16];
    let len = cbc::Decryptor::<aes::Aes256>::inner_iv_init(cipher, &iv)
        .decrypt_padded_b2b::<Pkcs7>(&data[16..], &mut out)
        .map_err(|_| Error::Crypto("bad padding".into()))?
        .len();
    out.truncate(len);
    Ok(out)
}

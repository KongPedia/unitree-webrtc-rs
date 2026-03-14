use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes256;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use md5::{Digest, Md5};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::rand_core::OsRng;
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use uuid::Uuid;

const AES_BLOCK_SIZE: usize = 16;
const CON_NOTIFY_KEY: [u8; 16] = [
    232, 86, 130, 189, 22, 84, 155, 0, 142, 4, 166, 104, 43, 179, 235, 227,
];

pub fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn hex_to_base64(hex_str: &str) -> Result<String, String> {
    let bytes = hex::decode(hex_str).map_err(|error| error.to_string())?;
    Ok(STANDARD.encode(bytes))
}

pub fn encrypt_key(key: &str) -> Result<String, String> {
    let prefixed_key = format!("UnitreeGo2_{key}");
    hex_to_base64(&md5_hex(&prefixed_key))
}

pub fn generate_aes_key() -> String {
    Uuid::new_v4().simple().to_string()
}

pub fn aes_ecb_encrypt(plain_text: &str, key: &str) -> Result<String, String> {
    let key_bytes = validate_aes256_key(key)?;
    let cipher = Aes256::new_from_slice(key_bytes).map_err(|error| error.to_string())?;

    let mut padded = pkcs7_pad(plain_text.as_bytes());
    for chunk in padded.chunks_exact_mut(AES_BLOCK_SIZE) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.encrypt_block(block);
    }

    Ok(STANDARD.encode(padded))
}

pub fn aes_ecb_decrypt(encoded_cipher_text: &str, key: &str) -> Result<String, String> {
    let key_bytes = validate_aes256_key(key)?;
    let cipher = Aes256::new_from_slice(key_bytes).map_err(|error| error.to_string())?;

    let mut encrypted_bytes = STANDARD
        .decode(encoded_cipher_text)
        .map_err(|error| error.to_string())?;

    if encrypted_bytes.len() % AES_BLOCK_SIZE != 0 {
        return Err("AES ECB payload length must be multiple of 16".to_string());
    }

    for chunk in encrypted_bytes.chunks_exact_mut(AES_BLOCK_SIZE) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.decrypt_block(block);
    }

    let plain_bytes = pkcs7_unpad(&encrypted_bytes)?;
    String::from_utf8(plain_bytes.to_vec()).map_err(|error| error.to_string())
}

pub fn aes_gcm_decrypt(encoded_cipher_text: &str) -> Result<String, String> {
    let payload = STANDARD
        .decode(encoded_cipher_text)
        .map_err(|error| error.to_string())?;

    if payload.len() < 28 {
        return Err("Decryption failed: input data too short".to_string());
    }

    let ciphertext = &payload[..payload.len() - 28];
    let nonce = &payload[payload.len() - 28..payload.len() - 16];
    let tag = &payload[payload.len() - 16..];

    let mut ciphertext_with_tag = Vec::with_capacity(ciphertext.len() + tag.len());
    ciphertext_with_tag.extend_from_slice(ciphertext);
    ciphertext_with_tag.extend_from_slice(tag);

    let cipher = Aes128Gcm::new_from_slice(&CON_NOTIFY_KEY).map_err(|error| error.to_string())?;
    let plain_bytes = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext_with_tag.as_ref())
        .map_err(|error| error.to_string())?;

    String::from_utf8(plain_bytes).map_err(|error| error.to_string())
}

pub fn decrypt_con_notify_data(encoded_cipher_text: &str) -> Result<String, String> {
    aes_gcm_decrypt(encoded_cipher_text)
}

pub fn rsa_load_public_key(encoded_public_key: &str) -> Result<RsaPublicKey, String> {
    let key_bytes = STANDARD
        .decode(encoded_public_key)
        .map_err(|error| error.to_string())?;

    RsaPublicKey::from_public_key_der(&key_bytes)
        .or_else(|_| RsaPublicKey::from_pkcs1_der(&key_bytes))
        .map_err(|error| error.to_string())
}

pub fn rsa_encrypt(data: &str, public_key: &RsaPublicKey) -> Result<String, String> {
    let max_chunk_size = public_key.size().saturating_sub(11);
    if max_chunk_size == 0 {
        return Err("RSA public key is too small".to_string());
    }

    let mut encrypted = Vec::new();
    let mut rng = OsRng;

    for chunk in data.as_bytes().chunks(max_chunk_size) {
        let block = public_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, chunk)
            .map_err(|error| error.to_string())?;
        encrypted.extend_from_slice(&block);
    }

    Ok(STANDARD.encode(encrypted))
}

fn validate_aes256_key(key: &str) -> Result<&[u8], String> {
    let key_bytes = key.as_bytes();
    if key_bytes.len() != 32 {
        return Err(format!(
            "AES-256 key must be 32 bytes, got {}",
            key_bytes.len()
        ));
    }

    Ok(key_bytes)
}

fn pkcs7_pad(data: &[u8]) -> Vec<u8> {
    let padding = AES_BLOCK_SIZE - (data.len() % AES_BLOCK_SIZE);
    let mut output = Vec::with_capacity(data.len() + padding);
    output.extend_from_slice(data);
    output.extend(std::iter::repeat_n(padding as u8, padding));
    output
}

fn pkcs7_unpad(data: &[u8]) -> Result<&[u8], String> {
    let Some(&last) = data.last() else {
        return Err("Encrypted payload is empty".to_string());
    };

    let padding = last as usize;
    if padding == 0 || padding > AES_BLOCK_SIZE || padding > data.len() {
        return Err("Invalid PKCS7 padding length".to_string());
    }

    let padding_bytes = &data[data.len() - padding..];
    if padding_bytes.iter().any(|byte| *byte as usize != padding) {
        return Err("Invalid PKCS7 padding bytes".to_string());
    }

    Ok(&data[..data.len() - padding])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_hex_is_stable() {
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn hex_to_base64_works() {
        assert_eq!(hex_to_base64("48656c6c6f").unwrap(), "SGVsbG8=");
    }

    #[test]
    fn encrypt_key_matches_md5_flow() {
        let expected = hex_to_base64(&md5_hex("UnitreeGo2_test-key")).unwrap();
        assert_eq!(encrypt_key("test-key").unwrap(), expected);
    }

    #[test]
    fn aes_ecb_round_trip() {
        let key = "0123456789abcdef0123456789abcdef";
        let plain = "hello world";

        let encrypted = aes_ecb_encrypt(plain, key).unwrap();
        let decrypted = aes_ecb_decrypt(&encrypted, key).unwrap();

        assert_eq!(decrypted, plain);
    }

    #[test]
    fn aes_gcm_con_notify_round_trip_layout() {
        let cipher = Aes128Gcm::new_from_slice(&CON_NOTIFY_KEY).unwrap();
        let nonce = [7_u8; 12];
        let plain = b"notify-payload";

        let ciphertext_with_tag = cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_ref())
            .unwrap();

        let split_index = ciphertext_with_tag.len() - 16;
        let ciphertext = &ciphertext_with_tag[..split_index];
        let tag = &ciphertext_with_tag[split_index..];

        let mut packed = Vec::new();
        packed.extend_from_slice(ciphertext);
        packed.extend_from_slice(&nonce);
        packed.extend_from_slice(tag);

        let encoded = STANDARD.encode(packed);
        assert_eq!(aes_gcm_decrypt(&encoded).unwrap(), "notify-payload");
    }

    #[test]
    fn generated_aes_key_has_expected_length() {
        let key = generate_aes_key();
        assert_eq!(key.len(), 32);
    }
}

use std::fs;

use aes_gcm::{
    aead::{Aead, AeadCore, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use anyhow::{bail, Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use rand::{rngs::OsRng, RngCore};
use rsa::{
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    RsaPrivateKey, RsaPublicKey,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;

use crate::config::Config;

pub const B64: base64::engine::general_purpose::GeneralPurpose = STANDARD;

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub iv: String,
    pub ct: String,
}

pub struct Crypto {
    pub rsa: RsaPrivateKey,
    pub public_pem: String,
}

impl Crypto {
    pub fn init(cfg: &Config) -> Result<Self> {
        fs::create_dir_all(&cfg.state_dir).context("创建状态目录失败")?;
        let key_path = cfg.state_dir.join("rsa_private.pem");

        let rsa = if key_path.exists() {
            let pem = fs::read_to_string(&key_path).context("读取 RSA 私钥失败")?;
            RsaPrivateKey::from_pkcs8_pem(&pem).context("解析 RSA 私钥失败")?
        } else {
            let mut rng = OsRng;
            let key = RsaPrivateKey::new(&mut rng, cfg.rsa_bits).context("生成 RSA 私钥失败")?;
            let pem = key
                .to_pkcs8_pem(LineEnding::LF)
                .context("编码 RSA 私钥失败")?
                .to_string();
            fs::write(&key_path, &pem).context("保存 RSA 私钥失败")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))
                    .context("设置 RSA 私钥权限失败")?;
            }
            key
        };

        let public_pem = RsaPublicKey::from(&rsa)
            .to_public_key_pem(LineEnding::LF)
            .context("编码 RSA 公钥失败")?;

        Ok(Self { rsa, public_pem })
    }
}

pub fn hash_password(password: &str) -> Result<String> {
    let mut salt_bytes = [0u8; 16];
    OsRng.fill_bytes(&mut salt_bytes);
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|err| anyhow::anyhow!("生成盐失败: {err}"))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|err| anyhow::anyhow!("计算密码哈希失败: {err}"))?;
    Ok(hash.to_string())
}

pub fn verify_password_hash(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn random_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

pub fn random_b64(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn encrypt_envelope<T: Serialize>(
    key: &[u8; 32],
    aad: &str,
    value: &T,
) -> Result<serde_json::Value> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("无效的会话密钥"))?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let plaintext = serde_json::to_vec(value)?;
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow::anyhow!("加密失败"))?;
    Ok(json!({
        "iv": B64.encode(nonce.as_slice()),
        "ct": B64.encode(ciphertext),
    }))
}

/// Extracts the 12-byte AES-GCM nonce from an envelope. Exposed so the API
/// layer can register the nonce in the session replay window before decoding.
pub fn envelope_nonce(envelope: &Envelope) -> Result<[u8; 12]> {
    let iv = B64
        .decode(envelope.iv.as_bytes())
        .context("iv 不是合法 base64")?;
    iv.try_into()
        .map_err(|_| anyhow::anyhow!("加密负载格式错误"))
}

pub fn decrypt_envelope<T: DeserializeOwned>(
    key: &[u8; 32],
    aad: &str,
    envelope: &Envelope,
) -> Result<T> {
    let iv = envelope_nonce(envelope)?;
    let ciphertext = B64
        .decode(envelope.ct.as_bytes())
        .context("ct 不是合法 base64")?;
    if ciphertext.len() < 16 {
        bail!("加密负载格式错误");
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("无效的会话密钥"))?;
    let nonce = Nonce::from_slice(&iv);
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow::anyhow!("解密失败"))?;
    Ok(serde_json::from_slice(&plaintext)?)
}

/// Encrypts one binary stream chunk into the wire frame shared by downloads
/// and previews: `[u32 ciphertext length][12-byte nonce][ciphertext + tag]`.
pub fn seal_frame(cipher: &Aes256Gcm, aad: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow::anyhow!("流分块加密失败"))?;
    let mut frame = Vec::with_capacity(4 + 12 + ciphertext.len());
    frame.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
    frame.extend_from_slice(nonce.as_slice());
    frame.extend_from_slice(&ciphertext);
    Ok(frame)
}

/// The AAD binds every encrypted API payload to the exact HTTP method and URI,
/// so a ciphertext cannot be replayed against a different endpoint.
pub fn request_aad(method: &axum::http::Method, uri: &axum::http::Uri) -> String {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or_else(|| uri.path());
    format!("{} {}", method.as_str(), path_and_query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_is_bound_to_aad() {
        let key = [7u8; 32];
        let envelope_value =
            encrypt_envelope(&key, "POST /api/list", &json!({"path": "/demo"})).unwrap();
        let envelope: Envelope = serde_json::from_value(envelope_value).unwrap();
        let value: serde_json::Value = decrypt_envelope(&key, "POST /api/list", &envelope).unwrap();
        assert_eq!(value["path"], "/demo");
        assert!(
            decrypt_envelope::<serde_json::Value>(&key, "POST /api/delete", &envelope).is_err()
        );
    }

    #[test]
    fn stream_frame_roundtrip_is_bound_to_aad() {
        let key = [9u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let frame = seal_frame(&cipher, "pv:stream:0", b"hello").unwrap();
        assert_eq!(frame.len(), 4 + 12 + 5 + 16);
        let length = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(length, 5 + 16);
        let nonce = Nonce::from_slice(&frame[4..16]);
        let ciphertext = &frame[16..];
        let plaintext = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: ciphertext,
                    aad: b"pv:stream:0",
                },
            )
            .unwrap();
        assert_eq!(plaintext, b"hello");
        assert!(cipher
            .decrypt(
                nonce,
                Payload {
                    msg: ciphertext,
                    aad: b"pv:stream:1",
                },
            )
            .is_err());
    }
}

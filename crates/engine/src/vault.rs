//! Secret environment values, encrypted at rest.
//!
//! Values of keys listed in `Environment::secret_keys` are stored as
//! `vault:v1:<base64(nonce ‖ ciphertext)>` (AES-256-GCM). The key lives in the
//! OS keychain (or the engine's secret store) and is created on first use.
//! Secrets are decrypted only while building a render/script context, and are
//! never written to exports or Git.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use irs_core::{Doc, Environment, VarMap};
use serde_json::Value;

use crate::{Engine, EngineError, Result};

pub const PREFIX: &str = "vault:v1:";
const KEY_ID: &str = "vault-key";

/// What an environment looks like outside this machine (secret values blanked).
fn shareable(e: &Environment) -> Environment {
    let mut e = e.clone();
    for k in &e.secret_keys {
        if let Some(v) = e.data.get_mut(k) {
            *v = Value::String(String::new());
        }
    }
    e
}

pub fn is_sealed(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.starts_with(PREFIX))
}

fn err(msg: impl Into<String>) -> EngineError {
    EngineError::Message(msg.into())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub has_key: bool,
    /// Number of encrypted values in the database.
    pub sealed_values: usize,
}

impl Engine {
    fn vault_cipher(&self, create: bool) -> Result<Option<Aes256Gcm>> {
        if let Some(k) = self.secrets.get(KEY_ID) {
            let bytes = B64
                .decode(k.trim())
                .map_err(|_| err("the vault key in the keychain is corrupt"))?;
            return Aes256Gcm::new_from_slice(&bytes)
                .map(Some)
                .map_err(|_| err("the vault key has the wrong length"));
        }
        if !create {
            return Ok(None);
        }
        let key = Aes256Gcm::generate_key(OsRng);
        self.secrets
            .set(KEY_ID, &B64.encode(key))
            .map_err(|e| err(format!("could not store the vault key: {e}")))?;
        Ok(Some(Aes256Gcm::new(&key)))
    }

    pub fn seal_value(&self, plain: &str) -> Result<String> {
        let cipher = self.vault_cipher(true)?.expect("created");
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ct = cipher
            .encrypt(&nonce, plain.as_bytes())
            .map_err(|_| err("encryption failed"))?;
        let mut out = nonce.to_vec();
        out.extend(ct);
        Ok(format!("{PREFIX}{}", B64.encode(out)))
    }

    pub fn unseal_value(&self, sealed: &str) -> Result<String> {
        let Some(b64) = sealed.strip_prefix(PREFIX) else {
            return Ok(sealed.to_string());
        };
        let cipher = self.vault_cipher(false)?.ok_or_else(|| {
            err("this machine doesn't have the vault key for these secrets — import it (Settings → Vault, or `irs vault import-key`) or re-enter the values")
        })?;
        let bytes = B64.decode(b64).map_err(|_| err("corrupt secret value"))?;
        if bytes.len() < 12 {
            return Err(err("corrupt secret value"));
        }
        let (nonce, ct) = bytes.split_at(12);
        let plain = cipher
            .decrypt(Nonce::from_slice(nonce), ct)
            .map_err(|_| err("a secret couldn't be decrypted with this machine's vault key (was the key replaced?)"))?;
        String::from_utf8(plain).map_err(|_| err("corrupt secret value"))
    }

    /// Encrypt plaintext secret values and decrypt values whose key is no longer secret.
    pub fn seal_env(&self, env: &mut Environment) -> Result<()> {
        let keys: Vec<String> = env.data.keys().cloned().collect();
        for k in keys {
            let v = env.data[&k].clone();
            let secret = env.secret_keys.contains(&k);
            if secret && !is_sealed(&v) {
                let plain = crate::scalar_string(&v);
                if !plain.is_empty() {
                    env.data.insert(k, Value::String(self.seal_value(&plain)?));
                }
            } else if !secret && is_sealed(&v) {
                env.data
                    .insert(k, Value::String(self.unseal_value(v.as_str().unwrap())?));
            }
        }
        Ok(())
    }

    /// The environment's variables with secrets decrypted (for rendering and scripts).
    pub fn open_env_data(&self, env: &Environment) -> Result<VarMap> {
        if !env.data.values().any(is_sealed) {
            return Ok(env.data.clone());
        }
        env.data
            .iter()
            .map(|(k, v)| {
                let v = if is_sealed(v) {
                    Value::String(
                        self.unseal_value(v.as_str().unwrap())
                            .map_err(|e| err(format!("secret '{k}' in '{}': {e}", env.name)))?,
                    )
                } else {
                    v.clone()
                };
                Ok((k.clone(), v))
            })
            .collect()
    }

    /// Save an environment edited by a user or script, keeping the vault invariant.
    pub fn update_environment(&self, doc: &Doc<Environment>) -> Result<Doc<Environment>> {
        let mut d = doc.clone();
        self.seal_env(&mut d.body)?;
        // Secret values never leave this machine, so changing only them keeps the
        // timestamp: exports and Git files stay byte-identical.
        if let Ok(old) = self.store.get::<Environment>(d.id())
            && shareable(&old.body) == shareable(&d.body)
        {
            d.meta.modified = old.meta.modified;
            self.store.batch(|tx| tx.put(&d))?;
            return Ok(d);
        }
        Ok(self.store.update(&d)?)
    }

    /// Set one variable; `secret` marks it as a vault secret (encrypted at rest).
    pub fn set_env_var(
        &self,
        env_id: &str,
        key: &str,
        value: Value,
        secret: bool,
    ) -> Result<Doc<Environment>> {
        let mut d: Doc<Environment> = self.store.get(env_id)?;
        d.body.data.insert(key.to_string(), value);
        d.body.secret_keys.retain(|k| k != key);
        if secret {
            d.body.secret_keys.push(key.to_string());
        }
        self.update_environment(&d)
    }

    /// Plaintext of one secret, for an explicit "reveal".
    pub fn reveal_secret(&self, env_id: &str, key: &str) -> Result<String> {
        let d: Doc<Environment> = self.store.get(env_id)?;
        match d.data.get(key) {
            Some(v) if is_sealed(v) => self.unseal_value(v.as_str().unwrap()),
            Some(v) => Ok(crate::scalar_string(v)),
            None => Err(err(format!("no variable '{key}' in '{}'", d.name))),
        }
    }

    fn all_environments(&self) -> Result<Vec<Doc<Environment>>> {
        Ok(self.store.all_of::<Environment>()?)
    }

    pub fn vault_status(&self) -> Result<VaultStatus> {
        let sealed_values = self
            .all_environments()?
            .iter()
            .map(|e| e.data.values().filter(|v| is_sealed(v)).count())
            .sum();
        Ok(VaultStatus {
            has_key: self.secrets.get(KEY_ID).is_some(),
            sealed_values,
        })
    }

    /// The vault key as base64, to move secrets to another machine.
    pub fn vault_export_key(&self) -> Result<String> {
        self.vault_cipher(true)?;
        self.secrets.get(KEY_ID).ok_or_else(|| err("no vault key"))
    }

    /// Install a vault key from another machine. Refused when it can't decrypt
    /// the secrets already stored here.
    pub fn vault_import_key(&self, key_b64: &str) -> Result<()> {
        let bytes = B64
            .decode(key_b64.trim())
            .map_err(|_| err("that isn't a vault key (expected base64)"))?;
        let cipher = Aes256Gcm::new_from_slice(&bytes)
            .map_err(|_| err("that isn't a vault key (expected 32 bytes)"))?;
        for e in self.all_environments()? {
            for v in e.data.values().filter(|v| is_sealed(v)) {
                let raw = B64
                    .decode(v.as_str().unwrap().trim_start_matches(PREFIX))
                    .unwrap_or_default();
                if raw.len() < 12
                    || cipher
                        .decrypt(Nonce::from_slice(&raw[..12]), &raw[12..])
                        .is_err()
                {
                    return Err(err(format!(
                        "that key can't decrypt the secrets in '{}' — they were encrypted with a different key",
                        e.name
                    )));
                }
            }
        }
        self.secrets
            .set(KEY_ID, &B64.encode(bytes))
            .map_err(|e| err(format!("could not store the vault key: {e}")))
    }

    /// Forget the key and blank every encrypted value (they can't be recovered).
    pub fn vault_reset(&self) -> Result<usize> {
        let envs = self.all_environments()?;
        let mut cleared = 0;
        self.store.batch(|tx| {
            for mut e in envs {
                let mut changed = false;
                for v in e.body.data.values_mut() {
                    if is_sealed(v) {
                        *v = Value::String(String::new());
                        changed = true;
                        cleared += 1;
                    }
                }
                if changed {
                    tx.update(&e)?;
                }
            }
            Ok(())
        })?;
        let _ = self.secrets.delete(KEY_ID);
        Ok(cleared)
    }
}

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use openssl::asn1::Asn1Time;
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::PKey;
use openssl::x509::{X509NameBuilder, X509ReqBuilder, X509};

use crate::config::agent::AgentConfig;
use crate::model::host::AgentIdentityState;

#[derive(Clone, Debug)]
pub struct IdentityStore {
    cert_path: PathBuf,
    key_path: PathBuf,
    ca_path: PathBuf,
}

impl IdentityStore {
    pub fn new(cfg: &AgentConfig) -> Self {
        Self {
            cert_path: PathBuf::from(cfg.cert_path.clone()),
            key_path: PathBuf::from(cfg.key_path.clone()),
            ca_path: PathBuf::from(cfg.ca_path.clone()),
        }
    }

    pub fn ensure_private_key(&self) -> Result<Vec<u8>> {
        if let Ok(existing) = fs::read(&self.key_path) {
            if !existing.is_empty() {
                return Ok(existing);
            }
        }

        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)
            .context("generate agent key: create EC group")?;
        let ec_key = EcKey::generate(&group).context("generate agent key: create EC key")?;
        let pkey = PKey::from_ec_key(ec_key).context("generate agent key: create pkey")?;
        let pem = pkey
            .private_key_to_pem_pkcs8()
            .context("generate agent key: encode pem")?;
        self.write_file(&self.key_path, &pem)?;
        Ok(pem)
    }

    pub fn generate_csr(&self, private_key_pem: &[u8], common_name: &str) -> Result<String> {
        let pkey = PKey::private_key_from_pem(private_key_pem)
            .context("generate agent csr: load private key")?;

        let mut name = X509NameBuilder::new().context("generate agent csr: create subject")?;
        name.append_entry_by_text("CN", common_name)
            .context("generate agent csr: set common name")?;

        let mut builder = X509ReqBuilder::new().context("generate agent csr: create builder")?;
        builder
            .set_subject_name(&name.build())
            .context("generate agent csr: set subject")?;
        builder
            .set_pubkey(&pkey)
            .context("generate agent csr: set public key")?;
        builder
            .sign(&pkey, MessageDigest::sha256())
            .context("generate agent csr: sign request")?;

        let csr = builder.build();
        let pem = csr.to_pem().context("generate agent csr: encode pem")?;
        String::from_utf8(pem).context("generate agent csr: invalid utf8")
    }

    pub fn load_identity(&self) -> Result<AgentIdentityState> {
        let client_cert_pem = fs::read(&self.cert_path)
            .with_context(|| format!("read client cert {}", self.cert_path.display()))?;
        let client_key_pem = fs::read(&self.key_path)
            .with_context(|| format!("read client key {}", self.key_path.display()))?;
        let ca_bundle_pem = fs::read(&self.ca_path)
            .with_context(|| format!("read ca bundle {}", self.ca_path.display()))?;

        let cert_not_after = match X509::from_pem(&client_cert_pem) {
            Ok(cert) => Some(cert.not_after().to_string()),
            Err(_) => None,
        };

        Ok(AgentIdentityState {
            client_cert_pem,
            client_key_pem,
            ca_bundle_pem,
            cert_not_after,
        })
    }

    pub fn save_identity(&self, client_cert_pem: &[u8], ca_bundle_pem: &[u8]) -> Result<()> {
        self.write_file(&self.cert_path, client_cert_pem)?;
        self.write_file(&self.ca_path, ca_bundle_pem)?;
        Ok(())
    }

    pub fn has_usable_client_certificate(&self) -> bool {
        match self.load_identity() {
            Ok(state) => {
                if state.client_cert_pem.is_empty()
                    || state.client_key_pem.is_empty()
                    || state.ca_bundle_pem.is_empty()
                {
                    return false;
                }

                let cert = match X509::from_pem(&state.client_cert_pem) {
                    Ok(cert) => cert,
                    Err(_) => return false,
                };
                let now = match Asn1Time::days_from_now(0) {
                    Ok(now) => now,
                    Err(_) => return false,
                };
                matches!(cert.not_after().compare(&now), Ok(Ordering::Greater))
            }
            Err(_) => false,
        }
    }

    pub fn clear_identity(&self) {
        let _ = fs::remove_file(&self.cert_path);
        let _ = fs::remove_file(&self.ca_path);
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create identity directory {}", parent.display()))?;
        }
        fs::write(path, content)
            .with_context(|| format!("write identity file {}", path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = if path == self.key_path.as_path() {
                0o600
            } else {
                0o640
            };
            fs::set_permissions(path, fs::Permissions::from_mode(mode))
                .with_context(|| format!("set identity file permissions {}", path.display()))?;
        }

        Ok(())
    }
}

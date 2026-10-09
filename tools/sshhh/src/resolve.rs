use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use secrecy::{ExposeSecret, SecretString};

use crate::config::{Server, Settings, infer_auth};
use crate::creds::Credentials;
use crate::error::Error;
use crate::policy::{self, Policy};
use crate::secrets::{BwItem, Reference, Secrets, Shape, Target, parse_reference};

struct Picked {
    value: SecretString,
    material: bool,
}

pub struct EscalationSecrets {
    pub root_user: String,
    pub root_pass: Option<SecretString>,
    pub sudo_pass: Option<SecretString>,
}

pub struct Resolver {
    secrets: Secrets,
    home: Option<PathBuf>,
    cwd: PathBuf,
}

impl Resolver {
    pub fn new(settings: &Settings, env: BTreeMap<String, String>, home: Option<PathBuf>, cwd: PathBuf) -> Resolver {
        Resolver { secrets: Secrets::new(settings, env, home.clone()), home, cwd }
    }

    pub fn secrets_mut(&mut self) -> &mut Secrets {
        &mut self.secrets
    }

    async fn vault_item(&self, server: &Server) -> Result<Option<Arc<BwItem>>, Error> {
        let Some(value) = &server.vault else { return Ok(None) };
        let invalid = |reason: &str| Error::InvalidValue { alias: server.alias.clone(), key: "VAULT", reason: reason.to_string() };
        match parse_reference(value)? {
            Some(Reference { raw, target: Target::Bitwarden { item, field: None } }) => Ok(Some(self.secrets.bw_item(&raw, &item).await?)),
            Some(Reference { target: Target::Bitwarden { .. }, .. }) => Err(invalid("a vault item takes no field, write bw://ITEM")),
            _ => Err(invalid("expected bw://ITEM")),
        }
    }

    /// An explicit value wins; the vault item's custom field of the same name fills the gaps.
    /// A field missing from a fetched item is legitimately unset, a failed fetch is an error.
    async fn pick(&self, configured: Option<&str>, name: &str, vault: Option<&BwItem>, shape: Shape) -> Result<Option<Picked>, Error> {
        if let Some(value) = configured {
            return match parse_reference(value)? {
                Some(reference) => Ok(Some(Picked { value: self.secrets.fetch(&reference, shape).await?, material: true })),
                None => Ok(Some(Picked { value: SecretString::from(value.to_owned()), material: false })),
            };
        }
        match vault {
            Some(item) => Ok(item.custom(name)?.map(|value| Picked { value, material: true })),
            None => Ok(None),
        }
    }

    fn expand(&self, spec: &str) -> PathBuf {
        let rest = spec.strip_prefix("~/").or_else(|| spec.strip_prefix("~\\"));
        let path = match (rest, spec == "~", &self.home) {
            (Some(rest), _, Some(home)) => home.join(rest),
            (None, true, Some(home)) => home.clone(),
            _ => PathBuf::from(spec),
        };
        if path.is_absolute() { path } else { self.cwd.join(path) }
    }

    async fn key(&self, picked: Picked) -> Result<(SecretString, Option<String>), Error> {
        if picked.material {
            return Ok((picked.value, None));
        }
        let path = self.expand(picked.value.expose_secret());
        let unusable = |what: &str, e: std::io::Error| Error::KeyUnusable { reason: format!("cannot read {what} {}: {}", path.display(), e.kind()) };
        let text = tokio::fs::read_to_string(&path).await.map_err(|e| unusable("key file", e))?;
        let mut cert_path = path.clone().into_os_string();
        cert_path.push("-cert.pub");
        let cert = match tokio::fs::read_to_string(Path::new(&cert_path)).await {
            Ok(cert) => Some(cert),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(unusable("certificate", e)),
        };
        Ok((SecretString::from(text), cert))
    }

    pub async fn escalation(&self, server: &Server) -> Result<EscalationSecrets, Error> {
        let vault = self.vault_item(server).await?;
        let vault = vault.as_deref();
        let secret = |picked: Option<Picked>| picked.map(|p| p.value);
        let root_user = secret(self.pick(server.root_user.as_deref(), "ROOT_USER", vault, Shape::Line).await?)
            .map(|u| u.expose_secret().to_owned())
            .unwrap_or_else(|| server.root_user().to_owned());
        let root_pass = secret(self.pick(server.root_pass.as_ref().map(|s| s.expose_secret()), "ROOT_PASS", vault, Shape::Line).await?);
        let sudo_pass = secret(self.pick(server.sudo_pass.as_ref().map(|s| s.expose_secret()), "SUDO_PASS", vault, Shape::Line).await?);
        Ok(EscalationSecrets { root_user, root_pass, sudo_pass })
    }

    /// Policy kept in the vault item's custom fields. It is added to the `.env` policy and can
    /// only restrict further; a failed fetch is an error, never an empty policy.
    pub async fn vault_policy(&self, server: &Server) -> Result<Policy, Error> {
        let mut policy = Policy::default();
        let Some(item) = self.vault_item(server).await? else { return Ok(policy) };
        for key in policy::KEYS {
            if let Some(value) = item.custom(key)? {
                policy
                    .add_layer(key, value.expose_secret())
                    .map_err(|reason| Error::InvalidValue { alias: server.alias.clone(), key, reason })?;
            }
        }
        Ok(policy)
    }

    pub async fn credentials(&self, server: &Server) -> Result<Credentials, Error> {
        let vault = self.vault_item(server).await?;
        let vault = vault.as_deref();
        let text = |picked: Option<Picked>| picked.map(|p| p.value);

        let host = text(self.pick(server.host.as_deref(), "HOST", vault, Shape::Line).await?)
            .ok_or_else(|| Error::MissingHost { alias: server.alias.clone() })?;
        let port = match server.port {
            Some(port) => port,
            None => match text(self.pick(None, "PORT", vault, Shape::Line).await?) {
                Some(raw) => raw.expose_secret().trim().parse::<u16>().map_err(|_| Error::InvalidValue {
                    alias: server.alias.clone(),
                    key: "PORT",
                    reason: "not a port number".into(),
                })?,
                None => server.port(),
            },
        };
        let user = text(self.pick(server.user.as_deref(), "USER", vault, Shape::Line).await?)
            .map(|u| u.expose_secret().to_owned())
            .unwrap_or_else(|| server.user().to_owned());
        let pass = text(self.pick(server.pass.as_ref().map(|s| s.expose_secret()), "PASS", vault, Shape::Line).await?);
        let key_pass = text(self.pick(server.key_pass.as_ref().map(|s| s.expose_secret()), "KEY_PASS", vault, Shape::Line).await?);
        let (key, cert) = match self.pick(server.key.as_deref(), "KEY", vault, Shape::Whole).await? {
            Some(picked) => {
                let (key, cert) = self.key(picked).await?;
                (Some(key), cert)
            }
            None => (None, None),
        };
        let auth = server.auth.clone().unwrap_or_else(|| infer_auth(key.is_some(), pass.is_some()));
        Ok(Credentials {
            alias: server.alias.clone(),
            host: host.expose_secret().trim().to_owned(),
            port,
            user,
            auth,
            pass,
            key,
            key_pass,
            cert,
        })
    }
}

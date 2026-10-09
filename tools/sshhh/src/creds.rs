use secrecy::SecretString;

use crate::config::AuthMethod;

pub struct Credentials {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Vec<AuthMethod>,
    pub pass: Option<SecretString>,
    pub key: Option<SecretString>,
    pub key_pass: Option<SecretString>,
    pub cert: Option<String>,
}

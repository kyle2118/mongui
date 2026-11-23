//! Saved connection profiles: storage, import/export, and turning a profile into a live client.

use crate::jks;
use anyhow::{Context, Result, anyhow, bail};
use mongodb::{
    Client,
    options::{AuthMechanism, ClientOptions, Credential, ServerAddress, Tls, TlsOptions},
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub enum AuthKind {
    None,
    #[default]
    Password,
    /// Personal LDAP login (PLAIN on $external). Its password never leaves this machine.
    Ldap,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub enum ClientCert {
    None,
    #[default]
    Jks,
    Pem,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub enum CaSource {
    /// CA certificates found inside the client keystore.
    #[default]
    Keystore,
    /// A Java trust store such as the team's `cacerts` file.
    TrustStore,
    /// A CA .pem file, as used in Compass.
    Pem,
    /// The operating system's trusted roots.
    System,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub color: usize,
    /// Comma separated `host:port` list.
    pub hosts: String,
    pub srv: bool,
    pub auth: AuthKind,
    pub username: String,
    pub password: String,
    pub auth_db: String,
    pub tls: bool,
    pub client_cert: ClientCert,
    pub client_cert_path: String,
    pub client_cert_password: String,
    pub ca: CaSource,
    pub ca_path: String,
    pub ca_password: String,
    pub insecure: bool,
    pub replica_set: String,
    pub read_preference: String,
    pub default_db: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            color: 0,
            hosts: String::new(),
            srv: false,
            auth: AuthKind::Password,
            username: String::new(),
            password: String::new(),
            auth_db: String::new(),
            tls: true,
            client_cert: ClientCert::Jks,
            client_cert_path: String::new(),
            client_cert_password: String::new(),
            ca: CaSource::Keystore,
            ca_path: String::new(),
            ca_password: String::new(),
            insecure: false,
            replica_set: String::new(),
            read_preference: String::new(),
            default_db: String::new(),
        }
    }
}

pub fn new_id() -> String {
    mongodb::bson::oid::ObjectId::new().to_hex()
}

impl Profile {
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else if !self.hosts.trim().is_empty() {
            self.hosts.split(',').next().unwrap_or_default().trim().to_string()
        } else {
            "New connection".into()
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.hosts.trim().is_empty() {
            bail!("Add at least one host");
        }
        if self.auth != AuthKind::None && self.username.trim().is_empty() {
            bail!("Username is required");
        }
        if self.tls {
            if self.client_cert != ClientCert::None && self.client_cert_path.trim().is_empty() {
                bail!("Choose the client certificate file");
            }
            if matches!(self.ca, CaSource::Pem | CaSource::TrustStore) && self.ca_path.trim().is_empty() {
                bail!("Choose the server CA file");
            }
            if self.ca == CaSource::Keystore && self.client_cert != ClientCert::Jks {
                bail!("\"From client keystore\" needs a JKS client certificate. Pick another server CA option.");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- storage

#[derive(Serialize, Deserialize, Default)]
struct StoreFile {
    version: u32,
    connections: Vec<Profile>,
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MONGUI_HOME") {
        return PathBuf::from(dir);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("mongui")
}

fn store_path() -> PathBuf {
    config_dir().join("connections.json")
}

pub fn load_profiles() -> Vec<Profile> {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|s| serde_json::from_str::<StoreFile>(&s).ok())
        .map(|f| f.connections)
        .unwrap_or_default()
}

fn write_private(path: &std::path::Path, contents: &str) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).with_context(|| format!("Cannot write {}", path.display()))?;
    f.write_all(contents.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn save_profiles(profiles: &[Profile]) -> Result<()> {
    std::fs::create_dir_all(config_dir())?;
    let file = StoreFile { version: 1, connections: profiles.to_vec() };
    write_private(&store_path(), &serde_json::to_string_pretty(&file)?)
}

/// Export for sharing with teammates. LDAP passwords are personal and are always removed.
/// Shared service-account and keystore passwords are kept only if `include_shared_passwords`.
pub fn export_profiles(profiles: &[Profile], path: &std::path::Path, include_shared_passwords: bool) -> Result<()> {
    let cleaned: Vec<Profile> = profiles
        .iter()
        .cloned()
        .map(|mut p| {
            if p.auth == AuthKind::Ldap || !include_shared_passwords {
                p.password.clear();
            }
            if !include_shared_passwords {
                p.client_cert_password.clear();
                p.ca_password.clear();
            }
            p
        })
        .collect();
    let file = StoreFile { version: 1, connections: cleaned };
    write_private(path, &serde_json::to_string_pretty(&file)?)
}

pub fn import_profiles(path: &std::path::Path) -> Result<Vec<Profile>> {
    let text = std::fs::read_to_string(path)?;
    let file: StoreFile = serde_json::from_str(&text).context("This is not a mongui connections file")?;
    Ok(file
        .connections
        .into_iter()
        .map(|mut p| {
            p.id = new_id();
            p
        })
        .collect())
}

// ---------------------------------------------------------------- connection strings

fn pct(s: &str, plus_is_space: bool) -> String {
    let s = if plus_is_space { s.replace('+', " ") } else { s.to_string() };
    percent_encoding::percent_decode_str(&s).decode_utf8_lossy().into_owned()
}

fn truthy(v: &str) -> bool {
    v.eq_ignore_ascii_case("true")
}

/// Fill a profile from a `mongodb://` or `mongodb+srv://` connection string.
pub fn profile_from_uri(uri: &str) -> Result<Profile> {
    let uri = uri.trim();
    let (srv, rest) = if let Some(r) = uri.strip_prefix("mongodb+srv://") {
        (true, r)
    } else if let Some(r) = uri.strip_prefix("mongodb://") {
        (false, r)
    } else {
        bail!("Connection strings start with mongodb:// or mongodb+srv://");
    };
    let (main, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = main.split_once('/').unwrap_or((main, ""));
    let (userinfo, hosts) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };

    let mut p = Profile {
        srv,
        hosts: hosts.to_string(),
        default_db: pct(path, false),
        tls: srv,
        auth: AuthKind::None,
        client_cert: ClientCert::None,
        ca: CaSource::System,
        ..Profile::default()
    };
    if let Some(u) = userinfo {
        let (user, pass) = u.split_once(':').unwrap_or((u, ""));
        p.username = pct(user, false);
        p.password = pct(pass, false);
        if !p.username.is_empty() {
            p.auth = AuthKind::Password;
        }
    }
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = pct(v, true);
        match k.to_ascii_lowercase().as_str() {
            "tls" | "ssl" => p.tls = truthy(&v),
            "tlscafile" => {
                p.tls = true;
                p.ca = CaSource::Pem;
                p.ca_path = v;
            }
            "tlscertificatekeyfile" => {
                p.tls = true;
                p.client_cert = ClientCert::Pem;
                p.client_cert_path = v;
            }
            "tlscertificatekeyfilepassword" => p.client_cert_password = v,
            "tlsinsecure" | "tlsallowinvalidcertificates" | "tlsallowinvalidhostnames" => {
                p.insecure |= truthy(&v)
            }
            "authsource" => p.auth_db = v,
            "authmechanism" => {
                if v.eq_ignore_ascii_case("PLAIN") {
                    p.auth = AuthKind::Ldap;
                }
            }
            "replicaset" => p.replica_set = v,
            "readpreference" => p.read_preference = v,
            _ => {}
        }
    }
    if p.auth == AuthKind::Ldap {
        p.auth_db.clear();
    }
    Ok(p)
}

// ---------------------------------------------------------------- Compass import

pub struct CompassImport {
    pub profiles: Vec<Profile>,
    pub skipped: usize,
    pub passwords_found: usize,
}

fn compass_dirs() -> Vec<PathBuf> {
    let Some(base) = dirs::config_dir() else { return vec![] };
    ["MongoDB Compass", "MongoDB Compass Beta", "MongoDB Compass Dev"]
        .iter()
        .map(|n| base.join(n).join("Connections"))
        .collect()
}

/// Compass 1.37 keeps passwords in the macOS Keychain. Reading them makes macOS ask for
/// permission, so this only runs when the user ticks the box.
fn compass_keychain_secrets(id: &str) -> Option<serde_json::Value> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    for service in ["MongoDB Compass/Connections", "MongoDB Compass Beta/Connections"] {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-a", id, "-w"])
            .output()
            .ok()?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                return Some(v);
            }
        }
    }
    None
}

const COMPASS_COLORS: [&str; 10] =
    ["color1", "color2", "color3", "color4", "color5", "color6", "color7", "color8", "color9", "color10"];

pub fn parse_compass_connection(json: &serde_json::Value) -> Option<Profile> {
    let info = json.get("connectionInfo").unwrap_or(json);
    let uri = info.pointer("/connectionOptions/connectionString")?.as_str()?;
    let name = info.pointer("/favorite/name").and_then(|v| v.as_str())?;
    let mut p = profile_from_uri(uri).ok()?;
    p.name = name.to_string();
    if let Some(c) = info.pointer("/favorite/color").and_then(|v| v.as_str()) {
        p.color = COMPASS_COLORS.iter().position(|x| *x == c).map(|i| i % crate::theme::CONN_COLORS.len()).unwrap_or(0);
    }
    Some(p)
}

pub fn import_from_compass(read_keychain: bool) -> Result<CompassImport> {
    let mut result = CompassImport { profiles: vec![], skipped: 0, passwords_found: 0 };
    let mut found_dir = false;
    for dir in compass_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        found_dir = true;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(json) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            else {
                result.skipped += 1;
                continue;
            };
            let Some(mut p) = parse_compass_connection(&json) else {
                result.skipped += 1;
                continue;
            };
            if read_keychain && p.password.is_empty() {
                let id = json
                    .pointer("/connectionInfo/id")
                    .or_else(|| json.get("_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if let Some(secrets) = compass_keychain_secrets(id) {
                    if let Some(pw) = secrets.get("password").and_then(|v| v.as_str()) {
                        p.password = pw.to_string();
                        result.passwords_found += 1;
                    }
                    if let Some(pw) = secrets.get("tlsCertificateKeyFilePassword").and_then(|v| v.as_str()) {
                        p.client_cert_password = pw.to_string();
                    }
                }
            }
            result.profiles.push(p);
        }
    }
    if !found_dir {
        bail!("No Compass connections folder was found on this computer");
    }
    result.profiles.sort_by_key(|p| p.name.to_lowercase());
    Ok(result)
}

// ---------------------------------------------------------------- connecting

fn pem_file_ders(path: &str) -> Result<Vec<Vec<u8>>> {
    use base64::Engine;
    let text = std::fs::read_to_string(path).with_context(|| format!("Cannot read {path}"))?;
    let mut out = vec![];
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line == "-----BEGIN CERTIFICATE-----" {
            cur = Some(String::new());
        } else if line == "-----END CERTIFICATE-----" {
            if let Some(b64) = cur.take() {
                out.push(
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|_| anyhow!("{path} has a damaged certificate"))?,
                );
            }
        } else if let Some(c) = cur.as_mut() {
            c.push_str(line);
        }
    }
    if out.is_empty() {
        bail!("{path} has no certificates in it");
    }
    Ok(out)
}

/// Builds the driver options. Any PEM the driver needs is written to a private temp folder
/// that the caller deletes as soon as the client has been created.
pub fn client_options(p: &Profile) -> Result<(ClientOptions, Option<tempfile::TempDir>)> {
    p.validate()?;
    let mut hosts = vec![];
    for h in p.hosts.split(',').map(str::trim).filter(|h| !h.is_empty()) {
        hosts.push(ServerAddress::parse(h).map_err(|e| anyhow!("Bad host '{h}': {e}"))?);
    }

    let mut o = ClientOptions::default();
    o.hosts = hosts;
    o.app_name = Some("mongui".into());
    o.connect_timeout = Some(Duration::from_secs(10));
    o.server_selection_timeout = Some(Duration::from_secs(30));
    if !p.replica_set.trim().is_empty() {
        o.repl_set_name = Some(p.replica_set.trim().to_string());
    }
    if !p.default_db.trim().is_empty() {
        o.default_database = Some(p.default_db.trim().to_string());
    }
    if let Some(rp) = read_preference(&p.read_preference) {
        o.selection_criteria = Some(mongodb::options::SelectionCriteria::ReadPreference(rp));
    }

    o.credential = match p.auth {
        AuthKind::None => None,
        AuthKind::Password => Some(
            Credential::builder()
                .username(p.username.trim().to_string())
                .password(p.password.clone())
                .source(if p.auth_db.trim().is_empty() { "admin".to_string() } else { p.auth_db.trim().to_string() })
                .build(),
        ),
        AuthKind::Ldap => Some(
            Credential::builder()
                .username(p.username.trim().to_string())
                .password(p.password.clone())
                .source("$external".to_string())
                .mechanism(AuthMechanism::Plain)
                .build(),
        ),
    };

    let mut tmp = None;
    if p.tls {
        let mut tls = TlsOptions::default();
        let dir = tempfile::Builder::new().prefix("mongui-").tempdir()?;

        let keystore = match p.client_cert {
            ClientCert::Jks => Some(jks::load(p.client_cert_path.trim(), &p.client_cert_password)?),
            _ => None,
        };
        match p.client_cert {
            ClientCert::None => {}
            ClientCert::Pem => {
                tls.cert_key_file_path = Some(p.client_cert_path.trim().into());
                if !p.client_cert_password.is_empty() {
                    tls.tls_certificate_key_file_password = Some(p.client_cert_password.clone().into_bytes());
                }
            }
            ClientCert::Jks => {
                let path = dir.path().join("client.pem");
                write_private(&path, &keystore.as_ref().unwrap().client_pem()?)?;
                tls.cert_key_file_path = Some(path);
            }
        }

        let ca_ders: Option<Vec<Vec<u8>>> = match p.ca {
            CaSource::Pem => Some(pem_file_ders(p.ca_path.trim())?),
            CaSource::TrustStore => {
                let ks = jks::load(p.ca_path.trim(), &p.ca_password)?;
                let ders = ks.ca_ders();
                if ders.is_empty() {
                    bail!("The trust store has no certificates in it");
                }
                Some(ders)
            }
            CaSource::Keystore => {
                let ders = keystore.as_ref().map(|k| k.ca_ders()).unwrap_or_default();
                if ders.is_empty() {
                    bail!(
                        "The client keystore has no CA certificates inside. Choose your cacerts file or a CA .pem as the server CA instead."
                    );
                }
                Some(ders)
            }
            CaSource::System => {
                let native = rustls_native_certs::load_native_certs();
                let ders: Vec<Vec<u8>> = native.certs.iter().map(|c| c.as_ref().to_vec()).collect();
                if ders.is_empty() { None } else { Some(ders) }
            }
        };
        if let Some(ders) = ca_ders {
            let path = dir.path().join("ca.pem");
            write_private(&path, &jks::ders_to_pem(&ders))?;
            tls.ca_file_path = Some(path);
        }
        if p.insecure {
            tls.allow_invalid_certificates = Some(true);
        }
        o.tls = Some(Tls::Enabled(tls));
        tmp = Some(dir);
    }
    Ok((o, tmp))
}

fn read_preference(s: &str) -> Option<mongodb::options::ReadPreference> {
    use mongodb::options::ReadPreference as R;
    let opts = || None;
    match s.trim().to_ascii_lowercase().as_str() {
        "primary" => Some(R::Primary),
        "primarypreferred" => Some(R::PrimaryPreferred { options: opts() }),
        "secondary" => Some(R::Secondary { options: opts() }),
        "secondarypreferred" => Some(R::SecondaryPreferred { options: opts() }),
        "nearest" => Some(R::Nearest { options: opts() }),
        _ => None,
    }
}

pub async fn connect(p: &Profile) -> Result<Client> {
    let (mut opts, tmp) = client_options(p)?;
    if p.srv {
        // Let the driver resolve the SRV record, then re-apply our settings on top.
        let host = p.hosts.split(',').next().unwrap_or_default().trim();
        let mut resolved = ClientOptions::parse(format!("mongodb+srv://{host}/")).await?;
        resolved.credential = opts.credential.take();
        resolved.app_name = opts.app_name.take();
        resolved.connect_timeout = opts.connect_timeout;
        resolved.server_selection_timeout = opts.server_selection_timeout;
        resolved.selection_criteria = opts.selection_criteria.take();
        resolved.default_database = opts.default_database.take();
        if opts.tls.is_some() {
            resolved.tls = opts.tls.take();
        }
        opts = resolved;
    }
    let client = Client::with_options(opts)?;
    // The driver has read the certificates by now; remove the temporary PEM copies.
    drop(tmp);
    client
        .database("admin")
        .run_command(mongodb::bson::doc! {"ping": 1})
        .await
        .map_err(|e| anyhow!(friendly_error(&e)))?;
    Ok(client)
}

pub fn friendly_error(e: &mongodb::error::Error) -> String {
    let raw = e.to_string();
    let lower = raw.to_lowercase();
    let hint = if lower.contains("unknownissuer") || lower.contains("unknown issuer") {
        "The server's certificate is not signed by a CA this connection trusts. Choose your team's cacerts file (or the CA .pem from Compass) as the Server CA."
    } else if lower.contains("notvalidforname") {
        "The server certificate does not list the host name you connected to. Use the exact host names from the certificate, or tick \"Skip server certificate checks\" under More options to confirm this is the cause."
    } else if lower.contains("authentication failed") || lower.contains("scram failure") {
        "Wrong username, password or authentication database."
    } else if lower.contains("wire version") {
        "This server runs a MongoDB version that is too old for mongui's driver."
    } else if lower.contains("unsupportedcertversion") || lower.contains("badsignature") || lower.contains("invalidcertificate") || lower.contains("invalid peer certificate") {
        "The server's certificate was rejected by the TLS check. Tick \"Skip server certificate checks\" under More options to confirm this is the cause, and send the text below to whoever maintains mongui."
    } else if lower.contains("handshakefailure") || lower.contains("handshake failure") || lower.contains("received fatal alert") {
        "The server refused the TLS handshake. Usually the client certificate is not one this server accepts (for example a QA keystore against PROD)."
    } else if lower.contains("server selection timeout") {
        "Could not reach the servers. Check VPN, host names and port, and that TLS settings match the server."
    } else {
        ""
    };
    if hint.is_empty() { raw } else { format!("{hint}\n\n{raw}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compass_style_uri() {
        let p = profile_from_uri(
            "mongodb://app_user:p%40ss@h1.example.com:27017,h2.example.com:27017/?tlsCAFile=%2FUsers%2Fme%2Fcerts%2Ffrom+Anand%2Fca.pem&tlsCertificateKeyFile=%2FUsers%2Fme%2Fclient.pem&tls=true&authSource=admin&replicaSet=rs0",
        )
        .unwrap();
        assert_eq!(p.hosts, "h1.example.com:27017,h2.example.com:27017");
        assert_eq!(p.username, "app_user");
        assert_eq!(p.password, "p@ss");
        assert!(p.tls);
        assert_eq!(p.ca, CaSource::Pem);
        assert_eq!(p.ca_path, "/Users/me/certs/from Anand/ca.pem");
        assert_eq!(p.client_cert, ClientCert::Pem);
        assert_eq!(p.client_cert_path, "/Users/me/client.pem");
        assert_eq!(p.replica_set, "rs0");
        assert_eq!(p.auth, AuthKind::Password);
    }

    #[test]
    fn parses_compass_file() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"_id":"abc","connectionInfo":{"id":"abc","favorite":{"name":"QA","color":"color3"},
            "connectionOptions":{"connectionString":"mongodb://u@h1:27017/?authMechanism=PLAIN&authSource=%24external&tls=true"}}}"#,
        )
        .unwrap();
        let p = parse_compass_connection(&json).unwrap();
        assert_eq!(p.name, "QA");
        assert_eq!(p.auth, AuthKind::Ldap);
        assert!(p.tls);
    }

    #[test]
    fn export_strips_ldap_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        let ldap = Profile { auth: AuthKind::Ldap, password: "mine".into(), hosts: "h".into(), ..Profile::default() };
        let svc = Profile { password: "shared".into(), client_cert_password: "ks".into(), hosts: "h".into(), ..Profile::default() };
        export_profiles(&[ldap.clone(), svc.clone()], &path, true).unwrap();
        let back = import_profiles(&path).unwrap();
        assert_eq!(back[0].password, "");
        assert_eq!(back[1].password, "shared");
        assert_eq!(back[1].client_cert_password, "ks");
        export_profiles(&[ldap, svc], &path, false).unwrap();
        let back = import_profiles(&path).unwrap();
        assert_eq!(back[1].password, "");
        assert_eq!(back[1].client_cert_password, "");
    }

    fn fx(name: &str) -> String {
        format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn reads_jks_and_pkcs12_keystores() {
        let ks = jks::load(&fx("client.jks"), "changeit").unwrap();
        assert_eq!(ks.keys.len(), 1);
        assert_eq!(ks.keys[0].1.len(), 3, "leaf + issuing CA + root");
        assert_eq!(ks.ca_ders().len(), 2);
        let pem = ks.client_pem().unwrap();
        assert!(pem.contains("BEGIN PRIVATE KEY") && pem.matches("BEGIN CERTIFICATE").count() == 3);

        let err = format!("{:#}", jks::load(&fx("client.jks"), "wrong").unwrap_err());
        assert!(err.contains("Wrong keystore password"), "{err}");

        // Trust stores open without a password, like cacerts.
        assert_eq!(jks::load(&fx("cacerts"), "").unwrap().trusted.len(), 1);
        assert_eq!(jks::load(&fx("cacerts"), "changeit").unwrap().trusted.len(), 1);
        assert_eq!(jks::load(&fx("cacerts.p12"), "changeit").unwrap().trusted.len(), 1);
        let p12 = jks::load(&fx("client.p12"), "changeit").unwrap();
        assert_eq!(p12.keys.len(), 1);
    }

    fn live(client_cert: ClientCert, cert: &str, ca: CaSource, ca_file: &str) -> Profile {
        Profile {
            hosts: "localhost:27017".into(),
            username: "app_user".into(),
            password: "s3cret!".into(),
            client_cert,
            client_cert_path: if cert.is_empty() { String::new() } else { fx(cert) },
            client_cert_password: if client_cert == ClientCert::Jks { "changeit".into() } else { String::new() },
            ca,
            ca_path: if ca_file.is_empty() { String::new() } else { fx(ca_file) },
            ..Profile::default()
        }
    }

    /// Needs the TLS test server from tests/README.md: `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn connects_to_tls_server_every_way() {
        let cases = [
            ("jks + CA from keystore", live(ClientCert::Jks, "client.jks", CaSource::Keystore, "")),
            ("jks + cacerts (JKS)", live(ClientCert::Jks, "client.jks", CaSource::TrustStore, "cacerts")),
            ("jks + cacerts (PKCS12)", {
                let mut p = live(ClientCert::Jks, "client.jks", CaSource::TrustStore, "cacerts.p12");
                p.ca_password = "changeit".into();
                p
            }),
            ("pkcs12 keystore + CA pem", live(ClientCert::Jks, "client.p12", CaSource::Pem, "ca.pem")),
            ("two pem files, like Compass", live(ClientCert::Pem, "client.pem", CaSource::Pem, "ca.pem")),
        ];
        for (name, p) in cases {
            let client = connect(&p).await.unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let dbs = client.list_database_names().await.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(dbs.contains(&"admin".to_string()), "{name}");
        }
        // No temp PEM files are left behind.
        let leftovers = std::fs::read_dir(std::env::temp_dir()).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("mongui-")).count();
        assert_eq!(leftovers, 0);

        // Failure cases give a readable reason.
        let no_cert = connect(&live(ClientCert::None, "", CaSource::Pem, "ca.pem")).await;
        assert!(no_cert.is_err(), "server requires a client certificate");
        let untrusted = connect(&live(ClientCert::Pem, "client.pem", CaSource::System, "")).await.unwrap_err();
        assert!(format!("{untrusted:#}").contains("not signed by a CA"), "{untrusted:#}");
        let mut bad_pw = live(ClientCert::Jks, "client.jks", CaSource::Keystore, "");
        bad_pw.password = "nope".into();
        let e = format!("{:#}", connect(&bad_pw).await.unwrap_err());
        assert!(e.contains("Wrong username, password"), "{e}");
    }
}

//! Reads Java keystores (classic JKS, plus PKCS12 which newer JDKs also call ".jks")
//! and turns them into PEM, which is what the MongoDB driver's TLS layer understands.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use sha1::{Digest, Sha1};

#[derive(Default, Debug)]
pub struct KeyStoreContents {
    /// Client identities: (PKCS#8 private key DER, certificate chain DER, leaf first).
    pub keys: Vec<(Vec<u8>, Vec<Vec<u8>>)>,
    /// Trusted certificate entries (DER).
    pub trusted: Vec<Vec<u8>>,
}

impl KeyStoreContents {
    /// PEM with the client certificate chain followed by its private key.
    pub fn client_pem(&self) -> Result<String> {
        let (key, chain) = self
            .keys
            .first()
            .ok_or_else(|| anyhow!("The keystore has no private key entry, so it cannot be used as a client certificate"))?;
        let mut out = String::new();
        for c in chain {
            out.push_str(&pem("CERTIFICATE", c));
        }
        out.push_str(&pem("PRIVATE KEY", key));
        Ok(out)
    }

    /// Every certificate that could act as a CA: trusted entries plus the issuers in key chains.
    pub fn ca_ders(&self) -> Vec<Vec<u8>> {
        let mut out = self.trusted.clone();
        for (_, chain) in &self.keys {
            out.extend(chain.iter().skip(1).cloned());
        }
        out
    }
}

pub fn pem(label: &str, der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

pub fn load(path: &str, password: &str) -> Result<KeyStoreContents> {
    let data = std::fs::read(path).with_context(|| format!("Cannot read keystore {path}"))?;
    parse(&data, password).with_context(|| format!("Cannot open keystore {path}"))
}

pub fn parse(data: &[u8], password: &str) -> Result<KeyStoreContents> {
    if data.len() < 12 {
        bail!("File is too small to be a keystore");
    }
    match &data[..4] {
        [0xFE, 0xED, 0xFE, 0xED] => parse_jks(data, password),
        [0xCE, 0xCE, 0xCE, 0xCE] => bail!(
            "This is a JCEKS keystore, which is not supported. Convert it with: keytool -importkeystore -srckeystore in.jks -destkeystore out.jks -deststoretype JKS"
        ),
        [0x30, ..] => parse_p12(data, password),
        _ => bail!("Not a JKS or PKCS12 keystore"),
    }
}

fn parse_p12(data: &[u8], password: &str) -> Result<KeyStoreContents> {
    use p12_keystore::{KeyStore, KeyStoreEntry, Pkcs12ImportPolicy};
    let ks = KeyStore::from_pkcs12(data, password, Pkcs12ImportPolicy::Relaxed)
        .map_err(|e| anyhow!("PKCS12 keystore could not be opened (wrong password?): {e}"))?;
    let mut out = KeyStoreContents::default();
    for (_, entry) in ks.entries() {
        match entry {
            KeyStoreEntry::PrivateKeyChain(chain) => out.keys.push((
                chain.key().as_der().to_vec(),
                chain.certs().iter().map(|c| c.as_der().to_vec()).collect(),
            )),
            KeyStoreEntry::Certificate(c) => out.trusted.push(c.as_der().to_vec()),
            KeyStoreEntry::Secret(_) => {}
        }
    }
    Ok(out)
}

struct Reader<'a> {
    d: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.i.checked_add(n).filter(|e| *e <= self.d.len()).ok_or_else(|| anyhow!("Keystore file is truncated"))?;
        let s = &self.d[self.i..end];
        self.i = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<usize> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]) as usize)
    }
    fn u32(&mut self) -> Result<usize> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
    fn utf(&mut self) -> Result<()> {
        let n = self.u16()?;
        self.take(n)?;
        Ok(())
    }
    fn cert(&mut self, version: usize) -> Result<Vec<u8>> {
        if version == 2 {
            self.utf()?; // certificate type, always "X.509"
        }
        let n = self.u32()?;
        Ok(self.take(n)?.to_vec())
    }
}

fn password_bytes(password: &str) -> Vec<u8> {
    password.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
}

fn parse_jks(data: &[u8], password: &str) -> Result<KeyStoreContents> {
    let pw = password_bytes(password);
    let body_len = data.len().checked_sub(20).ok_or_else(|| anyhow!("Keystore file is truncated"))?;

    // Integrity check, the same one `keytool` does. Skipped when no password is given,
    // which is fine for reading public certificates out of a trust store such as cacerts.
    if !password.is_empty() {
        let mut h = Sha1::new();
        h.update(&pw);
        h.update(b"Mighty Aphrodite");
        h.update(&data[..body_len]);
        if h.finalize().as_slice() != &data[body_len..] {
            bail!("Wrong keystore password (or the file is corrupted)");
        }
    }

    let mut r = Reader { d: &data[..body_len], i: 4 };
    let version = r.u32()?;
    if version != 1 && version != 2 {
        bail!("Unsupported JKS version {version}");
    }
    let count = r.u32()?;
    let mut out = KeyStoreContents::default();
    for _ in 0..count {
        let tag = r.u32()?;
        r.utf()?; // alias
        r.take(8)?; // creation date
        match tag {
            1 => {
                let n = r.u32()?;
                let protected = r.take(n)?;
                let chain_len = r.u32()?;
                let mut chain = Vec::with_capacity(chain_len);
                for _ in 0..chain_len {
                    chain.push(r.cert(version)?);
                }
                if password.is_empty() {
                    bail!("This keystore holds a private key, so its password is required");
                }
                out.keys.push((unprotect_key(protected, &pw)?, chain));
            }
            2 => out.trusted.push(r.cert(version)?),
            other => bail!("Unknown JKS entry type {other}"),
        }
    }
    Ok(out)
}

/// Reads one DER element: returns (tag, content, rest).
fn der(d: &[u8]) -> Result<(u8, &[u8], &[u8])> {
    let bad = || anyhow!("Malformed key entry in keystore");
    let tag = *d.first().ok_or_else(bad)?;
    let l0 = *d.get(1).ok_or_else(bad)? as usize;
    let (len, hdr) = if l0 < 0x80 {
        (l0, 2)
    } else {
        let n = l0 & 0x7F;
        if n == 0 || n > 4 {
            return Err(bad());
        }
        let bytes = d.get(2..2 + n).ok_or_else(bad)?;
        (bytes.iter().fold(0usize, |a, b| (a << 8) | *b as usize), 2 + n)
    };
    let content = d.get(hdr..hdr + len).ok_or_else(bad)?;
    Ok((tag, content, &d[hdr + len..]))
}

/// Undo Sun's proprietary JKS key protection (OID 1.3.6.1.4.1.42.2.17.1.1):
/// a SHA-1 keystream seeded with the password and a 20-byte salt, XORed over the PKCS#8 key.
fn unprotect_key(protected: &[u8], pw: &[u8]) -> Result<Vec<u8>> {
    const SUN_JKS_OID: &[u8] = &[0x2B, 0x06, 0x01, 0x04, 0x01, 0x2A, 0x02, 0x11, 0x01, 0x01];
    let (_, seq, _) = der(protected)?;
    let (_, alg, rest) = der(seq)?;
    let (_, oid, _) = der(alg)?;
    if oid != SUN_JKS_OID {
        bail!("The private key uses an unsupported protection algorithm");
    }
    let (tag, enc, _) = der(rest)?;
    if tag != 0x04 || enc.len() < 40 {
        bail!("Malformed key entry in keystore");
    }
    let salt = &enc[..20];
    let body = &enc[20..enc.len() - 20];
    let check = &enc[enc.len() - 20..];

    let mut plain = Vec::with_capacity(body.len());
    let mut digest: Vec<u8> = salt.to_vec();
    for chunk in body.chunks(20) {
        let mut h = Sha1::new();
        h.update(pw);
        h.update(&digest);
        digest = h.finalize().to_vec();
        plain.extend(chunk.iter().zip(digest.iter()).map(|(a, b)| a ^ b));
    }
    let mut h = Sha1::new();
    h.update(pw);
    h.update(&plain);
    if h.finalize().as_slice() != check {
        bail!("Wrong password for the private key in the keystore");
    }
    Ok(plain)
}

/// Converts PEM text holding certificates into DER blobs (used for de-duplicating CA bundles).
pub fn ders_to_pem(ders: &[Vec<u8>]) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    for d in ders {
        if seen.insert(d.clone()) {
            out.push_str(&pem("CERTIFICATE", d));
        }
    }
    out
}

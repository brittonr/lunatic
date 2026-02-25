use std::path::Path;

use anyhow::Result;
use rcgen::*;

pub static TEST_ROOT_CERT: &str = r#"""
-----BEGIN CERTIFICATE-----
MIIBnDCCAUGgAwIBAgIIR5Hk+O5RdOgwCgYIKoZIzj0EAwIwKTEQMA4GA1UEAwwH
Um9vdCBDQTEVMBMGA1UECgwMTHVuYXRpYyBJbmMuMCAXDTc1MDEwMTAwMDAwMFoY
DzQwOTYwMTAxMDAwMDAwWjApMRAwDgYDVQQDDAdSb290IENBMRUwEwYDVQQKDAxM
dW5hdGljIEluYy4wWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAARlVNxYAwsmmFNc
2EMBbZZVwL8GBtnnu8IROdDd68ixc0VBjfrV0zAM344lKJcs9slsMTEofoYvMCpI
BhnSGyAFo1EwTzAdBgNVHREEFjAUghJyb290Lmx1bmF0aWMuY2xvdWQwHQYDVR0O
BBYEFOh0Ue745JFH76xErjqkW2/SbHhAMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZI
zj0EAwIDSQAwRgIhAJKPv4XUZ9ej+CVgsJ+9x/CmJEcnebyWh2KntJri97nxAiEA
/KvaQE6GtYZPGFv/WYM3YEmTQ7hoOvaaAuvD27cHkaw=
-----END CERTIFICATE-----
"""#;

pub static CTRL_SERVER_NAME: &str = "ctrl.lunatic.cloud";

static TEST_ROOT_KEYS: &str = r#"""
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg9ferf0du4h975Jhu
boMyGfdI+xwp7ewOulGvpTcvdpehRANCAARlVNxYAwsmmFNc2EMBbZZVwL8GBtnn
u8IROdDd68ixc0VBjfrV0zAM344lKJcs9slsMTEofoYvMCpIBhnSGyAF
-----END PRIVATE KEY-----"""#;

/// Returns the test root CA certificate and its key pair.
pub fn test_root_cert() -> Result<(Certificate, KeyPair)> {
    let key_pair = KeyPair::from_pem(TEST_ROOT_KEYS)?;
    let root_params = CertificateParams::from_ca_cert_pem(TEST_ROOT_CERT)?;
    let root_cert = root_params.self_signed(&key_pair)?;
    Ok((root_cert, key_pair))
}

/// Returns the root CA certificate and its key pair from files.
pub fn root_cert(ca_cert: &str, ca_keys: &str) -> Result<(Certificate, KeyPair)> {
    let ca_cert_pem = std::fs::read(Path::new(ca_cert))?;
    let ca_keys_pem = std::fs::read(Path::new(ca_keys))?;
    let key_pair = KeyPair::from_pem(std::str::from_utf8(&ca_keys_pem)?)?;
    let root_params = CertificateParams::from_ca_cert_pem(std::str::from_utf8(&ca_cert_pem)?)?;
    let root_cert = root_params.self_signed(&key_pair)?;
    Ok((root_cert, key_pair))
}

pub fn default_server_certificates(
    root_cert: &Certificate,
    root_key_pair: &KeyPair,
) -> Result<(String, String)> {
    let mut ctrl_params = CertificateParams::new(vec![CTRL_SERVER_NAME.into()])?;
    ctrl_params
        .distinguished_name
        .push(DnType::OrganizationName, "Lunatic Inc.");
    ctrl_params
        .distinguished_name
        .push(DnType::CommonName, "Control CA");
    let ctrl_key_pair = KeyPair::generate()?;
    let cert = ctrl_params.signed_by(&ctrl_key_pair, root_cert, root_key_pair)?;
    let cert_pem = cert.pem();
    let key_pem = ctrl_key_pair.serialize_pem();
    Ok((cert_pem, key_pem))
}

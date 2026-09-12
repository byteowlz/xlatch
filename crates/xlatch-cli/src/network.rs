//! Provider-independent interface discovery and certified pairing origins.

use anyhow::{Context, Result, ensure};
use std::net::{IpAddr, SocketAddr};

pub fn origins(listen: SocketAddr, explicit: Option<&str>) -> Result<Vec<String>> {
    if let Some(origin) = explicit {
        let url = url::Url::parse(origin)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "--public-url must be an HTTPS origin"
        );
        return Ok(vec![origin.to_owned()]);
    }
    let mut addresses = if listen.ip().is_unspecified() {
        interfaces(listen.is_ipv4())?
    } else {
        vec![listen.ip()]
    };
    addresses.sort();
    addresses.dedup();
    // Shared-address space is commonly used by mesh VPNs. No provider CLI is needed.
    addresses.sort_by_key(|ip| match ip {
        IpAddr::V4(ip) if ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]) => 0,
        _ => 1,
    });
    ensure!(
        !addresses.is_empty(),
        "no usable network address found; connect your network or specify --public-url"
    );
    Ok(addresses
        .into_iter()
        .take(8)
        .map(|ip| format!("https://{}", SocketAddr::new(ip, listen.port())))
        .collect())
}

#[cfg(unix)]
fn interfaces(ipv4: bool) -> Result<Vec<IpAddr>> {
    use nix::{ifaddrs::getifaddrs, net::if_::InterfaceFlags};
    Ok(getifaddrs()?
        .filter(|interface| interface.flags.contains(InterfaceFlags::IFF_UP))
        .filter_map(|interface| {
            let address = interface.address?;
            if ipv4 {
                address
                    .as_sockaddr_in()
                    .map(|address| IpAddr::V4(address.ip()))
            } else {
                address
                    .as_sockaddr_in6()
                    .map(|address| IpAddr::V6(address.ip()))
            }
        })
        .filter(|ip| match ip {
            IpAddr::V4(ip) => {
                !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
            }
            IpAddr::V6(ip) => {
                !ip.is_loopback()
                    && !ip.is_unicast_link_local()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
            }
        })
        .collect())
}

#[cfg(not(unix))]
fn interfaces(_: bool) -> Result<Vec<IpAddr>> {
    anyhow::bail!("automatic discovery currently requires Unix")
}

pub fn certificate_covers(der: &[u8], origin: &str) -> bool {
    covers(der, origin).is_ok()
}

fn covers(der: &[u8], origin: &str) -> Result<()> {
    use rustls::{
        client::danger::ServerCertVerifier,
        pki_types::{CertificateDer, ServerName, UnixTime},
    };
    let certificate = CertificateDer::from(der.to_vec());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate.clone())?;
    let verifier =
        rustls::client::WebPkiServerVerifier::builder(std::sync::Arc::new(roots)).build()?;
    let url = url::Url::parse(origin)?;
    let name = ServerName::try_from(
        url.host_str()
            .context("missing host")?
            .trim_matches(['[', ']'])
            .to_owned(),
    )?;
    verifier.verify_server_cert(&certificate, &[], &name, &[], UnixTime::now())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_origins_reject_credentials_and_insecure_transport() -> Result<()> {
        let listen = "0.0.0.0:7443".parse()?;
        for invalid in [
            "http://host:7443",
            "https://user@host:7443",
            "https://host/path",
            "https://host?token=secret",
        ] {
            ensure!(
                origins(listen, Some(invalid)).is_err(),
                "invalid origin accepted"
            );
        }
        ensure!(
            origins(listen, Some("https://server.example:7443"))?
                == vec!["https://server.example:7443"],
            "explicit origin changed"
        );
        ensure!(
            origins("127.0.0.1:7443".parse()?, None)? == vec!["https://127.0.0.1:7443"],
            "loopback override changed"
        );
        Ok(())
    }

    #[test]
    fn certificate_address_check_preserves_host_validation() -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let identity =
            rcgen::generate_simple_self_signed(vec!["100.64.0.12".into(), "192.168.1.240".into()])?;
        ensure!(
            certificate_covers(identity.cert.der(), "https://100.64.0.12:7443"),
            "mesh SAN rejected"
        );
        ensure!(
            certificate_covers(identity.cert.der(), "https://192.168.1.240:7443"),
            "LAN SAN rejected"
        );
        ensure!(
            !certificate_covers(identity.cert.der(), "https://192.168.1.241:7443"),
            "wrong SAN accepted"
        );
        Ok(())
    }
}

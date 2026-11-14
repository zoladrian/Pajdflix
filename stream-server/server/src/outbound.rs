//! Resolve once, validate every result and pin the connection to that resolution.
//! Redirects are returned to the caller, never followed to an unchecked destination.
use anyhow::{Result, ensure};
use std::net::{IpAddr, SocketAddr};
use url::Url;

pub(crate) fn initialize_tls() {
    // librqbit and HTTP dependencies can enable different providers together.
    // Select one explicitly instead of letting Rustls panic on ambiguous features.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

fn public_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only ordinary global-unicast IPv6, excluding special-purpose ranges,
            // Teredo, 6to4 and documentation. IPv4-mapped addresses were canonicalized.
            s[0] & 0xe000 == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] <= 0x1ff || s[1] == 0xdb8))
                && !(s[0] == 0x3fff && s[1] <= 0x0fff)
        }
    }
}

pub(crate) async fn client_for(url: &Url) -> Result<reqwest::Client> {
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "source must use HTTP(S)"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "URL credentials are forbidden"
    );
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("missing source host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("missing port"))?;
    let addresses: Vec<SocketAddr> = match url.host() {
        Some(url::Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Some(url::Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        _ => tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::net::lookup_host((host, port)),
        )
        .await??
        .collect(),
    };
    ensure!(
        !addresses.is_empty() && addresses.iter().all(|address| public_ip(address.ip())),
        "private or special-purpose source address"
    );
    Ok(pinned_builder(host, &addresses).build()?)
}

fn pinned_builder(host: &str, addresses: &[SocketAddr]) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(30))
        .resolve_to_addrs(host, addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_private_and_transition_addresses() {
        for raw in [
            "0.0.0.0",
            "10.1.1.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "2002:7f00:1::",
            "64:ff9b::7f00:1",
        ] {
            assert!(!public_ip(raw.parse().unwrap()), "{raw}");
        }
        for raw in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(raw.parse().unwrap()), "{raw}");
        }
    }
    #[tokio::test]
    async fn rejects_local_urls_before_connecting() {
        for raw in [
            "http://127.0.0.1:1/",
            "http://[::1]/",
            "http://2130706433/",
            "http://user:password@1.1.1.1/",
            "file:///C:/Windows/win.ini",
        ] {
            assert!(
                client_for(&Url::parse(raw).unwrap()).await.is_err(),
                "{raw}"
            );
        }
    }

    #[tokio::test]
    async fn https_rejects_self_signed_certificates() {
        initialize_tls();
        // The key below is a public test fixture, never a production identity.
        let certificate = include_bytes!("../tests/fixtures/self-signed-cert.pem");
        let key = include_bytes!("../tests/fixtures/self-signed-key.pem");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let tls =
            axum_server::tls_rustls::RustlsConfig::from_pem(certificate.to_vec(), key.to_vec())
                .await
                .unwrap();
        let app =
            axum::Router::new().route("/", axum::routing::get(|| async { "legal TLS fixture" }));
        let server = tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .serve(app.into_make_service())
                .await
        });
        let url = format!("https://source.example:{}/", address.port());
        // Only this internal test helper pins a public test hostname to loopback.
        // Production client_for rejects loopback addresses before calling it.
        let untrusted = pinned_builder("source.example", &[address])
            .build()
            .unwrap();
        let error = untrusted.get(&url).send().await.unwrap_err();
        assert!(error.is_connect(), "unexpected TLS failure: {error}");
        let trusted = pinned_builder("source.example", &[address])
            .add_root_certificate(reqwest::Certificate::from_pem(certificate).unwrap())
            .build()
            .unwrap();
        assert_eq!(
            trusted.get(&url).send().await.unwrap().status(),
            reqwest::StatusCode::OK,
            "positive control must prove the TLS endpoint is reachable"
        );
        server.abort();
    }
}

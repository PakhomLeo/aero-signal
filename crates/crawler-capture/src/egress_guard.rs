//! SSRF/egress target validation shared by page and asset fetchers.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use url::Url;

use crate::error::{CrawlerError, CrawlerResult};

pub async fn validate_public_http_url(url: &Url) -> CrawlerResult<()> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CrawlerError::new(
            "crawler.url_scheme_unsupported",
            "target URL must use http or https",
            false,
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| CrawlerError::new("crawler.url_invalid", "target URL has no host", false))?;
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower == "metadata.google.internal"
        || lower == "metadata"
    {
        return Err(ssrf_error());
    }

    let port = url.port_or_known_default().ok_or_else(|| {
        CrawlerError::new(
            "crawler.url_invalid",
            "target URL has no supported port",
            false,
        )
    })?;
    let addrs: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| CrawlerError::new("crawler.dns_failed", "target DNS lookup failed", true))?
            .collect(),
    };
    if addrs.iter().any(|addr| is_forbidden_ip(addr.ip())) {
        return Err(ssrf_error());
    }
    Ok(())
}

fn ssrf_error() -> CrawlerError {
    CrawlerError::new(
        "crawler.egress_blocked",
        "target resolves to a private or local network address",
        false,
    )
}

fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4 == Ipv4Addr::new(169, 254, 169, 254)
                || v4 == Ipv4Addr::new(100, 100, 100, 200)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_unspecified()
                || v6 == Ipv6Addr::LOCALHOST
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blocks_private_and_metadata_targets() {
        for raw in [
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://metadata.google.internal/",
        ] {
            let url = Url::parse(raw).unwrap();
            assert_eq!(
                validate_public_http_url(&url).await.unwrap_err().code,
                "crawler.egress_blocked"
            );
        }
    }

    #[tokio::test]
    async fn blocks_non_http_schemes() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert_eq!(
            validate_public_http_url(&url).await.unwrap_err().code,
            "crawler.url_scheme_unsupported"
        );
    }
}

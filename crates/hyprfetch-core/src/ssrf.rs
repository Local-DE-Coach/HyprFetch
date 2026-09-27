//! URL safety checks — SSRF protection.
//!
//! Rejects schemes other than http/https, and (when enabled) blocks hosts
//! that resolve to private/loopback/link-local addresses.

use std::net::IpAddr;

use thiserror::Error;
use url::Url;

/// SSRF check failure.
#[derive(Debug, Error)]
pub enum UrlSafetyError {
    /// Scheme is not http or https.
    #[error("scheme must be http or https, got {0}")]
    BadScheme(String),
    /// Host is missing.
    #[error("URL is missing a host")]
    MissingHost,
    /// Host resolved to a blocked IP range.
    #[error("host {host} resolves to blocked IP {ip} ({range})")]
    BlockedIp {
        host: String,
        ip: IpAddr,
        range: &'static str,
    },
    /// DNS resolution failed.
    #[error("DNS resolution failed for {host}: {err}")]
    DnsFailed { host: String, err: String },
}

/// Policy for the SSRF check.
#[derive(Debug, Clone, Copy)]
pub struct SsrfPolicy {
    /// When true, hosts that resolve to private/loopback/link-local addresses
    /// are rejected.
    pub block_private: bool,
}

impl Default for SsrfPolicy {
    fn default() -> Self {
        Self {
            block_private: true,
        }
    }
}

/// Validate the URL scheme. Does not touch DNS.
pub fn validate_scheme(url: &Url) -> Result<(), UrlSafetyError> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(UrlSafetyError::BadScheme(other.to_string())),
    }
}

/// Returns `true` if the IP is in a private/loopback/link-local range.
///
/// Used by both the URL validator (when DNS resolves the host) and by the
/// redirect-chain validator (each hop is re-checked).
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                // Carrier-grade NAT: 100.64.0.0/10
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local fc00::/7
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local fe80::/10
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Returns a human-readable name for the blocked range, or `None` if the IP
/// is not in any known private range.
pub fn private_range_name(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                Some("loopback")
            } else if v4.is_private() {
                Some("private (RFC 1918)")
            } else if v4.is_link_local() {
                Some("link-local")
            } else if v4.is_unspecified() {
                Some("unspecified")
            } else if v4.is_broadcast() {
                Some("broadcast")
            } else if v4.is_documentation() {
                Some("documentation")
            } else if v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64 {
                Some("CGNAT (100.64.0.0/10)")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                Some("loopback")
            } else if v6.is_unspecified() {
                Some("unspecified")
            } else if v6.is_multicast() {
                Some("multicast")
            } else if (v6.segments()[0] & 0xfe00) == 0xfc00 {
                Some("unique local (fc00::/7)")
            } else if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                Some("link-local (fe80::/10)")
            } else {
                None
            }
        }
    }
}

/// Full URL safety check: scheme + host + (optionally) DNS-resolved IP.
///
/// The DNS resolution is synchronous — this is fine because the engine
/// calls it once per task creation, not in any hot path.
pub fn check_url(url: &Url, policy: SsrfPolicy) -> Result<(), UrlSafetyError> {
    validate_scheme(url)?;
    let host = url.host_str().ok_or(UrlSafetyError::MissingHost)?;

    if !policy.block_private {
        return Ok(());
    }

    // IP-literal URLs (e.g. http://127.0.0.1:8080/) — check directly.
    if let Some(ip) = url.host().and_then(|h| match h {
        url::Host::Ipv4(v) => Some(IpAddr::V4(v)),
        url::Host::Ipv6(v) => Some(IpAddr::V6(v)),
        url::Host::Domain(_) => None,
    }) {
        if let Some(range) = private_range_name(ip) {
            return Err(UrlSafetyError::BlockedIp {
                host: host.to_string(),
                ip,
                range,
            });
        }
        return Ok(());
    }

    // Domain — resolve and check each A/AAAA record.
    use std::net::ToSocketAddrs;
    let port = url.port_or_known_default().unwrap_or(80);
    let resolved: Vec<IpAddr> = match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs.map(|s| s.ip()).collect(),
        Err(e) => {
            return Err(UrlSafetyError::DnsFailed {
                host: host.to_string(),
                err: e.to_string(),
            });
        }
    };
    if resolved.is_empty() {
        return Err(UrlSafetyError::DnsFailed {
            host: host.to_string(),
            err: "no A/AAAA records".into(),
        });
    }
    for ip in resolved {
        if let Some(range) = private_range_name(ip) {
            return Err(UrlSafetyError::BlockedIp {
                host: host.to_string(),
                ip,
                range,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn rejects_ftp_scheme() {
        let err = validate_scheme(&url("ftp://example.com/x")).unwrap_err();
        assert!(matches!(err, UrlSafetyError::BadScheme(s) if s == "ftp"));
    }

    #[test]
    fn accepts_http_and_https() {
        validate_scheme(&url("http://example.com/x")).unwrap();
        validate_scheme(&url("https://example.com/x")).unwrap();
    }

    #[test]
    fn blocks_loopback_ipv4() {
        let err = check_url(&url("http://127.0.0.1/x"), SsrfPolicy::default()).unwrap_err();
        assert!(matches!(err, UrlSafetyError::BlockedIp { .. }));
    }

    #[test]
    fn blocks_private_ipv4_10() {
        let err = check_url(&url("http://10.0.0.1/x"), SsrfPolicy::default()).unwrap_err();
        assert!(matches!(err, UrlSafetyError::BlockedIp { .. }));
    }

    #[test]
    fn blocks_private_ipv4_192_168() {
        let err = check_url(&url("http://192.168.1.1/x"), SsrfPolicy::default()).unwrap_err();
        assert!(matches!(
            err,
            UrlSafetyError::BlockedIp {
                range: "private (RFC 1918)",
                ..
            }
        ));
    }

    #[test]
    fn blocks_private_ipv4_172_16() {
        let err = check_url(&url("http://172.16.0.1/x"), SsrfPolicy::default()).unwrap_err();
        assert!(matches!(err, UrlSafetyError::BlockedIp { .. }));
    }

    #[test]
    fn blocks_loopback_ipv6() {
        let err = check_url(&url("http://[::1]/x"), SsrfPolicy::default()).unwrap_err();
        assert!(matches!(
            err,
            UrlSafetyError::BlockedIp {
                range: "loopback",
                ..
            }
        ));
    }

    #[test]
    fn blocks_cgnat_range() {
        // 100.64.0.1 is in the CGNAT range.
        assert!(is_private_ip(IpAddr::V4("100.64.0.1".parse().unwrap())));
        // 100.128.0.1 also CGNAT (top of /10)
        assert!(is_private_ip(IpAddr::V4(
            "100.127.255.255".parse().unwrap()
        )));
        // 100.128.0.1 is outside CGNAT
        assert!(!is_private_ip(IpAddr::V4("100.128.0.1".parse().unwrap())));
    }

    #[test]
    fn allows_public_ipv4() {
        // 8.8.8.8 is Google DNS — a public IP. (Test will pass even without network.)
        let policy = SsrfPolicy::default();
        let u = url("http://8.8.8.8/x");
        // Direct IP — no DNS lookup needed.
        check_url(&u, policy).unwrap();
    }

    #[test]
    fn allows_when_block_private_disabled() {
        let policy = SsrfPolicy {
            block_private: false,
        };
        check_url(&url("http://127.0.0.1/x"), policy).unwrap();
    }

    #[test]
    fn dns_failure_is_reported() {
        let err = check_url(
            &url("http://this-host-does-not-exist-zzz.invalid/x"),
            SsrfPolicy::default(),
        )
        .unwrap_err();
        // Note: depending on resolver config this may be either DnsFailed or BlockedIp.
        // We accept either; both indicate the URL was rejected.
        assert!(matches!(
            err,
            UrlSafetyError::DnsFailed { .. } | UrlSafetyError::BlockedIp { .. }
        ));
    }
}

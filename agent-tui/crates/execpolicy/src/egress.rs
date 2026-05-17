//! Egress policy: which hosts a tool can reach.
//!
//! Default policy:
//!   - Loopback always allowed.
//!   - Private RFC 1918 ranges blocked unless explicitly allow-listed.
//!   - All other hosts pass through (Phase 2 default; tightenable per-config).

use std::collections::HashSet;
use std::net::IpAddr;
use url::Url;

#[derive(Debug, Clone)]
pub enum EgressVerdict {
    Allow,
    Block(String),
}

#[derive(Debug, Default, Clone)]
pub struct EgressPolicy {
    pub allow_hosts: HashSet<String>,
    pub block_hosts: HashSet<String>,
    pub block_private_ranges: bool,
}

impl EgressPolicy {
    pub fn restrictive() -> Self {
        Self {
            allow_hosts: HashSet::new(),
            block_hosts: HashSet::new(),
            block_private_ranges: true,
        }
    }

    pub fn check_url(&self, raw: &str) -> EgressVerdict {
        let url = match Url::parse(raw) {
            Ok(u) => u,
            Err(e) => return EgressVerdict::Block(format!("invalid url: {e}")),
        };
        let host = match url.host_str() {
            Some(h) => h,
            None => return EgressVerdict::Block("no host".into()),
        };
        if self.block_hosts.contains(host) {
            return EgressVerdict::Block(format!("blocked host: {host}"));
        }
        if self.allow_hosts.contains(host) {
            return EgressVerdict::Allow;
        }
        if host == "localhost" {
            return EgressVerdict::Allow;
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if ip.is_loopback() {
                return EgressVerdict::Allow;
            }
            if self.block_private_ranges {
                match ip {
                    IpAddr::V4(v4) if v4.is_private() => {
                        return EgressVerdict::Block(format!("private ip: {v4}"));
                    }
                    IpAddr::V6(v6) if v6.is_loopback() => return EgressVerdict::Allow,
                    _ => {}
                }
            }
        }
        EgressVerdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_allowed() {
        let p = EgressPolicy::restrictive();
        assert!(matches!(p.check_url("http://localhost:8080/x"), EgressVerdict::Allow));
        assert!(matches!(p.check_url("http://127.0.0.1/"), EgressVerdict::Allow));
    }

    #[test]
    fn private_blocked() {
        let p = EgressPolicy::restrictive();
        assert!(matches!(p.check_url("http://192.168.1.1/x"), EgressVerdict::Block(_)));
    }

    #[test]
    fn public_allowed() {
        let p = EgressPolicy::restrictive();
        assert!(matches!(p.check_url("https://api.anthropic.com/"), EgressVerdict::Allow));
    }
}

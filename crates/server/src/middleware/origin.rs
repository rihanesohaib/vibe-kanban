use std::{net::IpAddr, sync::OnceLock};

use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    response::Response,
};
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
struct OriginKey {
    https: bool,
    host: String,
    port: u16,
}

impl OriginKey {
    fn from_origin(origin: &str) -> Option<Self> {
        let url = Url::parse(origin).ok()?;
        let https = match url.scheme() {
            "http" => false,
            "https" => true,
            _ => return None,
        };
        let host = normalize_host(url.host_str()?);
        let port = url.port_or_known_default()?;
        Some(Self { https, host, port })
    }

    fn from_host_header(host: &str, https: bool) -> Option<Self> {
        let authority: axum::http::uri::Authority = host.parse().ok()?;
        let host = normalize_host(authority.host());
        let port = authority.port_u16().unwrap_or_else(|| default_port(https));
        Some(Self { https, host, port })
    }
}

/// Reject requests whose Host is not loopback (`localhost`, `127.0.0.1`,
/// `[::1]`, any port) or a host listed in `VK_ALLOWED_ORIGINS`. This blocks
/// DNS rebinding, where an attacker-controlled name resolves to loopback.
#[allow(clippy::result_large_err)]
pub fn validate_host<B>(req: &mut Request<B>) -> Result<(), Response> {
    match request_host_name(req) {
        Some(name) if is_allowed_host_name(&name, allowed_hosts()) => Ok(()),
        _ => Err(forbidden()),
    }
}

/// Host check for the preview proxy listener, which is addressed as
/// `{port}.localhost:{proxy_port}`. Accepts everything [`validate_host`] does
/// plus one-label subdomains of those hosts.
#[allow(clippy::result_large_err)]
pub fn validate_preview_proxy_host<B>(req: &mut Request<B>) -> Result<(), Response> {
    match request_host_name(req) {
        Some(name) if is_allowed_preview_host_name(&name, allowed_hosts()) => Ok(()),
        _ => Err(forbidden()),
    }
}

/// Origin check. Must run after [`validate_host`]: the same-origin shortcut
/// below is only safe once the Host itself is known to be allowlisted.
#[allow(clippy::result_large_err)]
pub fn validate_origin<B>(req: &mut Request<B>) -> Result<(), Response> {
    let Some(origin) = get_origin_header(req) else {
        return Ok(());
    };

    if origin.eq_ignore_ascii_case("null") {
        return Err(forbidden());
    }

    let host = get_host_header(req);

    // quick short-circuit same-origin check
    if host.is_some_and(|host| origin_matches_host(origin, host)) {
        return Ok(());
    }

    let Some(origin_key) = OriginKey::from_origin(origin) else {
        return Err(forbidden());
    };

    if allowed_origins()
        .iter()
        .any(|allowed| allowed == &origin_key)
    {
        return Ok(());
    }

    if let Some(host_key) =
        host.and_then(|host| OriginKey::from_host_header(host, origin_key.https))
        && host_key == origin_key
    {
        return Ok(());
    }

    Err(forbidden())
}

fn get_origin_header<B>(req: &Request<B>) -> Option<&str> {
    get_header(req, header::ORIGIN)
}

fn get_host_header<B>(req: &Request<B>) -> Option<&str> {
    get_header(req, header::HOST)
}

/// Normalized host name (no port) from the Host header, or from the URI
/// authority for HTTP/2 requests that carry no Host header.
fn request_host_name<B>(req: &Request<B>) -> Option<String> {
    let authority = match get_host_header(req) {
        Some(host) => host.parse::<axum::http::uri::Authority>().ok()?,
        None => req.uri().authority()?.clone(),
    };
    Some(normalize_host(authority.host()))
}

fn is_allowed_host_name(name: &str, allowed: &[String]) -> bool {
    name == "localhost" || allowed.iter().any(|host| host == name)
}

fn is_allowed_preview_host_name(name: &str, allowed: &[String]) -> bool {
    is_allowed_host_name(name, allowed)
        || name.split_once('.').is_some_and(|(label, parent)| {
            !label.is_empty() && is_allowed_host_name(parent, allowed)
        })
}

fn parse_allowed_hosts(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            if entry.contains("://") {
                Url::parse(entry).ok()?.host_str().map(normalize_host)
            } else {
                entry
                    .parse::<axum::http::uri::Authority>()
                    .ok()
                    .map(|authority| normalize_host(authority.host()))
            }
        })
        .collect()
}

fn allowed_hosts() -> &'static [String] {
    static ALLOWED: OnceLock<Vec<String>> = OnceLock::new();
    ALLOWED.get_or_init(|| {
        std::env::var("VK_ALLOWED_ORIGINS")
            .map(|value| parse_allowed_hosts(&value))
            .unwrap_or_default()
    })
}

fn get_header<B>(req: &Request<B>, name: header::HeaderName) -> Option<&str> {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
}

fn forbidden() -> Response {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn origin_matches_host(origin: &str, host: &str) -> bool {
    origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .is_some_and(|rest| rest.eq_ignore_ascii_case(host))
}

fn normalize_host(host: &str) -> String {
    let trimmed = host.trim().trim_start_matches('[').trim_end_matches(']');
    let lower = trimmed.to_ascii_lowercase();
    if lower == "localhost" {
        return "localhost".to_string();
    }
    if let Ok(ip) = lower.parse::<IpAddr>() {
        if ip.is_loopback() {
            return "localhost".to_string();
        }
        return ip.to_string();
    }
    lower
}

fn default_port(https: bool) -> u16 {
    if https { 443 } else { 80 }
}

fn allowed_origins() -> &'static Vec<OriginKey> {
    static ALLOWED: OnceLock<Vec<OriginKey>> = OnceLock::new();
    ALLOWED.get_or_init(|| {
        let value = match std::env::var("VK_ALLOWED_ORIGINS") {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };

        value
            .split(',')
            .filter_map(|origin| OriginKey::from_origin(origin.trim()))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use axum::http::{Request, header};

    use super::*;

    fn make_request(origin: Option<&str>, host: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/test").method("GET");
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        if let Some(host) = host {
            builder = builder.header(header::HOST, host);
        }
        builder.body(Body::empty()).unwrap()
    }

    fn is_forbidden(result: Result<(), Response>) -> bool {
        matches!(result, Err(resp) if resp.status() == StatusCode::FORBIDDEN)
    }

    #[test]
    fn no_origin_header_allows_request() {
        let mut req = make_request(None, Some("example.com"));
        assert!(validate_origin(&mut req).is_ok());
    }

    #[test]
    fn null_origin_is_forbidden() {
        for null in ["null", "NULL", "Null"] {
            let mut req = make_request(Some(null), Some("example.com"));
            assert!(is_forbidden(validate_origin(&mut req)));
        }
    }

    #[test]
    fn same_origin_allows_request() {
        // HTTP, HTTPS, with port, case-insensitive
        let cases = [
            ("http://example.com", "example.com"),
            ("https://example.com", "example.com"),
            ("http://example.com:8080", "example.com:8080"),
            ("http://EXAMPLE.COM", "example.com"),
        ];
        for (origin, host) in cases {
            let mut req = make_request(Some(origin), Some(host));
            assert!(validate_origin(&mut req).is_ok(), "{origin} vs {host}");
        }
    }

    #[test]
    fn cross_origin_forbidden() {
        let cases = [
            ("http://unknown.com", "example.com"),         // different host
            ("http://example.com:8080", "example.com:80"), // different port
            ("ftp://example.com", "example.com"),          // non-http scheme
            ("not-a-valid-url", "example.com"),            // invalid URL
            ("http://example.com", ""),                    // missing host (invalid)
        ];
        for (origin, host) in cases {
            let host_opt = if host.is_empty() { None } else { Some(host) };
            let mut req = make_request(Some(origin), host_opt);
            assert!(is_forbidden(validate_origin(&mut req)), "{origin}");
        }
    }

    #[test]
    fn loopback_addresses_normalized_and_equivalent() {
        // All loopback forms normalize to "localhost"
        assert_eq!(
            OriginKey::from_origin("http://localhost:3000")
                .unwrap()
                .host,
            "localhost"
        );
        assert_eq!(
            OriginKey::from_origin("http://127.0.0.1:3000")
                .unwrap()
                .host,
            "localhost"
        );
        assert_eq!(
            OriginKey::from_origin("http://[::1]:3000").unwrap().host,
            "localhost"
        );

        // Cross-loopback requests should be allowed
        let mut req = make_request(Some("http://127.0.0.1:3000"), Some("[::1]:3000"));
        assert!(validate_origin(&mut req).is_ok());
    }

    #[test]
    fn default_ports_handled_correctly() {
        assert_eq!(
            OriginKey::from_origin("http://example.com").unwrap().port,
            80
        );
        assert_eq!(
            OriginKey::from_origin("https://example.com").unwrap().port,
            443
        );

        // Explicit default port matches implicit
        let mut req = make_request(Some("http://example.com:80"), Some("example.com"));
        assert!(validate_origin(&mut req).is_ok());
    }

    fn host_name_of(host: &str) -> Option<String> {
        request_host_name(&make_request(None, Some(host)))
    }

    #[test]
    fn loopback_hosts_are_allowed_on_any_port() {
        for host in [
            "localhost",
            "localhost:3005",
            "LOCALHOST:1",
            "127.0.0.1",
            "127.0.0.1:3005",
            "[::1]",
            "[::1]:3005",
        ] {
            let name = host_name_of(host).unwrap_or_else(|| panic!("{host} should parse"));
            assert!(is_allowed_host_name(&name, &[]), "{host}");
        }
    }

    #[test]
    fn foreign_hosts_are_rejected() {
        for host in [
            "evil.example",
            "evil.example:3005",
            "localhost.evil.example",
            "3005.localhost",
            "0.0.0.0:3005",
        ] {
            let name = host_name_of(host).unwrap();
            assert!(!is_allowed_host_name(&name, &[]), "{host}");
        }
        let mut no_host = make_request(None, None);
        assert!(is_forbidden(validate_host(&mut no_host)));
        let mut evil = make_request(None, Some("evil.example"));
        assert!(is_forbidden(validate_host(&mut evil)));
    }

    #[test]
    fn rebinding_request_with_matching_origin_is_rejected_by_host_check() {
        let mut req = make_request(
            Some("http://attacker.example:3005"),
            Some("attacker.example:3005"),
        );
        assert!(is_forbidden(validate_host(&mut req)));
    }

    #[test]
    fn allowed_origins_env_entries_extend_the_host_allowlist() {
        let allowed = parse_allowed_hosts(
            "http://localhost:3005, https://Box.Example.test, other.example.test:8080, ,not a url",
        );
        assert_eq!(
            allowed,
            vec![
                "localhost".to_string(),
                "box.example.test".to_string(),
                "other.example.test".to_string(),
            ]
        );
        let name = host_name_of("box.example.test:443").unwrap();
        assert!(is_allowed_host_name(&name, &allowed));
        assert!(!is_allowed_host_name("evil.example", &allowed));
    }

    #[test]
    fn preview_proxy_accepts_localhost_subdomains_only() {
        let allowed = vec!["box.example.test".to_string()];
        for host in [
            "3000.localhost:4000",
            "3000--abc.localhost:4000",
            "localhost:4000",
            "127.0.0.1:4000",
            "3000.box.example.test",
        ] {
            let name = host_name_of(host).unwrap();
            assert!(is_allowed_preview_host_name(&name, &allowed), "{host}");
        }
        for host in [
            "3000.evil.example:4000",
            "3005.attacker.example",
            "a.b.localhost",
            "localhost.evil.example",
        ] {
            let name = host_name_of(host).unwrap();
            assert!(!is_allowed_preview_host_name(&name, &allowed), "{host}");
        }
    }
}

use url::{Host, Url};

use super::{contract_error, DynamicLanError, ErrorKind, CONTROL_PORT};

pub(crate) fn control_base_url(host: &str) -> Result<Url, DynamicLanError> {
    let host = host.trim();
    if host.is_empty()
        || host.len() > 253
        || host.starts_with('-')
        || host.contains(['/', '@', '?', '#'])
        || host.chars().any(char::is_whitespace)
        || host.contains(':')
    {
        return Err(DynamicLanError::new(
            ErrorKind::Contract,
            "dynamic_lan host must be a hostname or private IP address without a scheme, port, or path.",
        ));
    }
    let url = Url::parse(&format!("http://{host}:{CONTROL_PORT}/")).map_err(contract_error)?;
    if !url_is_local(&url) {
        return Err(DynamicLanError::new(
            ErrorKind::Contract,
            "dynamic_lan host must be a loopback, private-network, .local, or single-label host.",
        ));
    }
    Ok(url)
}

pub(crate) fn url_is_local(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => valid_local_hostname(host),
        Some(Host::Ipv4(address)) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        Some(Host::Ipv6(address)) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
        None => false,
    }
}

fn valid_local_hostname(host: &str) -> bool {
    let labels = host.split('.').collect::<Vec<_>>();
    (labels.len() == 1 || host.ends_with(".local"))
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub(crate) fn url_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

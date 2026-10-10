//! Which addresses a run may read pages from.
//!
//! A run that starts at a public address reads only public addresses. A page can name any host in its links, its redirects
//! and the files it points at (`robots.txt`, sitemaps, `llms.txt`), and jurl would otherwise fetch whatever those name:
//! the machine's own services, the private network behind it, a cloud metadata address. A run that starts at a private
//! address (a local page the user asks for on purpose) reads as it always has.
//!
//! The check is at the connection, not only at the URL: [`GlobalOnly`] is the DNS resolver of the guarded clients, so the
//! address a connection goes to is the one checked, and a name that rebinds to a private address after the start is refused
//! too. A literal address never reaches a resolver, so it is checked before each request ([`check`]), and so is each redirect's
//! target: the guarded clients follow no redirects of their own (see `fetch::send_get`). Under a proxy the proxy connects, so
//! the check before each request is what covers it; the README says what that does not cover.

use std::{
    collections::HashSet,
    error::Error,
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use anyhow::{Context, Result, bail};
use url::{Host, Url};

/// How many redirects a page's request may follow, as the normal client has always allowed.
pub(crate) const MAX_REDIRECTS: usize = 10;

/// What a run may read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Reach {
    /// The run started at a private address, or at no address: any address, as before.
    #[default]
    Private,
    /// The run started at a public address: only public addresses, and `allowed`, the start's own addresses. In a real run
    /// `allowed` holds public addresses only (see [`Reach::of`]), so the check is `is_global` alone; a test may name a
    /// loopback start here to make the local test server the public start.
    Public { allowed: HashSet<IpAddr> },
}

impl Reach {
    /// The reach of a run that starts at `start`: public when the start is at a public address (a literal, or a name with at
    /// least one public address, whose public addresses are then the only ones the run may use), private otherwise. A name
    /// that does not resolve is private: its request fails the same way it always has.
    pub(crate) async fn of(start: &Url) -> Reach {
        let found: Vec<IpAddr> = match start.host() {
            Some(Host::Ipv4(ip)) => vec![IpAddr::V4(ip)],
            Some(Host::Ipv6(ip)) => vec![IpAddr::V6(ip)],
            Some(Host::Domain(name)) => {
                let port = start.port_or_known_default().unwrap_or(80);
                match tokio::net::lookup_host((name, port)).await {
                    Ok(addrs) => addrs.map(|a| a.ip()).collect(),
                    Err(_) => Vec::new(),
                }
            }
            None => Vec::new(),
        };
        let allowed: HashSet<IpAddr> = found.into_iter().filter(|ip| is_global(*ip)).collect();
        if allowed.is_empty() { Reach::Private } else { Reach::Public { allowed } }
    }
}

/// The refusal for an address a run does not read. Its text is the trace's reason: `jurl: skipped <url>: not a public address`.
#[derive(Debug)]
pub(crate) struct NotPublic;

impl fmt::Display for NotPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a public address")
    }
}

impl Error for NotPublic {}

/// Whether `err`, or an error it wraps, is the guard's refusal. The clients wrap a redirect's or a resolver's error in their
/// own, so the refusal is looked for along the whole chain.
pub(crate) fn refused(err: &(dyn Error + 'static)) -> bool {
    let mut cur: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if e.is::<NotPublic>() {
            return true;
        }
        cur = e.source();
    }
    false
}

/// The literal address `url` names, if its host is one. A name is None: it is checked where it connects.
fn literal(url: &Url) -> Option<IpAddr> {
    match url.host()? {
        Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
        Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
        Host::Domain(_) => None,
    }
}

/// Refuses `url` when `reach` does not admit its literal address: under a public run, an address that is neither public nor
/// one of the start's own. A name is admitted here; the resolver decides it at the connection.
pub(crate) fn admit(url: &Url, reach: &Reach) -> Result<(), NotPublic> {
    match reach {
        Reach::Private => Ok(()),
        Reach::Public { allowed } => match literal(url) {
            Some(ip) if !is_global(ip) && !allowed.contains(&ip) => Err(NotPublic),
            _ => Ok(()),
        },
    }
}

/// The addresses in `found` that are public, or the refusal when none are.
pub(crate) fn public_only(found: impl IntoIterator<Item = SocketAddr>) -> Result<Vec<SocketAddr>, NotPublic> {
    let public: Vec<SocketAddr> = found.into_iter().filter(|a| is_global(a.ip())).collect();
    if public.is_empty() { Err(NotPublic) } else { Ok(public) }
}

/// The DNS resolver of the guarded clients: the system's answers for a name, with the addresses that are not public removed.
/// A name with no public answer fails, so a connection is never made to a private address. reqwest and the browser client
/// ask it for each new connection, so the address checked is the address connected to. The names in `exempt` are a proxy's
/// own host (see [`proxy_hosts`]): a connection to the proxy is not a page's address, so those names resolve as the system does.
pub(crate) struct GlobalOnly {
    exempt: Vec<String>,
}

impl GlobalOnly {
    pub(crate) fn new(exempt: Vec<String>) -> Self {
        GlobalOnly { exempt }
    }

    fn is_exempt(&self, name: &str) -> bool {
        self.exempt.iter().any(|e| e.eq_ignore_ascii_case(name))
    }
}

async fn lookup(name: &str, exempt: bool) -> Result<Vec<SocketAddr>, Box<dyn Error + Send + Sync>> {
    // Port 0: the client sets the URL's own port on each address.
    let found = tokio::net::lookup_host((name, 0)).await?;
    if exempt { Ok(found.collect()) } else { Ok(public_only(found)?) }
}

impl reqwest::dns::Resolve for GlobalOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let exempt = self.is_exempt(&host);
        Box::pin(async move {
            let addrs = lookup(&host, exempt).await?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

impl wreq::dns::Resolve for GlobalOnly {
    fn resolve(&self, name: wreq::dns::Name) -> wreq::dns::Resolving {
        let host = name.as_str().to_string();
        let exempt = self.is_exempt(&host);
        Box::pin(async move {
            let addrs = lookup(&host, exempt).await?;
            Ok(Box::new(addrs.into_iter()) as wreq::dns::Addrs)
        })
    }
}

/// The variables a proxy is read from, in the order reqwest reads them (both spellings).
const PROXY_VARS: [&str; 6] = ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"];

/// Whether a proxy is set, as `get` reads the environment. An empty value is not a proxy.
pub(crate) fn proxy_configured(get: impl Fn(&str) -> Option<String>) -> bool {
    PROXY_VARS.iter().any(|name| get(name).is_some_and(|v| !v.trim().is_empty()))
}

/// The host names of the proxies `get` reads: the first names a connection goes to, when a proxy is used.
pub(crate) fn proxy_hosts(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    PROXY_VARS
        .iter()
        .filter_map(|name| get(name))
        .filter_map(|value| {
            let value = value.trim();
            let url = if value.contains("://") { value.to_string() } else { format!("http://{value}") };
            Url::parse(&url).ok()?.host_str().map(str::to_ascii_lowercase)
        })
        .collect()
}

/// Checks the address `url` will be read from, before a request for it, under `reach`. A literal must be admitted (see
/// [`admit`]); a name must resolve to public addresses only, since the request may connect to any of them. A private run
/// checks nothing. Under a proxy the proxy resolves the name itself, so this check is what covers the request (see README).
pub(crate) async fn check(url: &Url, reach: &Reach) -> Result<()> {
    if matches!(reach, Reach::Private) {
        return Ok(());
    }
    admit(url, reach)?;
    if literal(url).is_some() {
        return Ok(());
    }
    let Some(host) = url.host_str() else { bail!("{url}: no host to check") };
    let port = url.port_or_known_default().unwrap_or(80);
    let answers: Vec<SocketAddr> =
        tokio::net::lookup_host((host, port)).await.with_context(|| format!("resolving {host}"))?.collect();
    if answers.is_empty() {
        bail!("{host}: no address to check");
    }
    if answers.iter().all(|a| is_global(a.ip())) { Ok(()) } else { Err(NotPublic.into()) }
}

/// Whether `JURL_PUBLIC_ONLY` is set to 1: the start URL must then be public, or the run is refused. The cloud runner sets it.
pub(crate) fn required_by_env(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1")
}

/// The reach of a run from `start`. With `required` (see [`required_by_env`]) a start that is not public is an error, so the
/// run reads public addresses or nothing.
pub(crate) async fn decide(start: &Url, required: bool) -> Result<Reach> {
    let reach = Reach::of(start).await;
    if required && reach == Reach::Private {
        bail!("{start}: not a public address, and JURL_PUBLIC_ONLY=1 allows only public addresses");
    }
    Ok(reach)
}

/// Sets `args`' reach from its start URL, and the public-only flag from `JURL_PUBLIC_ONLY`, once per run, after `prepare` has
/// given the URL its scheme. A start that is not a URL keeps the private reach, and the run fails where it always has.
pub(crate) async fn set_reach(args: &mut crate::cli::Args) -> Result<()> {
    let env = |name: &str| std::env::var(name).ok();
    set_reach_with(args, required_by_env(env("JURL_PUBLIC_ONLY").as_deref()), proxy_configured(env)).await
}

/// [`set_reach`] with the environment given. Under `public_only` a proxy is refused, and so is a start that is not a public
/// address or not a URL.
async fn set_reach_with(args: &mut crate::cli::Args, public_only: bool, proxy: bool) -> Result<()> {
    args.public_only = public_only;
    if public_only && proxy {
        bail!(
            "JURL_PUBLIC_ONLY=1 refuses a proxy: HTTP_PROXY, HTTPS_PROXY or ALL_PROXY is set, and a proxy resolves the page's name itself, outside the address check"
        );
    }
    let Ok(start) = Url::parse(&args.url) else {
        if public_only {
            bail!("{}: not a URL, and JURL_PUBLIC_ONLY=1 allows only public addresses", args.url);
        }
        return Ok(());
    };
    args.reach = decide(&start, public_only).await?;
    Ok(())
}

/// Whether `ip` is a public address: one a page on the open internet can name, so one a run may read. Everything else is
/// refused. That is the machine's own network, the private and shared networks, links, documentation and test ranges,
/// multicast, reserved blocks, and the IPv4 addresses that IPv6 forms name (see [`is_global_v6`]).
pub(crate) fn is_global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_global_v4(ip),
        IpAddr::V6(ip) => is_global_v6(ip),
    }
}

/// Whether the top `prefix` bits of `ip` equal those of `net`.
fn within(ip: u32, net: u32, prefix: u32) -> bool {
    let mask = u32::MAX << (32 - prefix);
    ip & mask == net & mask
}

/// The IANA registry of special-purpose IPv4 addresses (RFC 6890). Std's stable methods cover the ranges they name; the rest
/// are written out with their RFCs.
fn is_global_v4(ip: Ipv4Addr) -> bool {
    let bits = u32::from(ip);
    let net = |a: u8, b: u8, c: u8, d: u8| u32::from(Ipv4Addr::new(a, b, c, d));
    !(ip.is_unspecified() // 0.0.0.0 (RFC 1122 §3.2.1.3)
        || ip.is_loopback() // 127/8 (RFC 1122 §3.2.1.3)
        || ip.is_private() // 10/8, 172.16/12, 192.168/16 (RFC 1918)
        || ip.is_link_local() // 169.254/16, which holds cloud metadata services (RFC 3927)
        || ip.is_broadcast() // 255.255.255.255 (RFC 919 §7)
        || ip.is_documentation() // 192.0.2/24, 198.51.100/24, 203.0.113/24 (RFC 5737)
        || ip.is_multicast() // 224/4 (RFC 5771)
        || within(bits, net(0, 0, 0, 0), 8) // "this network" (RFC 791 §3.2, RFC 1122 §3.2.1.3)
        || within(bits, net(100, 64, 0, 0), 10) // shared address space, for carrier-grade NAT (RFC 6598)
        || within(bits, net(192, 0, 0, 0), 24) // IETF protocol assignments, including DS-Lite (RFC 6890 §2.1, RFC 6333)
        || within(bits, net(192, 88, 99, 0), 24) // 6to4 relay anycast (RFC 7526, which deprecates it)
        || within(bits, net(198, 18, 0, 0), 15) // benchmarking (RFC 2544)
        || within(bits, net(240, 0, 0, 0), 4)) // reserved for future use (RFC 1112 §4)
}

/// The IPv4 address an IPv6 address carries in its last 32 bits.
fn embedded_v4(ip: Ipv6Addr) -> Ipv4Addr {
    let s = ip.segments();
    Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8)
}

/// The IPv6 forms of IPv4 addresses are judged by the IPv4 address they carry, so `::ffff:127.0.0.1` is refused as
/// `127.0.0.1` is. Otherwise the global unicast space (2000::/3, RFC 4291 §2.4) holds the public addresses, less the blocks
/// in it that are special-purpose.
fn is_global_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        // ::ffff:0:0/96, IPv4-mapped (RFC 4291 §2.5.5.2): the IPv4 address decides.
        return is_global_v4(v4);
    }
    let s = ip.segments();
    if s[..6] == [0; 6] {
        // ::/96, IPv4-compatible (RFC 4291 §2.5.5.1), which is deprecated and never routed; this covers ::1 and :: too.
        return false;
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        // 64:ff9b::/96, the NAT64 well-known prefix (RFC 6052): a gateway turns it into the embedded IPv4 address.
        return is_global_v4(embedded_v4(ip));
    }
    let special = ip.is_multicast() // ff00::/8 (RFC 4291 §2.7)
        || s[0] & 0xffc0 == 0xfe80 // fe80::/10, link-local (RFC 4291 §2.5.6)
        || s[0] & 0xffc0 == 0xfec0 // fec0::/10, site-local, deprecated (RFC 3879)
        || s[0] & 0xfe00 == 0xfc00 // fc00::/7, unique-local (RFC 4193)
        || s[0] == 0x2001 && s[1] == 0 // 2001::/32, Teredo (RFC 4380)
        || s[0] == 0x2001 && s[1] == 0x0002 && s[2] == 0 // 2001:2::/48, benchmarking (RFC 5180)
        || s[0] == 0x2001 && s[1] & 0xfff0 == 0x0010 // 2001:10::/28, ORCHID, deprecated (RFC 4843)
        || s[0] == 0x2001 && s[1] == 0x0db8 // 2001:db8::/32, documentation (RFC 3849)
        || s[0] == 0x2002 // 2002::/16, 6to4, which is not routed on the open internet (RFC 3056, RFC 7526)
        || s[0] == 0x3fff && s[1] & 0xf000 == 0; // 3fff::/20, documentation (RFC 9637)
    // 2000::/3 is the global unicast space; the block tests above take out its special-purpose parts.
    !special && s[0] & 0xe000 == 0x2000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse().expect("an IPv4 address"))
    }

    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().expect("an IPv6 address"))
    }

    #[test]
    fn this_network_is_refused() {
        assert!(!is_global(v4("0.0.0.0")));
        assert!(!is_global(v4("0.1.2.3")));
        assert!(!is_global(v4("0.255.255.255")));
    }

    #[test]
    fn private_networks_are_refused_at_their_edges() {
        for ip in ["10.0.0.0", "10.255.255.255", "172.16.0.0", "172.31.255.255", "192.168.0.0", "192.168.255.255"] {
            assert!(!is_global(v4(ip)), "{ip}");
        }
        assert!(is_global(v4("172.32.0.0")), "just past 172.16/12");
        assert!(is_global(v4("11.0.0.1")), "just past 10/8");
    }

    #[test]
    fn shared_address_space_for_carrier_nat_is_refused() {
        assert!(!is_global(v4("100.64.0.0")));
        assert!(!is_global(v4("100.127.255.255")));
        assert!(is_global(v4("100.128.0.0")), "just past 100.64/10");
    }

    #[test]
    fn loopback_is_refused() {
        assert!(!is_global(v4("127.0.0.1")));
        assert!(!is_global(v4("127.255.255.254")));
    }

    #[test]
    fn link_local_and_the_metadata_address_are_refused() {
        assert!(!is_global(v4("169.254.169.254")));
        assert!(!is_global(v4("169.254.0.1")));
        assert!(is_global(v4("169.255.0.1")), "just past 169.254/16");
    }

    #[test]
    fn ietf_protocol_assignments_are_refused() {
        assert!(!is_global(v4("192.0.0.1")));
        assert!(!is_global(v4("192.0.0.255")));
        assert!(is_global(v4("192.0.1.1")), "just past 192.0.0/24");
    }

    #[test]
    fn documentation_ranges_are_refused() {
        assert!(!is_global(v4("192.0.2.1")));
        assert!(!is_global(v4("198.51.100.1")));
        assert!(!is_global(v4("203.0.113.1")));
    }

    #[test]
    fn benchmarking_is_refused() {
        assert!(!is_global(v4("198.18.0.1")));
        assert!(!is_global(v4("198.19.255.255")));
        assert!(is_global(v4("198.20.0.0")), "just past 198.18/15");
    }

    #[test]
    fn multicast_reserved_and_broadcast_are_refused() {
        assert!(!is_global(v4("224.0.0.1")));
        assert!(!is_global(v4("240.0.0.1")));
        assert!(!is_global(v4("255.255.255.255")));
    }

    #[test]
    fn the_6to4_relay_anycast_block_is_refused() {
        assert!(!is_global(v4("192.88.99.1")));
    }

    #[test]
    fn public_addresses_pass() {
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
            assert!(is_global(v4(ip)), "{ip}");
        }
    }

    #[test]
    fn unspecified_and_loopback_v6_are_refused() {
        assert!(!is_global(v6("::")));
        assert!(!is_global(v6("::1")));
    }

    #[test]
    fn ipv4_mapped_v6_takes_the_ipv4_address_status() {
        assert!(!is_global(v6("::ffff:127.0.0.1")), "mapped loopback");
        assert!(!is_global(v6("::ffff:10.0.0.1")), "mapped private");
        assert!(!is_global(v6("::ffff:169.254.169.254")), "mapped metadata");
        assert!(is_global(v6("::ffff:8.8.8.8")), "mapped public");
    }

    #[test]
    fn ipv4_compatible_v6_is_refused_even_for_a_public_address() {
        assert!(!is_global(v6("::127.0.0.1")));
        assert!(!is_global(v6("::8.8.8.8")), "deprecated, never routed");
    }

    #[test]
    fn nat64_takes_the_status_of_the_ipv4_address_it_embeds() {
        assert!(!is_global(v6("64:ff9b::10.0.0.1")));
        assert!(!is_global(v6("64:ff9b::127.0.0.1")));
        assert!(is_global(v6("64:ff9b::8.8.8.8")));
    }

    #[test]
    fn unique_local_v6_is_refused() {
        assert!(!is_global(v6("fd00::1")));
        assert!(!is_global(v6("fc00::1")));
    }

    #[test]
    fn link_local_v6_is_refused() {
        assert!(!is_global(v6("fe80::1")));
        assert!(!is_global(v6("febf::1")));
    }

    #[test]
    fn site_local_v6_is_refused() {
        assert!(!is_global(v6("fec0::1")));
    }

    #[test]
    fn multicast_v6_is_refused() {
        assert!(!is_global(v6("ff02::1")));
    }

    #[test]
    fn documentation_teredo_orchid_benchmarking_and_6to4_v6_are_refused() {
        assert!(!is_global(v6("2001:db8::1")), "documentation");
        assert!(!is_global(v6("3fff::1")), "documentation");
        assert!(!is_global(v6("2001::1")), "Teredo");
        assert!(!is_global(v6("2001:2::1")), "benchmarking");
        assert!(!is_global(v6("2001:10::1")), "ORCHID");
        assert!(!is_global(v6("2002::1")), "6to4");
    }

    #[test]
    fn addresses_outside_global_unicast_are_refused() {
        assert!(!is_global(v6("4000::1")));
        assert!(!is_global(v6("100::1")), "discard-only");
    }

    #[test]
    fn public_v6_passes() {
        assert!(is_global(v6("2606:4700:4700::1111")));
        assert!(is_global(v6("2001:4860:4860::8888")));
    }

    #[test]
    fn literal_hosts_are_admitted_only_when_public_or_allowed() {
        let local = Url::parse("http://127.0.0.2:8080/x").unwrap();
        let metadata = Url::parse("http://169.254.169.254/latest").unwrap();
        let public = Url::parse("http://93.184.216.34/").unwrap();
        let name = Url::parse("http://localhost:8080/").unwrap();
        assert!(admit(&local, &Reach::Private).is_ok(), "a private run admits everything");
        assert!(admit(&metadata, &Reach::Private).is_ok());
        let public_run = Reach::Public { allowed: HashSet::from([v4("127.0.0.1")]) };
        assert!(admit(&public, &public_run).is_ok());
        assert!(admit(&name, &public_run).is_ok(), "a name is decided at the connection");
        assert!(matches!(admit(&local, &public_run), Err(NotPublic)));
        assert!(matches!(admit(&metadata, &public_run), Err(NotPublic)));
        let start = Url::parse("http://127.0.0.1:8080/").unwrap();
        assert!(admit(&start, &public_run).is_ok(), "the start's own address is allowed");
    }

    #[test]
    fn the_resolver_keeps_public_answers_only() {
        let public: SocketAddr = "93.184.216.34:0".parse().unwrap();
        let private: SocketAddr = "10.0.0.5:0".parse().unwrap();
        let kept = public_only([private, public]).expect("a public answer");
        assert_eq!(kept, vec![public]);
        assert!(matches!(public_only([private]), Err(NotPublic)));
        assert!(matches!(public_only(std::iter::empty()), Err(NotPublic)));
    }

    #[tokio::test]
    async fn a_name_that_resolves_only_to_loopback_is_refused_by_the_guarded_client() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let port = listener.local_addr().expect("the address").port();
        let client =
            reqwest::Client::builder().no_proxy().dns_resolver(GlobalOnly::new(Vec::new())).build().expect("a client");
        let err = client.get(format!("http://localhost:{port}/")).send().await.expect_err("refused");
        assert!(refused(&err), "the refusal is in the chain: {err:?}");
    }

    #[test]
    fn required_is_set_by_the_value_one_only() {
        assert!(required_by_env(Some("1")));
        assert!(required_by_env(Some(" 1 ")));
        assert!(!required_by_env(Some("0")));
        assert!(!required_by_env(Some("yes")));
        assert!(!required_by_env(None));
    }

    #[tokio::test]
    async fn the_run_reach_is_set_from_the_start_url_and_public_only_fails_closed() {
        use clap::Parser;
        let parse = |url: &str| crate::cli::Args::try_parse_from(["jurl", url]).expect("args");
        let mut private = parse("http://127.0.0.1:8080/");
        assert!(set_reach_with(&mut private, true, false).await.is_err(), "a private start is refused when required");
        let mut private = parse("http://127.0.0.1:8080/");
        set_reach_with(&mut private, false, false).await.expect("a private run");
        assert_eq!(private.reach, Reach::Private);
        let mut public = parse("http://93.184.216.34/");
        set_reach_with(&mut public, true, false).await.expect("a public run");
        assert!(matches!(public.reach, Reach::Public { .. }));
        let mut broken = parse("http://93.184.216.34/");
        broken.url = "https://exa mple.com".to_string();
        assert!(set_reach_with(&mut broken, true, false).await.is_err(), "a start that is not a URL is not public");
        broken.reach = Reach::Private;
        set_reach_with(&mut broken, false, false).await.expect("without the requirement it is left to the fetch");
    }

    #[test]
    fn a_proxy_is_read_from_the_environment_and_its_host_is_named() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
        };
        assert!(proxy_configured(env(&[("HTTPS_PROXY", "http://proxy.corp:3128")])));
        assert!(!proxy_configured(env(&[("HTTP_PROXY", "  ")])), "an empty value is no proxy");
        assert!(!proxy_configured(env(&[("NO_PROXY", "localhost")])));
        assert_eq!(proxy_hosts(env(&[("HTTP_PROXY", "http://Proxy.Corp:3128")])), ["proxy.corp"]);
        assert_eq!(proxy_hosts(env(&[("ALL_PROXY", "proxy2.corp:3128")])), ["proxy2.corp"], "a bare host and port");
    }

    #[tokio::test]
    async fn public_only_refuses_a_configured_proxy() {
        use clap::Parser;
        let parse = || crate::cli::Args::try_parse_from(["jurl", "http://93.184.216.34/"]).expect("args");
        let mut refused = parse();
        assert!(set_reach_with(&mut refused, true, true).await.is_err(), "a proxy is refused under JURL_PUBLIC_ONLY");
        assert!(refused.public_only);
        let mut allowed = parse();
        set_reach_with(&mut allowed, false, true).await.expect("a proxy is used without the requirement");
        assert!(matches!(allowed.reach, Reach::Public { .. }));
    }

    #[tokio::test]
    async fn the_pre_check_admits_a_public_address_and_refuses_a_name_that_is_not_public() {
        let public = Reach::Public { allowed: HashSet::new() };
        check(&Url::parse("http://93.184.216.34/x").unwrap(), &public).await.expect("a public literal");
        let private = Url::parse("http://10.0.0.1/x").unwrap();
        assert!(format!("{:#}", check(&private, &public).await.unwrap_err()) == "not a public address");
        let named = Url::parse("http://localhost:8080/x").unwrap();
        assert!(format!("{:#}", check(&named, &public).await.unwrap_err()) == "not a public address");
        check(&named, &Reach::Private).await.expect("a private run checks nothing");
    }

    #[tokio::test]
    async fn a_private_start_is_refused_when_public_only_is_required() {
        let private = Url::parse("http://127.0.0.1:8080/").unwrap();
        assert!(decide(&private, true).await.is_err(), "JURL_PUBLIC_ONLY refuses a private start");
        assert_eq!(decide(&private, false).await.expect("a private run"), Reach::Private);
        let public = Url::parse("http://93.184.216.34/").unwrap();
        assert!(matches!(decide(&public, true).await, Ok(Reach::Public { .. })));
    }
}

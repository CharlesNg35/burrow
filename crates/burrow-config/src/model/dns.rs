use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "DNS provider")]
pub struct DnsEntry {
    #[serde(default)]
    #[schemars(
        description = "The domains this provider manages; each covers itself and every name under it, for example [example.com]"
    )]
    #[admin(
        ty = list(host_name),
        help = "The domains whose DNS records this provider can set, for example example.com; it covers every name under it, like wiki.example.com and *.apps.example.com"
    )]
    pub domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Set the records through Cloudflare's API with an API token")]
    #[admin(
        help = "Cloudflare sets the records, with an API token allowed Zone:Read and DNS:Edit on these zones only",
        none_label = "Not used"
    )]
    pub cloudflare: Option<CloudflareDns>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Set the records by dynamic update signed with TSIG (RFC 2136): BIND, Knot, PowerDNS, Technitium"
    )]
    #[admin(
        label = "RFC 2136 (BIND, Knot, PowerDNS)",
        help = "Your own DNS server sets the records by dynamic update signed with a TSIG key",
        none_label = "Not used"
    )]
    pub rfc2136: Option<Rfc2136Dns>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Set the records through an acme-dns server, reached by one CNAME per name"
    )]
    #[admin(
        label = "acme-dns",
        help = "An acme-dns server sets the records; each name's _acme-challenge is a CNAME to it",
        none_label = "Not used"
    )]
    pub acme_dns: Option<AcmeDnsDns>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "A program that sets the records for any provider, called as: present|cleanup _acme-challenge.NAME. VALUE"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        help = "A program that sets the records with any provider, called with present or cleanup, the record's name and its value, for example /usr/local/bin/burrow-dns",
        none_label = "Not used",
        locked
    )]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Shared wildcard certificates to get through this provider, each also covering its parent name, for example [\"*.example.com\"]"
    )]
    #[admin(
        ty = list(host_name),
        label = "Shared wildcards",
        help = "One certificate each for every site one label under it and its parent name, for example *.example.com",
        empty_label = "None"
    )]
    pub wildcards: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Seconds to wait for the zone's nameservers to answer the new record before telling Let's Encrypt; 120 when not set"
    )]
    #[admin(
        label = "Wait for the record",
        help = "How long burrow waits for every nameserver of the zone to answer the new record, for example 120 seconds",
        none_label = "120 seconds",
        advanced
    )]
    pub wait_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Resolvers used only to find the zone's nameservers, for example [1.1.1.1, 9.9.9.9]; the system's when not set"
    )]
    #[admin(
        ty = list(ip),
        help = "Only used to find the zone's public nameservers, which burrow then asks directly, for example 1.1.1.1; they must give the public view",
        empty_label = "The system's",
        advanced
    )]
    pub resolvers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Cloudflare")]
pub struct CloudflareDns {
    #[schemars(
        description = "The API token, from ${CF_TOKEN} or file:path; allowed Zone:Read and DNS:Edit on these zones only"
    )]
    #[admin(
        ty = secret_ref(max = 1, reference),
        label = "API token",
        help = "An API token allowed Zone:Read and DNS:Edit on these zones only, from an environment variable like ${CF_TOKEN} or a file like file:/etc/burrow/cf.token"
    )]
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "RFC 2136")]
pub struct Rfc2136Dns {
    #[schemars(
        description = "The zone's primary nameserver as host:port, for example 192.168.1.2:53; the public nameservers must get the record from it"
    )]
    #[admin(
        help = "The zone's primary as host:port, for example 192.168.1.2:53; it may be on the LAN if it feeds the public nameservers"
    )]
    pub server: String,
    #[schemars(
        description = "The TSIG key's name, as in the server's key file, for example burrow-key"
    )]
    #[admin(help = "The TSIG key's name, as in the server's key file, for example burrow-key")]
    pub key_name: String,
    #[schemars(
        description = "The TSIG secret in base64, as in the server's key file, from ${TSIG_SECRET} or file:path"
    )]
    #[admin(
        ty = secret_ref(max = 1, reference),
        label = "Key secret",
        help = "The TSIG secret in base64, as in the server's key file, from an environment variable like ${TSIG_SECRET} or a file like file:/etc/burrow/tsig.secret"
    )]
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "acme-dns")]
pub struct AcmeDnsDns {
    #[schemars(
        description = "The acme-dns server's address, for example https://auth.example.org"
    )]
    #[admin(
        label = "Server address",
        help = "The acme-dns server's address, for example https://auth.example.org"
    )]
    pub api: String,
    #[schemars(description = "The account's user name, from ${ACMEDNS_USER} or file:path")]
    #[admin(
        ty = secret_ref(max = 1, reference),
        label = "User name",
        help = "The acme-dns account's user name, from an environment variable like ${ACMEDNS_USER} or a file"
    )]
    pub username: String,
    #[schemars(description = "The account's password, from ${ACMEDNS_KEY} or file:path")]
    #[admin(
        ty = secret_ref(max = 1, reference),
        help = "The acme-dns account's password, from an environment variable like ${ACMEDNS_KEY} or a file"
    )]
    pub password: String,
    #[schemars(
        description = "The account's subdomain, for example d420c923-bbd7-4056-ab64-c3ca54c9b3cf"
    )]
    #[admin(
        help = "The account's subdomain, which each name's _acme-challenge CNAME points to, for example d420c923-bbd7-4056-ab64-c3ca54c9b3cf"
    )]
    pub subdomain: String,
}

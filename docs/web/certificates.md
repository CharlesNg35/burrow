# Certificates

In [How it works](how-it-works.md): [step 3](how-it-works.md#3-which-certificate-a-name-gets) picks the certificate, [step 4](how-it-works.md#4-lets-encrypt-checks-and-renewal) gets it from Let's Encrypt.

For each name of an HTTPS site, burrow takes the first that applies:

1. the site's own `tls: {cert, key}`;
2. an entry in `web.certificates` whose `hosts` lists the name (a listed wildcard covers the names one label below it);
3. `web.letsencrypt`.

If none applies, validate refuses the name and says how to fix it.

### Your own files

```yaml
# server.yaml
web:
  https: 443
  certificates:
    - hosts: ["*.example.com", example.com]
      cert: certs/example.com/fullchain.pem
      key: certs/example.com/privkey.pem
    - hosts: [nas.home.arpa] # an internal name, from your own CA
      cert: certs/home.pem
      key: certs/home.key
```

- Paths are relative to the config file. A file is used only for the names its `hosts` lists, and each name may be listed in one entry only.
- `burrow validate` (and every start and reload) reads the files: it refuses one it cannot read, one that is not PEM, a key that does not fit its certificate, and a certificate that does not cover a name it is used for.
- A file that changes on disk, for example after `certbot renew`, is used from the next reload; a file that breaks keeps the last good certificate in use.
- burrow warns 14 days before a file expires.

### Let's Encrypt

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
    # directory: staging          # Let's Encrypt's test service, for trying things out
    # profile: shortlived         # 6-day certificates
    # directory: https://ca.internal/acme/directory  # another ACME server
    # ca: certs/internal-ca.pem   # the CA to trust for that server
```

- **Writing `letsencrypt:` with an email accepts Let's Encrypt's terms** (<https://letsencrypt.org/repository/>). Let's Encrypt stopped sending expiry emails in June 2025; burrow shows the state of every certificate in `burrow status`, the admin UI and its metrics instead.
- **How Let's Encrypt checks a name:** over TLS on port 443 when `web.https` is 443, else over HTTP on port 80 when `web.http` is 80. Each name needs one of them reachable from the internet, also from several places, so a firewall that allows only some countries or addresses breaks the check. A site's `challenge: tls`, `challenge: http` or `challenge: dns` picks the check (`websocket.challenge` for the tunnel's name); validate refuses a check whose port is not there, `http` or `tls` for a wildcard, and `dns` for an IP address.
- **Wildcards** need a DNS check (below), or your own file.
- **Many sites under one domain** are best served by one wildcard certificate, like `*.example.com`: from Let's Encrypt as a shared wildcard on its DNS provider (below), or as your own file in `web.certificates`.
- **IP addresses** get Let's Encrypt's short-lived certificates (about 6 days), renewed after half their life.
- **`profile`** applies to certificates for DNS names; IP addresses always get the short-lived profile (or none if the CA doesn't list it). burrow checks the profile against the list the CA offers before ordering; one it does not offer is shown in `burrow status` and the admin UI with the list of the offered ones.
- **Renewal** happens by itself, at the time Let's Encrypt suggests (ARI), else after two thirds of a certificate's life. A failed try is retried after 1 minute, 10 minutes, 100 minutes, then daily, and the old certificate keeps being served, even after it expires.
- **No duplicates:** certificates are kept by their names. A restart, a renamed service or a name moved to another service orders nothing new, and at most 4 new certificates per set of names are ordered in 7 days.
- **Where they are kept:** a private folder that `burrow status` names. An installed service keeps it in `/var/lib/burrow/<name>` on Linux, `/Library/Application Support/burrow/<name>` on macOS, `%ProgramData%\burrow\services\<name>` on Windows; `burrow service uninstall` keeps it and only `--purge` deletes it. A server run by hand uses one folder per config file; `burrow server --state-dir DIR` picks another.
- **The account** is shown in `burrow status` and `burrow validate` (once the first start has created it), for a CAA DNS record with `accounturi` that lets only this account get certificates for your domain.

### DNS checks: wildcards and names the internet cannot reach

With a DNS check, Let's Encrypt reads a TXT record that burrow sets at `_acme-challenge.<name>`. No port has to be reachable from the internet, so it suits wildcards (Let's Encrypt gives them only this way), names inside your LAN, and servers whose ports 80 and 443 are closed. `letsencrypt.dns` lists how burrow sets records, each provider with the domains it manages:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
    dns:
      - domains: [example.com, example.net] # these domains and every name under them
        cloudflare: { token: "${CF_TOKEN}" } # quoted: ${...} inside { } must be a quoted string in YAML
        wildcards: ["*.example.com"] # one shared certificate: example.com and *.example.com
      - domains: [corp.example.org]
        rfc2136: { server: "192.168.1.2:53", key_name: burrow-key, secret: "${TSIG_SECRET}" }
services:
  wiki:
    client: home
    target: 3001
    web: wiki.example.com # covered by *.example.com: orders nothing of its own
  erp:
    client: office
    target: 8443
    web:
      hosts: [erp.corp.example.org]
      challenge: dns # only reachable inside the LAN
```

- **Which entry a name uses:** the one whose domain covers it most specifically, so `corp.example.org` wins over `example.org`. A domain covers itself and every name under it; write `example.com`, not `*.example.com`. Each domain sits in one entry only.
- **Which names use DNS:** wildcards always, and a site with `challenge: dns` (the tunnel's own name with `websocket.challenge: dns`). Other names keep the TLS or HTTP check.
- **The record must be public.** The name may be internal, but Let's Encrypt reads the TXT record from the internet, so the zone's public nameservers must answer it. An `rfc2136` server on your LAN works as the zone's hidden primary that feeds the public nameservers; a split-horizon server that answers only inside the LAN cannot work.
- **burrow waits for every nameserver.** Before it tells Let's Encrypt to look, burrow asks each of the zone's public nameservers directly until all of them answer the record, for up to `wait_secs` (120 when not set). `resolvers` (like `[1.1.1.1, 9.9.9.9]`) are used only to find those nameservers, and must give the public view; without it, the system's resolvers are used.
- **CNAMEs are followed.** If `_acme-challenge.<name>` is a CNAME, burrow writes the record at its target (up to 8 steps), as Let's Encrypt reads it there. The entry is still chosen by the name's own domain: to delegate, list the name's domain on the entry that can write the target.
- **Records are removed** after each order, also when it fails.
- **Secrets** (`token`, `secret`, `password`) come only from `${VAR}` or `file:path` (relative to the config file); validate refuses one written in the file. They are never printed: `burrow status`, the logs and the admin UI show only that they are set.

#### Shared wildcards

Many sites under one domain are best served by one wildcard certificate: one order and one renewal instead of one per site. List it in `wildcards` on the entry that manages its domain, like `wildcards: ["*.example.com"]` above.

- Each wildcard is one certificate that also covers its parent name (`*.example.com` with `example.com`).
- It is ordered when the config is saved, kept, and renewed by itself. Every site one label under it reuses it, also one added later; names two labels down get their own.
- Removing it from the list stops its renewals.
- burrow never makes a wildcard by itself. When three or more sites under one DNS-covered domain would each get their own certificate, validate suggests one.

#### Cloudflare

Create an API token at <https://dash.cloudflare.com/profile/api-tokens> with **Zone: Read** and **DNS: Edit**, limited to the zones you list (Zone Resources: Include, Specific zone). Give it to burrow as `${CF_TOKEN}` from the service's environment, or `file:/etc/burrow/cf.token`.

#### RFC 2136: BIND, Knot, PowerDNS, Technitium

burrow sends a dynamic update signed with a TSIG key to `server`, the zone's primary, and finds the zone itself. Make a key, and allow it to change only the `_acme-challenge` TXT records:

```text
# BIND: tsig-keygen -a hmac-sha256 burrow-key > /etc/bind/burrow-key.conf, then in the zone:
zone "example.org" {
    type primary;
    file "/var/lib/bind/example.org.db";
    update-policy { grant burrow-key name _acme-challenge.erp.corp.example.org. TXT; };
};
```

- **Knot DNS:** a `key` with `algorithm: hmac-sha256`, and an `acl` with `action: update`, `update-type: [TXT]` and `update-owner: name` for the `_acme-challenge` names.
- **PowerDNS:** set `dnsupdate=yes` in `pdns.conf`, make the key with `pdnsutil tsigkey generate burrow-key hmac-sha256` (before 5.0: `pdnsutil generate-tsig-key`), and allow it on the zone with `pdnsutil metadata add example.org TSIG-ALLOW-DNSUPDATE burrow-key`. Limiting the key to `_acme-challenge` names needs PowerDNS's Lua update policy.
- **Technitium:** allow dynamic updates for the zone with a TSIG key in the zone's options, as Technitium's documentation describes, limited to the `_acme-challenge` TXT records where it offers that.

`secret` is the key's base64 secret, as in the key file. `algorithm` is the key's algorithm: `hmac-sha256` (the default), `hmac-sha384` or `hmac-sha512`; it must be the one in the server's key file, or the server answers BADKEY. `hmac-md5` and `hmac-sha1` are refused, as RFC 8945 advises against them. A secret shorter than the algorithm's output (32, 48 or 64 bytes) is a warning; `tsig-keygen -a` makes one of the right length.

#### acme-dns

[acme-dns](https://github.com/acme-dns/acme-dns) serves only ACME records, so your main DNS provider needs no API at all: point each name's `_acme-challenge` CNAME at the acme-dns server once, and burrow updates the record there.

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: ops@example.com
    dns:
      - domains: [example.io]
        acme_dns:
          api: https://auth.example.org
          username: ${ACMEDNS_USER}
          password: ${ACMEDNS_KEY}
          subdomain: d420c923-bbd7-4056-ab64-c3ca54c9b3cf
```

`api` is an `https://` address. An acme-dns server on your own network may use plain `http://`, like `http://192.168.1.5:8080`, when its address is loopback or private (10/8, 172.16/12, 192.168/16, fc00::/7): validate refuses `http://` to a public address, and for a name, burrow looks it up each time and refuses it when any of its addresses is public, then connects to the address it checked.

#### A command for any provider

`command` runs a program of yours, like lego's `exec` provider: `PROGRAM present|cleanup NAME VALUE`, where `NAME` is the record's name with a final dot (the `_acme-challenge` name, or its CNAME target). It gets no secrets from burrow and an empty environment apart from `PATH`, so it reads its own credentials from a file. It must finish within 60 seconds, and it must add the value, not replace the record: a wildcard and its parent name set two values side by side. Installed as a service, it runs as the service account under the service's limits: on Linux it cannot see /home, can write only burrow's own folders, and cannot run programs that compile code at run time (`MemoryDenyWriteExecute`, so a Node.js hook fails); keep it and its credentials file outside /home, readable by that account.

```sh
#!/bin/sh
# /usr/local/bin/burrow-dns: $1 = present|cleanup, $2 = _acme-challenge.NAME. , $3 = value
case "$1" in
  present) nsupdate -k /etc/burrow/tsig.key <<EOF
update add $2 60 TXT "$3"
send
EOF
  ;;
  cleanup) nsupdate -k /etc/burrow/tsig.key <<EOF
update delete $2 TXT "$3"
send
EOF
  ;;
esac
```

Scripts written for lego's exec provider work unchanged when they read their credentials themselves.

### When a certificate fails

`burrow status` lists every certificate with where it comes from, its state, when it expires and when it is renewed; a failing one says why. The admin UI shows the same on the Overview and lists problems under Needs attention. Check, in order:

1. **DNS:** the name points at this server (`dig +short blog.example.com`), for IPv6 too if you have an `AAAA` record.
2. **Ports:** port 443 (or 80) reaches burrow from the internet, not only from your network. A router must forward it.
3. **Firewalls:** nothing blocks Let's Encrypt's checks from abroad.
4. **Limits:** after many failures Let's Encrypt slows down; burrow follows its schedule. `directory: staging` tries everything against the test service without counting against your limits.

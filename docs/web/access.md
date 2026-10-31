# Who may visit

A website at home is often meant for you, your family or your team, not the whole internet. burrow can check visitors before a request reaches the app: by address, by rate, with a password, through a single sign-on service like Authelia or authentik, or with a certificate on each device. The app needs no change for any of them.

In [How it works](how-it-works.md): device certificates are checked in the handshake, in [step 1](how-it-works.md#1-a-connection-arrives-on-the-https-port), and the other checks run for each request, in this order, in [step 2](how-it-works.md#2-a-request-is-answered).

Each key below lets in a kind of visitor, and says what the others get:

| Key | Who gets in | Otherwise |
| --- | --- | --- |
| `allow_from`, `deny_from` (the service's) | visitors from these addresses | `403` |
| `rate_limit` | up to so many requests per second per address | `429` |
| `basic_auth` | visitors with a user name and password | `401` |
| `forward_auth` | visitors your login service lets in, like Authelia or authentik | the login service's answer, usually its login page |
| `client_auth` | devices with a certificate from your CA | the TLS handshake fails |

A site takes one kind of login: `basic_auth` or `forward_auth`, never both. Every check needs burrow to do the HTTPS (not `tls: passthrough`), and `client_auth` also needs HTTPS (not `tls: off`).

`burrow validate` shows each site's checks after its service, like `-> service private (password for alice)`, `(login through service authelia)`, `(device certificate)` or `(allow_from 192.168.1.0/24)`.

The logins guard the website only. A service that also has its own `bind` port is reached there without them. So `burrow validate` warns when a site with a login has a public `bind` and no `allow_from`, and its summary line names that port. Remove `bind`, or limit it with `allow_from`, which applies to both. Visitors with `burrow visit` still need their own burrow login.

## Addresses

A service's `allow_from` and `deny_from` ([Who may connect](../services.md#who-may-connect)) apply to its website too, before any password, so a refused address costs no work. Behind a proxy in `web.trusted_proxies`, the address is the visitor's own, from `X-Forwarded-For` ([Behind Cloudflare or a load balancer](proxies.md#behind-cloudflare-or-a-load-balancer)). Let's Encrypt's checks come first, so a site open only to your network still gets its certificate.

## Rate limits

A rate limit caps how many requests one address may send, which slows down scrapers and password guessing. This config sets a default for every site and a tighter limit on the shop:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
  rate_limit: 20 # every site: 20 requests a second per visitor address
services:
  shop:
    client: home
    target: 8080
    web:
      hosts: [shop.example.com]
      rate_limit: 5 # this site: 5 a second
```

- `rate_limit` is requests per second per visitor address, with bursts of twice that. Above it, the visitor gets `429` with `Retry-After`. A page with many images asks for all of them at once, so leave room.
- IPv6 addresses are counted per /64, the block a single home or phone usually gets, so a visitor cannot get past the limit by changing addresses within it.
- Set it under `web:` for every site, or per site; the site's own value wins.
- A reload keeps the counts of a site whose `rate_limit` did not change.

## Passwords

The simplest login: the browser asks for a user name and password before it shows the site. This config gives one user access to a file site:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
services:
  files:
    client: home
    target: 8080
    web:
      hosts: [files.example.com]
      basic_auth: # user: hash, from burrow passwd USER
        alice: "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$Q0gCQmVHjy2MKCgr4tvPPMkJEY3gvb2vmT6oGmP4ZgM"
```

The app never sees the password: burrow removes the `Authorization` header.

- **In the admin UI**, a site's **Passwords** list takes a user name and a password. The server hashes it the way `burrow passwd` does and writes only the hash; nobody can read a password back, and existing users show as set. **Paste a hash instead** takes one from `burrow passwd` or `htpasswd -B`.
- **Make a hash** with `burrow passwd alice`: it asks for the password twice without showing it and prints the line to paste, `alice: $argon2id$...`. Piped in, it reads one line, for scripts.
- **Hashes** are argon2id, or bcrypt (`$2a$`, `$2b$`, `$2y$`) from tools like `htpasswd -B` or `caddy hash-password`. Validate refuses other kinds. It also refuses a hash that asks for more work than burrow allows, which would let one visitor keep the server busy: argon2 up to 64 MiB, 10 passes and 4 lanes; bcrypt cost 4 to 14. bcrypt reads only the first 72 bytes of a password.
- **Wrong passwords:** each visitor address (IPv6 per /64) may get the password wrong 10 times a minute, on all sites together. After that, even the right password gets `429` with `Retry-After` until the budget refills, a few seconds per attempt. A request with no password, or an unreadable one, does not count: a browser asks once without one before it shows the login box.
- **Busy:** checking a password takes real work on purpose. At most 2 checks run at once; a request that waits more than a second for its turn gets `503`. A right password is remembered for 10 minutes, so a page with many requests is checked once.
- **Over plain HTTP** (`tls: off`) passwords travel unencrypted; validate warns.

## Single sign-on

`forward_auth` asks a login service, like [Authelia](https://www.authelia.com/integration/proxies/caddy/) or [authentik](https://docs.goauthentik.io/add-secure-apps/providers/proxy/server_caddy/), before each request. If it says yes, the request goes on to the app with the headers it names. If not, its answer goes back to the visitor, usually a redirect to its login page. One login then covers every site, with two-factor sign-in if the login service offers it.

This config puts a wiki behind Authelia, which runs as a site of its own:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
  forward_auth: # login services, by name; a site picks one with forward_auth: <name>
    authelia:
      service: authelia
      path: /api/authz/forward-auth
      copy_headers: [Remote-User, Remote-Groups, Remote-Email, Remote-Name]
services:
  authelia: # the login portal itself, never behind a login
    client: home
    target: 9091
    web: auth.example.com
  wiki:
    client: home
    target: 8080
    web:
      hosts: [wiki.example.com]
      forward_auth: authelia
```

In Authelia, set `session.cookies` with your `domain` and `authelia_url: https://auth.example.com`, as its Caddy guide shows. For authentik, see [Single sign-on with authentik](recipes.md#single-sign-on-with-authentik).

- Each login service under `web.forward_auth` has a name, and a site picks one with `forward_auth: <name>`. With exactly one defined, `forward_auth: true` means that one; with two or more, validate asks you to name it.
- Several login services can live side by side: Authelia's `/api/authz/basic` endpoint for apps that send a password, two oauth2-proxy instances, or Authelia next to authentik while you move.

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
  forward_auth:
    authelia:
      service: authelia
      path: /api/authz/forward-auth
      copy_headers: [Remote-User, Remote-Groups]
    authelia-basic: # the same Authelia, an endpoint named basic for apps that send a password
      service: authelia
      path: /api/authz/basic
      copy_headers: [Remote-User, Remote-Groups]
services:
  authelia: { client: home, target: 9091, web: auth.example.com }
  wiki:
    client: home
    target: 8080
    web: { hosts: [wiki.example.com], forward_auth: authelia }
  dav:
    client: home
    target: 5232
    web: { hosts: [dav.example.com], forward_auth: authelia-basic }
```

Authelia answers on `/api/authz/<name>` for each endpoint under its [`server.endpoints.authz`](https://www.authelia.com/configuration/miscellaneous/server-endpoints-authz/); `basic` here is one you add there with the `HeaderAuthorization` strategy and the `Basic` scheme.

- `service` is a service of this config, reached through the tunnel like any other. The site of a login service must stay open: validate refuses `forward_auth` on it, whichever login service it names.
- `path` is where the login service answers the check: `/api/authz/forward-auth` for Authelia, `/outpost.goauthentik.io/auth/caddy` for authentik.
- burrow asks with a `GET` and no body. It removes the visitor's own `X-Forwarded-Method`, `X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Uri` and `X-Forwarded-For`, then sets them from the request as it was routed.
- `copy_headers` are copied from a yes to the app. A visitor's own headers of those names, also written with `_` for `-`, are always removed, so nobody can claim to be `Remote-User: admin`. Every site with `forward_auth` removes the `copy_headers` of all your login services, not only its own. So an app that still trusts `X-authentik-username` cannot be fooled while you move from authentik to Authelia.
- validate warns about a login service that no site uses.
- It never fails open: a login service that does not answer within `response_header_timeout_secs` gives `504`, and one that is down gives `502` or `503`.
- burrow asks over HTTP/1.1. A login service set to HTTP/2 only (`target_http2`) would fail every check; validate warns.
- Over plain HTTP (`tls: off`) the login cookies travel unencrypted; validate warns.

## Device certificates

With `client_auth`, only devices with a certificate signed by your CA get in. The check happens in the TLS handshake, before any request, so a device without one never sees the site. It suits internet-facing sites that only you and your family use, like a NAS or Home Assistant. This config lets only your devices reach the NAS:

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
services:
  nas:
    client: home
    target: 5000
    web:
      hosts: [nas.example.com]
      client_auth:
        ca: certs/devices-ca.pem # your device CA, with its intermediates
        crl: certs/devices.crl # optional: devices you revoked
```

- `ca` holds the CA that signs your devices' certificates, and its intermediates. Validate refuses a file it cannot read or that holds no CA certificate.
- `crl` holds the CA's revocation lists, PEM or DER: a device listed there is refused. burrow reads the file again when it changes. If the file breaks, the last good list stays, with a warning in `burrow status`. A list past its next-update date still applies, with a warning, so a late publish never locks every device out. Validate refuses a list another CA signed, and an old version 1 list.
- Every name of the site asks for a certificate. A name under a wildcard takes the wildcard's `client_auth` only when it has no site of its own. Each request must ask for the name of its connection (`421` otherwise), so a visitor cannot open an unprotected name and then ask for a protected one on the same connection.
- `client_auth` is refused on the tunnel's name: burrow clients have no device certificate.
- A site with `client_auth` is never served over plain HTTP. On `web.http` its names always get the redirect to `https://` (or the page that says the certificate is on its way), whatever `https_redirect` says, and also when a trusted proxy sends `X-Forwarded-Proto: https`: burrow cannot see a device certificate that was checked somewhere else. Validate refuses `client_auth` together with `https_redirect: false`, on the site or for all sites.
- A revoked device's connections that are already open stay open until they close.

### Your own device CA

Device certificates need a small CA of your own, which signs one certificate per device and can revoke a lost one. Two ways to run it follow.

With [step-ca](https://smallstep.com/docs/step-ca/), turn on its CRL in `ca.json` with `"crl": {"enabled": true, "generateOnRevoke": true}`. Give each device a certificate, and point `ca` at a file with step-ca's intermediate and root certificates. Without `generateOnRevoke`, step-ca writes a new list only every 16 hours or so, so a lost phone would stay admitted until then ([its configuration docs](https://smallstep.com/docs/step-ca/configuration/): `generateOnRevoke` is off by default, and the list renews after about 2/3 of `cacheDuration`, 24 h). step-ca serves the list at `/1.0/crl`. Fetch it into the file after a revoke, and now and then, for example from cron:

```bash
step ca revoke 2832... # the serial of the lost phone's certificate
curl -fsS --cacert root_ca.crt -o certs/devices.crl.new https://ca.example.com/1.0/crl && mv certs/devices.crl.new certs/devices.crl
```

With OpenSSL alone, keep the CA in its own folder, away from burrow's config:

```bash
cd devices-ca
cat > ca.cnf <<'EOF'
[ca]
default_ca = devices
[devices]
database = index.txt
crlnumber = crlnumber
certificate = ca.pem
private_key = ca.key
default_md = sha256
default_crl_days = 30
EOF
touch index.txt && echo 1000 > crlnumber
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -noenc -keyout ca.key -out ca.pem -days 3650 -subj "/CN=Home devices"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -noenc -keyout phone.key -out phone.csr -subj "/CN=phone"
openssl x509 -req -in phone.csr -CA ca.pem -CAkey ca.key -days 825 -extfile <(echo extendedKeyUsage=clientAuth) -out phone.pem
openssl pkcs12 -export -inkey phone.key -in phone.pem -certfile ca.pem -out phone.p12 # import this on the phone
openssl ca -config ca.cnf -gencrl -out devices.crl # an empty list, to start with
```

Copy `ca.pem` to `certs/devices-ca.pem` and `devices.crl` to `certs/devices.crl` next to the config. To revoke a device, then publish the new list:

```bash
openssl ca -config ca.cnf -revoke phone.pem
openssl ca -config ca.cnf -gencrl -out devices.crl && cp devices.crl /etc/burrow/certs/devices.crl
```

- The `crlnumber` file makes a version 2 list, which burrow needs.
- Make a new list before `default_crl_days` run out, even with nothing to revoke, or `burrow status` warns that it is past its date.
- Some phones refuse a `.p12` from OpenSSL 3; `openssl pkcs12 -export -legacy` writes the older form they read.

## Next steps

- [Behind a proxy](proxies.md): get the visitor's real address behind Cloudflare, so `allow_from` and rate limits see it.
- [Single sign-on with authentik](recipes.md#single-sign-on-with-authentik).
- [Security](../security.md#websites): how these checks are kept from being walked around.

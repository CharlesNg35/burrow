# Who may visit

In [How it works](how-it-works.md): device certificates are checked in the handshake, in [step 1](how-it-works.md#1-a-connection-arrives-on-the-https-port), and the other checks run for each request, in this order, in [step 2](how-it-works.md#2-a-request-is-answered).

| Key | Who gets in | Otherwise |
| --- | --- | --- |
| `allow_from`, `deny_from` (the service's) | visitors from these addresses | `403` |
| `rate_limit` | up to so many requests per second per address | `429` |
| `basic_auth` | visitors with a user name and password | `401` |
| `forward_auth` | visitors your login service lets in, like Authelia or authentik | the login service's answer, usually its login page |
| `client_auth` | devices with a certificate from your CA | the TLS handshake fails |

A site takes one kind of login: `basic_auth` or `forward_auth`, never both. All of them need burrow to do HTTPS (not `tls: passthrough`), and `client_auth` needs HTTPS (not `tls: off`).

The logins guard the website only. A service that also has its own `bind` port is reached there without them, so `burrow validate` warns when a site with a login has a public `bind` and no `allow_from`, and its summary line names that port. Remove `bind`, or limit it with `allow_from`, which applies to both. Visitors with `burrow visit` still need their own burrow login.

## Addresses

A service's `allow_from` and `deny_from` ([Who may connect](../services.md#who-may-connect)) apply to its website too, before any password, so a refused address costs no work. Behind a proxy in `web.trusted_proxies`, the address is the visitor's own, from `X-Forwarded-For` ([Behind Cloudflare or a load balancer](proxies.md#behind-cloudflare-or-a-load-balancer)). Let's Encrypt's checks are answered before this, so a site open only to your network still gets its certificate.

## Rate limits

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

- `rate_limit` is requests per second per visitor address, with bursts of twice that; above it the visitor gets `429` with `Retry-After`. A page with many images needs a burst of all of them at once, so leave room.
- IPv6 addresses are counted per /64, the block a single home or phone usually gets, so a visitor cannot get past the limit by changing addresses within it.
- Set it under `web:` for every site, or per site; the site's own value wins.
- A reload keeps the counts of a site whose `rate_limit` did not change.

## Passwords

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

The browser asks for a user name and password; the app never sees them, as burrow removes the `Authorization` header.

- **In the admin UI**, a site's **Passwords** list takes a user name and a password. The server hashes it the way `burrow passwd` does and writes only the hash; nobody can read a password back, and existing users show as set. **Paste a hash instead** takes one from `burrow passwd` or `htpasswd -B`.
- **Make a hash** with `burrow passwd alice`: it asks for the password twice without showing it and prints the line to paste, `alice: $argon2id$...`. Piped in, it reads one line, for scripts.
- **Hashes** are argon2id, or bcrypt (`$2a$`, `$2b$`, `$2y$`) from tools like `htpasswd -B` or `caddy hash-password`. Validate refuses other kinds, and a hash that asks for more work than burrow allows (argon2 up to 64 MiB, 10 passes and 4 lanes; bcrypt cost 4 to 14), which would let one visitor keep the server busy. bcrypt reads only the first 72 bytes of a password.
- **Wrong passwords:** each visitor address (IPv6 per /64) may get the password wrong 10 times a minute, on all sites together. After that, even the right password gets `429` with `Retry-After` until the budget refills, a few seconds per attempt. A request without a password, or with an unreadable one, does not count: a browser asks once without one before it shows the login box.
- **Busy:** checking a password takes real work on purpose. At most 2 checks run at once; a request that waits more than a second for its turn gets `503`. A right password is remembered for 10 minutes, so a page with many requests is checked once.
- **Over plain HTTP** (`tls: off`) passwords travel unencrypted; validate warns.

## Single sign-on

`forward_auth` asks a login service, like [Authelia](https://www.authelia.com/integration/proxies/caddy/) or [authentik](https://docs.goauthentik.io/add-secure-apps/providers/proxy/server_caddy/), before each request. If it says yes, the request goes on to the app with the headers it names; if not, its answer, usually a redirect to its login page, goes back to the visitor.

```yaml
# server.yaml
web:
  https: 443
  letsencrypt:
    email: you@example.com
  forward_auth: # the login service of every site with forward_auth: true
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
      forward_auth: true
```

In Authelia, set `session.cookies` with your `domain` and `authelia_url: https://auth.example.com`, as its Caddy guide shows. For authentik, see [Single sign-on with authentik](recipes.md#single-sign-on-with-authentik).

- `forward_auth: true` uses `web.forward_auth`; a site can name its own `{service, path, copy_headers}` instead.
- `service` is a service of this config, reached through the tunnel like any other; the login site itself must stay open (validate refuses a site that checks its own visitors).
- `path` is where the login service answers the check: `/api/authz/forward-auth` for Authelia, `/outpost.goauthentik.io/auth/caddy` for authentik.
- burrow asks with a `GET` and no body, sets `X-Forwarded-Method`, `X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Uri` and `X-Forwarded-For` from the request as it was routed, and removes the visitor's own copies of those first.
- `copy_headers` are copied from a yes to the app. A visitor's own headers of those names, also written with `_` for `-`, are always removed, so nobody can claim to be `Remote-User: admin`.
- It never fails open: a login service that does not answer within `response_header_timeout_secs` gives `504`, one that is down `502` or `503`.
- burrow asks over HTTP/1.1. A login service set to HTTP/2 only (`target_http2`) would fail every check; validate warns.
- Over plain HTTP (`tls: off`) the login cookies travel unencrypted; validate warns.

## Device certificates

With `client_auth`, only devices with a certificate signed by your CA get in. The check happens in the TLS handshake, before any request, so a device without one never sees the site. It suits a few sites only you and your family use, like a NAS or Home Assistant, opened to the internet.

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
- `crl` holds the CA's revocation lists, PEM or DER: a device listed there is refused. burrow reads the file again when it changes, and a file that breaks keeps the last good list, with a warning in `burrow status`. A list past its next-update date still applies, with a warning, so a late publish never locks every device out. Validate refuses a list another CA signed and an old version 1 list.
- Every name of the site asks for a certificate. A name under a wildcard takes the wildcard's `client_auth` only when it has no site of its own. Each request must ask for the name of its connection (`421` otherwise), so a visitor cannot open a free name and then ask a protected one on the same connection.
- `client_auth` is refused on the tunnel's name: burrow clients have no device certificate.
- A revoked device's connections that are already open stay open until they close.

### Your own device CA

With [step-ca](https://smallstep.com/docs/step-ca/), turn on its CRL in `ca.json` with `"crl": {"enabled": true, "generateOnRevoke": true}`, give each device a certificate, and point `ca` at a file with step-ca's intermediate and root certificates. Without `generateOnRevoke`, step-ca writes a new list only every 16 hours or so ([its configuration docs](https://smallstep.com/docs/step-ca/configuration/): `generateOnRevoke` is off by default, and the list renews after about 2/3 of `cacheDuration`, 24 h), so a lost phone would stay admitted until then. step-ca serves the list at `/1.0/crl`; fetch it into the file after a revoke, and now and then, for example from cron:

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

# Paths and redirects

In [How it works](how-it-works.md): routes are picked in [step 2](how-it-works.md#2-a-request-is-answered), and on port 80 in [step 6](how-it-works.md#6-port-80).

## Paths: several apps on one name

One name can be split between services by `paths`. The longest matching path wins, and a path covers itself and everything below it: `/api` matches `/api` and `/api/users`, never `/apix`. A service without `paths` takes everything the others leave.

```yaml
# server.yaml
web:
  https: 443
  http: 80
  letsencrypt:
    email: ops@example.com
services:
  site: # the website runs at home
    client: home
    target: 8080
    web: example.com # everything on example.com not claimed below
  api: # the API runs in the office
    client: office
    target: 9000
    web:
      hosts: [example.com]
      paths: [/api]
      app_path: / # the API app has no /api prefix of its own
```

`example.com/` goes to the home client's port 8080, and `example.com/api/users` to the office client's port 9000 as `/users`. One certificate covers `example.com` for both. If the office client is down, `/api` answers 503 while the rest of the site works.

- **`app_path`** is what the app sees in place of the matched path: `paths: [/api]` with `app_path: /v1` sends `/api/users` as `/v1/users`, and `app_path: /` removes the prefix. When the path changes, the app gets `X-Forwarded-Prefix: /api`, its public prefix; an app that builds absolute links must read it, or be told its public address, as behind any proxy that rewrites paths.
- **Routing is not a redirect.** Routing forwards the request and the address bar stays the same; a redirect sends the browser to another address. `paths` and `app_path` route; `redirect_from`, `redirects` and `web.redirect_names` redirect.
- **Two services may share a name only by different paths**, and they must choose the same `tls`.

### How burrow reads a path

burrow cleans each request's path once, before it picks a route, and sends the clean path to the app. Paths are matched like nginx, and forwarded cleaned like Traefik:

- repeated slashes are merged: `//admin` is `/admin`;
- escapes of plain characters are decoded and the others written in capitals: `/%61pi` is `/api`, and `%2f` is `%2F`;
- a `.` or `..` segment is refused with `400`, also when it is encoded (`%2E%2E`) or hides behind an encoded slash (`/public%2F..%2Fadmin`, `/a%5C..%5Cb`);
- an encoded slash on its own is data and passes unchanged, like GitLab's `/api/v4/projects/group%2Fproject`.
- letters match whatever their case: `/API/users` takes the route `/api`, and the app gets `/API/users` as it was sent. So two paths that differ only by case, like `/api` and `/API`, are one path, and validate refuses them.

So a password on `/wp-admin` cannot be walked around with `//wp-admin` or `/%77p-admin`. The paths you write in the config are cleaned the same way.

## Redirects

```yaml
# server.yaml
web:
  https: 443
  http: 80 # without it, http:// gets no answer at all, not even a redirect
  letsencrypt:
    email: ops@example.com
  redirect_names: # whole names with no site of their own
    old.example.com: "https://example.com" # path and query kept: /a?b -> https://example.com/a?b
    example.com/shop: { to: "https://shop.example.net/", permanent: true } # /shop/cart -> https://shop.example.net/cart
services:
  blog:
    client: home
    target: 8080
    web:
      hosts: [example.com]
      redirect_from: [www.example.com] # permanent (308) to example.com, path and query kept
      redirects:
        /old: /new # temporary (307): /old/page -> /new/page
        /feed: { to: "https://feeds.example.net/blog", permanent: true }
```

- `redirect_from` sends other names to the site's first name, for good (308).
- `redirects` sends paths of the site elsewhere: temporary (307) unless `permanent: true` (308). The rest of the path and the query are kept, and they are matched on the public path, before `app_path`. Browsers remember a permanent redirect, so use it only when you are sure.
- `web.redirect_names` is for names with no site of their own, or one path of a name another service serves (`example.com/shop` above): a longer path is a route of its own.
- A target is a full address (`https://...`) or a path on the same name (`/new`).
- Names that only redirect still get a certificate, the same way as a site's names.

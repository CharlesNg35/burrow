use super::{SECRET, SECRET2, render};

fn self_server(self_section: &str, services: &str, visitors: &str) -> String {
    format!(
        "listen: 0.0.0.0:7835\nmetrics:\n  listen: 127.0.0.1:9100\n{self_section}clients:\n  home:\n    secret: {SECRET}\n{visitors}services:\n{services}"
    )
}

#[test]
fn self_services_and_reach_are_valid() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [127.0.0.1/32, 169.254.169.254/32:80, \"unix:/run/app.sock\"]\n",
            "  db: { client: self, target: 127.0.0.1:5432, visitors: [laptop] }\n  web: { clients: [self, home], bind: 8080, target: 127.0.0.1:3000 }\n  meta: { client: self, bind: 8081, target: 169.254.169.254:80 }\n  app: { client: self, bind: 8082, target: \"unix:/run/app.sock\" }\n  game: { client: self, protocol: udp, bind: 30000-30001, target: 127.0.0.1:40000-40001 }\n",
            &format!(
                "visitors:\n  laptop:\n    secret: {SECRET2}\n    reach: {{ self: [127.0.0.1/32:22] }}\n"
            ),
        ),
        None,
    );
    insta::assert_snapshot!(text, @"server.yaml: valid server config (1 client, 1 visitor, 5 services)");
}

#[test]
fn a_self_service_without_self_allow_reaches_its_own_target() {
    let text = render(
        "server.yaml",
        &self_server(
            "",
            "  web: { client: self, bind: 8080, target: localhost:3000 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @"server.yaml: valid server config (1 client, 1 service)");
}

#[test]
fn a_wildcard_listen_port_on_another_machine_is_not_burrow() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [192.0.2.0/24, 127.0.0.1/32]\n",
            "  chain: { client: self, bind: 8092, target: 192.0.2.9:7835 }\n  peer: { client: self, bind: 8093, target: 192.0.2.9:8093 }\n  loop: { client: self, bind: 8094, target: 127.0.0.1:7835 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: service "loop" on "self" targets 127.0.0.1:7835, which is burrow's own listen (0.0.0.0:7835)
      --> server.yaml:12:45
       |
    12 |   loop: { client: self, bind: 8094, target: 127.0.0.1:7835 }
       |                                             ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn udp_self_targets_are_burrows_own_only_on_its_udp_binds() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [127.0.0.1/32]\n",
            "  wg: { client: self, protocol: udp, bind: 8100, target: 127.0.0.1:7835 }\n  stats: { client: self, protocol: udp, bind: 8101, target: 127.0.0.1:9100 }\n  echo: { client: self, protocol: udp, bind: 8102, target: 127.0.0.1:8102 }\n  quic: { client: self, protocol: udp, bind: 7835, target: 127.0.0.1:7835 }\n  loop: { client: self, bind: 8103, target: 127.0.0.1:7835 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: service "echo" on "self" targets 127.0.0.1:8102, which is burrow's own bind of this service (0.0.0.0:8102)
      --> server.yaml:12:60
       |
    12 |   echo: { client: self, protocol: udp, bind: 8102, target: 127.0.0.1:8102 }
       |                                                            ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "quic" on "self" targets 127.0.0.1:7835, which is burrow's own bind of this service (0.0.0.0:7835)
      --> server.yaml:13:60
       |
    13 |   quic: { client: self, protocol: udp, bind: 7835, target: 127.0.0.1:7835 }
       |                                                            ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "loop" on "self" targets 127.0.0.1:7835, which is burrow's own listen (0.0.0.0:7835)
      --> server.yaml:14:45
       |
    14 |   loop: { client: self, bind: 8103, target: 127.0.0.1:7835 }
       |                                             ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    server.yaml: 3 errors, 0 warnings
    "#);
}

#[test]
fn self_refusals() {
    let text = render(
        "server.yaml",
        &format!(
            "listen: 0.0.0.0:7835\nmetrics:\n  listen: 127.0.0.1:9100\nself:\n  allow: [127.0.0.0/8:1-9999, 169.254.0.0/16, \"unix:/run/app.sock\"]\nclients:\n  self:\n    secret: {SECRET}\nservices:\n  outside: {{ client: self, bind: 8080, target: 10.0.0.5:3000 }}\n  range: {{ client: self, bind: 8090-8091, target: 127.0.0.1:9999-10000 }}\n  socket: {{ client: self, bind: 8092, target: \"unix:/run/other.sock\" }}\n  loop: {{ client: self, bind: 8093, target: 127.0.0.1:7835 }}\n  metrics: {{ client: self, bind: 8094, target: localhost:9100 }}\n  itself: {{ client: self, bind: 8095, target: 127.0.0.1:8095 }}\n  meta: {{ client: self, bind: 8096, target: 169.254.169.254:80 }}\n"
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: a client cannot be named "self"
      --> server.yaml:7:3
       |
     7 |   self:
       |   ^^^^ reserved for this server
       = help: "self" means the server itself, in "client", "clients" and "reach"; give the client another name

    error: service "outside" on "self" targets 10.0.0.5:3000, which self.allow does not permit
      --> server.yaml:10:48
       |
    10 |   outside: { client: self, bind: 8080, target: 10.0.0.5:3000 }
       |                                                ^^^^^^^^^^^^^ not in self.allow
       = help: add "10.0.0.5/32:3000" to self.allow, or a rule that covers it

    error: service "range" on "self" targets 127.0.0.1:10000, which self.allow does not permit
      --> server.yaml:11:51
       |
    11 |   range: { client: self, bind: 8090-8091, target: 127.0.0.1:9999-10000 }
       |                                                   ^^^^^^^^^^^^^^^^^^^^ not in self.allow
       = help: add "127.0.0.1/32:10000" to self.allow, or a rule that covers it

    error: service "socket" on "self" targets unix:/run/other.sock, which self.allow does not permit
      --> server.yaml:12:47
       |
    12 |   socket: { client: self, bind: 8092, target: "unix:/run/other.sock" }
       |                                               ^^^^^^^^^^^^^^^^^^^^^^^ not in self.allow
       = help: add "unix:/run/other.sock" to self.allow, or a rule that covers it

    error: service "loop" on "self" targets 127.0.0.1:7835, which is burrow's own listen (0.0.0.0:7835)
      --> server.yaml:13:45
       |
    13 |   loop: { client: self, bind: 8093, target: 127.0.0.1:7835 }
       |                                             ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "metrics" on "self" targets 127.0.0.1:9100, which is burrow's own metrics.listen (127.0.0.1:9100)
      --> server.yaml:14:48
       |
    14 |   metrics: { client: self, bind: 8094, target: localhost:9100 }
       |                                                ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "itself" on "self" targets 127.0.0.1:8095, which is burrow's own bind of this service (0.0.0.0:8095)
      --> server.yaml:15:47
       |
    15 |   itself: { client: self, bind: 8095, target: 127.0.0.1:8095 }
       |                                               ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    error: service "meta" on "self" targets 169.254.169.254:80, a link-local or metadata address
      --> server.yaml:16:45
       |
    16 |   meta: { client: self, bind: 8096, target: 169.254.169.254:80 }
       |                                             ^^^^^^^^^^^^^^^^^^ not named in self.allow
       = help: list the address itself in self.allow, for example 169.254.169.254/32:80; a range that covers it does not count

    server.yaml: 8 errors, 0 warnings
    "#);
}

#[test]
fn reach_to_self_needs_self_allow() {
    let text = render(
        "server.yaml",
        &self_server(
            "",
            "  web: { client: home, bind: 8080, target: 3000 }\n",
            &format!(
                "visitors:\n  laptop:\n    secret: {SECRET2}\n    reach: {{ self: any }}\n  other:\n    secret: 9YTnoioCLHkV1Qo6L3s4s/0BNsE9Go9Ra+7Q7ybDVYo=\n    reach: {{ self: [127.0.0.1/32] }}\n"
            ),
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: visitor "laptop" has reach to "self", but the server config has no self.allow
      --> server.yaml:10:14
       |
    10 |     reach: { self: any }
       |              ^^^^ this server would have no limit
       = help: add what this server may reach for visitors, for example:
             self:
               allow: [127.0.0.1/32:22]

    error: visitor "other" has reach to "self", but the server config has no self.allow
      --> server.yaml:13:14
       |
    13 |     reach: { self: [127.0.0.1/32] }
       |              ^^^^ this server would have no limit
       = help: add what this server may reach for visitors, for example:
             self:
               allow: [127.0.0.1/32:22]

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn reach_any_to_self_is_warned_like_any_client() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [127.0.0.1/32]\n",
            "  web: { client: home, bind: 8080, target: 3000 }\n",
            &format!("visitors:\n  laptop:\n    secret: {SECRET2}\n    reach: {{ self: any }}\n"),
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    warning: visitor "laptop" may reach any target that client "self" allows, over TCP or UDP
      --> server.yaml:12:14
       |
    12 |     reach: { self: any }
       |              ^^^^ limited only by the client's allow list
       = help: keep "self.allow" narrow, for example allow: [127.0.0.1/32:22], or list only the services this visitor needs

    server.yaml: valid server config (1 client, 1 visitor, 1 service), 1 warning
    "#);
}

#[test]
fn self_allow_rules_are_checked() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [127.0.0.1/33, localhost]\n",
            "  web: { client: home, bind: 8080, target: 3000 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: invalid self.allow entry: "33" is not a valid prefix length
      --> server.yaml:5:11
       |
     5 |   allow: [127.0.0.1/33, localhost]
       |           ^^^^^^^^^^^^ not a valid CIDR
       = help: use a CIDR like 192.168.1.0/24, optionally with a port (192.168.1.0/24:22) or a range (:8000-8100)

    error: invalid self.allow entry: "localhost" is not an IP address or CIDR
      --> server.yaml:5:25
       |
     5 |   allow: [127.0.0.1/33, localhost]
       |                         ^^^^^^^^^ not a valid CIDR
       = help: self.allow entries are IP ranges, not host names; resolve the host and add its address, for example 192.168.1.10/32

    server.yaml: 2 errors, 0 warnings
    "#);
}

#[test]
fn a_self_allow_covering_a_whole_family_is_warned_about() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [0.0.0.0/0, \"::/0\", 0.0.0.0/0:443, 10.0.0.0/8]\n",
            "  web: { client: self, bind: 8080, target: 10.0.0.5:3000 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    warning: self.allow 0.0.0.0/0 lets this server dial any IPv4 address for services with client: self and reach: self, the internet included
      --> server.yaml:5:11
       |
     5 |   allow: [0.0.0.0/0, "::/0", 0.0.0.0/0:443, 10.0.0.0/8]
       |           ^^^^^^^^^ no restriction on targets
       = help: list only the networks this server should reach for itself, for example allow: [127.0.0.1/32, 10.0.0.0/24]

    warning: self.allow ::/0 lets this server dial any IPv6 address for services with client: self and reach: self, the internet included
      --> server.yaml:5:22
       |
     5 |   allow: [0.0.0.0/0, "::/0", 0.0.0.0/0:443, 10.0.0.0/8]
       |                      ^^^^^^ no restriction on targets
       = help: list only the networks this server should reach for itself, for example allow: [127.0.0.1/32, 10.0.0.0/24]

    warning: self.allow 0.0.0.0/0:443 lets this server dial any IPv4 address for services with client: self and reach: self, the internet included
      --> server.yaml:5:30
       |
     5 |   allow: [0.0.0.0/0, "::/0", 0.0.0.0/0:443, 10.0.0.0/8]
       |                              ^^^^^^^^^^^^^ no restriction on targets
       = help: list only the networks this server should reach for itself, for example allow: [127.0.0.1/32, 10.0.0.0/24]

    server.yaml: valid server config (1 client, 1 service), 3 warnings
    "#);
}

#[test]
fn cloud_metadata_targets_need_their_own_rule() {
    let text = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [100.64.0.0/10, \"fd00::/8\", \"fd00:ec2::254\"]\n",
            "  ali: { client: self, bind: 8080, target: 100.100.100.200:80 }\n  aws: { client: self, bind: 8081, target: \"[fd00:ec2::254]:80\" }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: service "ali" on "self" targets 100.100.100.200:80, a link-local or metadata address
      --> server.yaml:10:44
       |
    10 |   ali: { client: self, bind: 8080, target: 100.100.100.200:80 }
       |                                            ^^^^^^^^^^^^^^^^^^ not named in self.allow
       = help: list the address itself in self.allow, for example 100.100.100.200/32:80; a range that covers it does not count

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn localhost_targets_need_one_of_their_addresses_in_self_allow() {
    let text = render(
        "server.yaml",
        &format!(
            "listen: \"[::1]:7835\"\nself:\n  allow: [127.0.0.1/32, 10.0.0.0/24]\nclients:\n  home:\n    secret: {SECRET}\nservices:\n  web: {{ client: self, bind: 8080, target: localhost:13000 }}\n  loop: {{ client: self, bind: 8081, target: localhost:7835 }}\n"
        ),
        None,
    );
    insta::assert_snapshot!(text, @r#"
    error: service "loop" on "self" targets [::1]:7835, which is burrow's own listen ([::1]:7835)
      --> server.yaml:9:45
       |
     9 |   loop: { client: self, bind: 8081, target: localhost:7835 }
       |                                             ^^^^^^^^^^^^^^ a loop back into burrow
       = help: point the service at the application behind it, on another port

    server.yaml: 1 error, 0 warnings
    "#);
    let outside = render(
        "server.yaml",
        &self_server(
            "self:\n  allow: [10.0.0.0/24]\n",
            "  web: { client: self, bind: 8080, target: localhost:13000 }\n",
            "",
        ),
        None,
    );
    insta::assert_snapshot!(outside, @r#"
    error: service "web" on "self" targets 127.0.0.1:13000, which self.allow does not permit
      --> server.yaml:10:44
       |
    10 |   web: { client: self, bind: 8080, target: localhost:13000 }
       |                                            ^^^^^^^^^^^^^^^ not in self.allow
       = help: add "127.0.0.1/32:13000" to self.allow, or a rule that covers it

    server.yaml: 1 error, 0 warnings
    "#);
}

#[test]
fn a_server_whose_services_all_run_on_self_needs_no_clients() {
    let only_self = "listen: 0.0.0.0:7835\nmetrics:\n  listen: 127.0.0.1:9100\nservices:\n  web: { client: self, bind: 8080, target: localhost:3000 }\n  db: { clients: [self], bind: 8081, target: localhost:5432 }\n";
    let text = render("server.yaml", only_self, None);
    assert!(!text.contains("no clients are defined"), "{text}");
    let none = "listen: 0.0.0.0:7835\nmetrics:\n  listen: 127.0.0.1:9100\n";
    let text = render("server.yaml", none, None);
    assert!(text.contains("no clients are defined"), "{text}");
    let some =
        format!("{only_self}  nas: {{ clients: [self, home], bind: 8082, target: 10.0.0.2:80 }}\n");
    let text = render("server.yaml", &some, None);
    assert!(text.contains("no clients are defined"), "{text}");
}

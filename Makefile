JOBS ?= 4
TEST_THREADS ?= 4
DEBUG ?= line-tables-only
FLOW ?=
FUZZ_TARGETS ?= handshake frames config update patch syntax_parsers socks_udp_header
FUZZ_SECS ?= 30
KEEP_DAYS ?= 2
CARGO ?= cargo
WS_TESTS := --test integration --test reconnect --test visit --test ranges --test failover --test socks --test proxy_protocol --test sources --test admin --test admin_keepalive --test self_target
UI_DIST := crates/burrow-admin-ui/dist

export CARGO_BUILD_JOBS := $(JOBS)
export CARGO_PROFILE_DEV_DEBUG := $(DEBUG)
export CARGO_PROFILE_TEST_DEBUG := $(DEBUG)
export RUST_TEST_THREADS := $(TEST_THREADS)

.DEFAULT_GOAL := help

.PHONY: help tools build release ui ui-watch dev-server dev-visitor admin fmt fmt-check clippy clippy-native clippy-hooks clippy-no-default clippy-wasm test test-ws test-ui e2e update-baselines service-e2e fuzz size arch-check clean clean-tests ci

help:
	@printf 'burrow: make <command> [JOBS=4 TEST_THREADS=4 DEBUG=line-tables-only]\n'
	@printf '\nSetup\n'
	@printf '  %-18s %s\n' tools 'pinned wasm-bindgen, wasm-opt, Tailwind and Chrome for Testing into .tools/'
	@printf '\nBuild\n'
	@printf '  %-18s %s\n' build 'debug build of burrow'
	@printf '  %-18s %s\n' release 'release build of burrow'
	@printf '  %-18s %s\n' ui 'release build of the admin UI into crates/burrow-admin-ui/dist'
	@printf '  %-18s %s\n' ui-watch 'rebuild the admin UI on every save'
	@printf '\nDev loop (docs/admin-ui-dev.md)\n'
	@printf '  %-18s %s\n' dev-server 'run a local server with dev/server.yaml'
	@printf '  %-18s %s\n' dev-visitor 'run the dev visitor with dev/visitor.yaml'
	@printf '  %-18s %s\n' admin 'open the admin UI served from crates/burrow-admin-ui/dist'
	@printf '\nChecks\n'
	@printf '  %-18s %s\n' fmt 'format all code'
	@printf '  %-18s %s\n' fmt-check 'check the formatting'
	@printf '  %-18s %s\n' clippy 'clippy native, admin-test-hooks, no-default-features and wasm32'
	@printf '  %-18s %s\n' arch-check 'check the crate layering rules'
	@printf '  %-18s %s\n' size 'check the admin UI size budget (after make ui)'
	@printf '\nTests\n'
	@printf '  %-18s %s\n' test 'all tests'
	@printf '  %-18s %s\n' test-ws 'the integration tests over WebSocket'
	@printf '  %-18s %s\n' test-ui 'the admin UI crates without a browser'
	@printf '  %-18s %s\n' e2e 'admin UI flows in the pinned Chrome, FLOW=name filters, E2E_JOBS=n flows at a time'
	@printf '  %-18s %s\n' update-baselines 'e2e that records the visual baselines'
	@printf '  %-18s %s\n' service-e2e 'install and run as a system service (needs sudo)'
	@printf '  %-18s %s\n' fuzz 'fuzz each of FUZZ_TARGETS for FUZZ_SECS seconds (nightly)'
	@printf '  %-18s %s\n' ci 'the CI lint and test jobs, locally'
	@printf '\nClean\n'
	@printf '  %-18s %s\n' clean 'remove target/'
	@printf '  %-18s %s\n' clean-tests 'remove test binaries older than KEEP_DAYS days and incremental caches'

tools:
	$(CARGO) xtask tools

build:
	$(CARGO) build --locked

release:
	$(CARGO) build --locked --release

ui:
	$(CARGO) xtask ui --release

ui-watch:
	$(CARGO) xtask ui --watch

dev-server:
	$(CARGO) run -- server -c dev/server.yaml

dev-visitor:
	$(CARGO) run -- visit -c dev/visitor.yaml

admin:
	$(CARGO) run -- admin -c dev/visitor.yaml --ui-dir $(UI_DIST)

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

clippy: clippy-native clippy-hooks clippy-no-default clippy-wasm

clippy-native:
	$(CARGO) clippy --locked --workspace --all-targets -- -D warnings

clippy-hooks:
	$(CARGO) clippy --locked --all-targets --features admin-test-hooks -- -D warnings

clippy-no-default:
	$(CARGO) clippy --locked --all-targets --no-default-features -- -D warnings

clippy-wasm:
	$(CARGO) clippy --locked -p burrow-admin-ui -p burrow-ui-kit -p burrow-config --target wasm32-unknown-unknown -- -D warnings

arch-check:
	$(CARGO) xtask arch-check

size:
	$(CARGO) xtask size --budget

test:
	$(CARGO) test --locked

test-ws:
	BURROW_TEST_TRANSPORT=ws $(CARGO) test --locked $(WS_TESTS)

test-ui:
	$(CARGO) test --locked -p burrow-config -p burrow-admin-logic -p burrow-admin-proto -p burrow-syntax -p burrow-ui-kit -p burrow-admin-ui

e2e:
	$(CARGO) xtask e2e $(FLOW) $(if $(E2E_JOBS),--jobs $(E2E_JOBS))

update-baselines:
	$(CARGO) xtask e2e $(FLOW) --update-baselines

service-e2e:
	BURROW_E2E_PROFILE=dev bash tests/e2e/service.sh

fuzz:
	cd fuzz && for t in $(FUZZ_TARGETS); do $(CARGO) fuzz run $$t -- -max_total_time=$(FUZZ_SECS) -max_len=4096 || exit 1; done

clean:
	$(CARGO) clean

clean-tests:
	find target -type f -path '*/deps/*' -perm -u+x ! -name '*.so' ! -name '*.dylib' ! -name '*.dll' -mtime +$(KEEP_DAYS) -delete
	find target -type d -name incremental -prune -exec rm -rf {} +

ci: fmt-check clippy-native clippy-hooks arch-check test test-ws service-e2e

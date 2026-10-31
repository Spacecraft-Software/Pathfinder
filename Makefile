# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later

CARGO ?= cargo
PREFIX ?= $(HOME)/.local

.PHONY: all check fmt lint test diff conformance build install clean reuse

all: build

build:
	$(CARGO) build --release

check: fmt lint test

fmt:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# The shim's suites, then the engine crates' own tests (crates/UPSTREAM.md).
test:
	$(CARGO) test
	$(CARGO) test --workspace --exclude pathfinder-jq

# The differential suite alone, with verbose output. Fetches jq ephemerally via
# nix when PATHFINDER_REAL_JQ is unset; nothing is installed on the host.
diff:
	$(CARGO) test --test differential -- --nocapture

# jq 1.8.1's own test suite through the shim: per-file pass rates, the FLOOR
# ratchet, and (with V=1) every failing case. Needs no jq.
conformance:
	$(if $(V),PATHFINDER_CONFORMANCE_VERBOSE=1) $(CARGO) test --test conformance -- --nocapture

# Installs the binary, the `mjq` alias and the `jq` symlink into
# $(PREFIX)/bin. Refuses to clobber an existing jq or mjq that is not
# Pathfinder's, before touching the prefix; pass FORCE=--force to override.
install: build
	@if [ -z "$(FORCE)" ] && { [ -e $(PREFIX)/bin/mjq ] || [ -L $(PREFIX)/bin/mjq ]; } \
	    && [ "$$(readlink $(PREFIX)/bin/mjq)" != pathfinder ]; then \
		echo "$(PREFIX)/bin/mjq exists and is not Pathfinder's; pass FORCE=--force to replace it" >&2; \
		exit 1; \
	fi
	install -Dm755 target/release/pathfinder $(PREFIX)/bin/pathfinder
	ln -sf pathfinder $(PREFIX)/bin/mjq
	$(PREFIX)/bin/pathfinder --install-shim $(PREFIX)/bin $(FORCE)

reuse:
	reuse lint

clean:
	$(CARGO) clean

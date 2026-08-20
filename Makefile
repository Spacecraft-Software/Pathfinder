# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later

CARGO ?= cargo
PREFIX ?= $(HOME)/.local

.PHONY: all check fmt lint test diff build install clean reuse

all: build

build:
	$(CARGO) build --release

check: fmt lint test

fmt:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --all-targets -- -D warnings

test:
	$(CARGO) test

# The differential suite alone, with verbose output. Fetches jq ephemerally via
# nix when PATHFINDER_REAL_JQ is unset; nothing is installed on the host.
diff:
	$(CARGO) test --test differential -- --nocapture

# Installs the binary and the `jq` symlink into $(PREFIX)/bin.
# Refuses to clobber an existing jq; pass FORCE=--force to override.
install: build
	install -Dm755 target/release/pathfinder $(PREFIX)/bin/pathfinder
	$(PREFIX)/bin/pathfinder --install-shim $(PREFIX)/bin $(FORCE)

reuse:
	reuse lint

clean:
	$(CARGO) clean

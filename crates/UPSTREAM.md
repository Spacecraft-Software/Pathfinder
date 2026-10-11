<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder — Upstream

Where the engine crates under `crates/` came from (PLAN P-049). Pathfinder's
engine is a hard fork of [jaq](https://github.com/01mf02/jaq) by Michael
Färber and contributors, MIT-licensed. Nothing here is sent upstream
(Standard §6.4); fixes from later jaq releases are ported by hand (TODO T-020).

## Import base

| What | Value |
|---|---|
| jaq version | **3.1.1** (the `jaq` CLI crate; library crates at their own versions, below) |
| Upstream tag | `v3.1.1` (lightweight) |
| Upstream commit | `c866e70303b5dbc37d83a0b0cbacf10e90af9c8c` (`git ls-remote https://github.com/01mf02/jaq refs/tags/v3.1.1`) |
| Taken from | nixpkgs `jaq.src` at this flake's locked nixpkgs |
| nixpkgs rev | `151fa4e8ddfdd8dd25d945ad94ed54a13de9f6e4` (`narHash` `sha256-Miqqk/ammqnTUxaoCyvtwoPeLbI1ksFyLxi/CazZpWY=`, `lastModified` 1791260994) |
| Source URL | `https://github.com/01mf02/jaq/archive/refs/tags/v3.1.1.tar.gz` |
| Source hash | `sha256-/yAwLcPwfW5UH+PCCrsFaM0Nuk1S5QONLsNgvVCBLX8=` (`jaq.src.outputHash`) |
| Store path | `/nix/store/i9ws7pnvawh78xc8mx44mk36nxgk07m9-source` |

To reproduce the source tree:

```sh
nix eval --raw --inputs-from . nixpkgs#jaq.version                 # 3.1.1
nix build --no-link --print-out-paths --inputs-from . nixpkgs#jaq.src
```

The tree the patches in `packaging/jaq/` were developed against was checked
with `diff -r` and is identical to that store path.

# Contributing to Spacecraft Software

Thank you for your interest. Please read this document before opening an
issue or pull request — it sets honest expectations for both sides so no
one's time is wasted.

## Project Stance

Spacecraft Software is a **personal hobby project**. It is shaped around the
maintainer's own use case and developed at hobby pace. A handful of
subprojects (e.g., **Anvil-SSH**) are intentionally designed for general
use; the umbrella as a whole is not.

This is **not** a community-driven project, but external input is welcome
and appreciated within the bounds set out below.

## What Is Welcome

- **Bug reports** — clear, reproducible, with environment details (OS,
  kernel, Rust toolchain version, shell, relevant config).
- **Suggestions** — features, refactors, naming proposals (must be
  metallurgical per the [Spacecraft Software Standard §2](https://SpacecraftSoftware.org/)),
  design feedback.
- **Pull requests** — small, focused, and aligned with the Spacecraft Software
  Standard.
- **Documentation fixes** — typos, inaccuracies, broken links,
  clarifications, translations.
- **Test coverage improvements** — almost always merge-worthy.

## What Is Not Guaranteed

- **PR acceptance.** Direction, scope, and quality bar are set by the
  maintainer alone. A submitted contribution is not a guaranteed merge,
  even if it is correct, well-written, and passes CI. If a PR is not
  accepted, that is a judgment of fit, not of the work.
- **Response time.** This is a hobby project. Expect responses on the
  order of days to weeks, not hours.
- **Roadmap influence.** Suggestions may inform direction but do not
  override the maintainer's plans.
- **API stability for in-progress work.** Pre-1.0 subprojects may break
  in any release.

## Before Opening a PR

1. **Open an issue first** for non-trivial changes. Discuss the design
   before writing code.
2. **Read the Spacecraft Software Standard.** Memory safety → performance →
   hardened security, in that order. Rust where viable. POSIX-compliant
   CLIs. GPL-3.0-or-later with SPDX headers on source files.
3. **Match the project's CLI standard** if it ships a CLI (see the
   `spacecraft-cli-standard` skill / spec).
4. **Run the full test suite locally.** PRs that don't pass CI will not
   be reviewed.
5. **Use the project's preferred toolchain.** Format with `rustfmt`,
   lint with `clippy -- -D warnings`, and run `cargo audit` for any
   added dependency.
6. **Sign-off your commits** (`git commit -s`) under the
   [Developer Certificate of Origin](https://developercertificate.org/).

## Commit Style

- Conventional Commits prefix (`feat:`, `fix:`, `docs:`, `refactor:`,
  `test:`, `chore:`, `perf:`, `build:`, `ci:`).
- Subject ≤ 72 characters, imperative mood ("add" not "added").
- Body wrapped at 72 columns; explain *why*, not just *what*.
- Reference issues by number (`Closes #42`).

## Forking

If your needs diverge from the maintainer's, or you want to take a project
in a different direction, **fork it**. That is exactly what
GPL-3.0-or-later is for. The only constraints are those imposed by the
license itself: keep the source open and under a compatible license,
preserve copyright notices, and pass the same freedoms downstream.

## Reporting Security Issues

For security-sensitive bugs, do **not** open a public issue. Email
&lt;Mohamed.Hammad@SpacecraftSoftware.org&gt; with details. PGP key available on request.

A coordinated-disclosure window of 90 days from acknowledgment is the
default; this can be shortened or lengthened by mutual agreement.

## License of Contributions

By submitting a contribution, you agree that it will be licensed under
**GPL-3.0-or-later**, the same terms as the project. Contributions that
cannot be licensed under GPL-3.0-or-later cannot be accepted.

You retain copyright in your contributions; no CLA is required.

---

**Maintainer:** Mohamed Hammad &lt;Mohamed.Hammad@SpacecraftSoftware.org&gt;
**License:** GPL-3.0-or-later
**Website:** <https://SpacecraftSoftware.org/>

*--- Forged in Spacecraft Software ---*

// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The shim's front end, compiled into the fuzz targets.
//!
//! The shim is a binary without a lib target, so its modules are included here
//! by path, at a crate root of their own: they refer to each other as
//! `crate::…`, which resolves only when they sit at the root together. This is
//! the closure of `jqargs` and `program::assemble`; nothing here reaches
//! `translate`, `inputs`, `post`, `native` or `main`.

#![allow(
    dead_code,
    unused_imports,
    reason = "the shim's modules carry far more than the fuzz targets call"
)]

#[path = "../../crates/pathfinder/src/jqargs.rs"]
pub mod jqargs;

#[path = "../../crates/pathfinder/src/prelude.rs"]
pub mod prelude;

#[path = "../../crates/pathfinder/src/program.rs"]
pub mod program;

#[path = "../../crates/pathfinder/src/scan.rs"]
pub mod scan;

#[path = "../../crates/pathfinder/src/syntax/mod.rs"]
pub mod syntax;

// Rust guideline compliant 2026-05-18

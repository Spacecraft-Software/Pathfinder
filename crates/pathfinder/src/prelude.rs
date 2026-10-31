// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! jq-source definitions injected ahead of the user's filter.
//!
//! The engine defines jq 1.8.1's builtins itself (M20–M23): its natives and
//! its `defs.jq` are jq's. Two definitions are left here, injected only when
//! a filter needs them:
//!
//! - `builtins`, jq's list of some 230 names, kept out of the engine because
//!   its definitions are compiled on every run, and a filter rarely asks.
//! - `_pf_modify`, jq's own `_modify`, which the rewriter sends an update of
//!   several paths through (`(.a, .b) |= f`): jq deletes the paths `f`
//!   yields nothing for together at the end, the engine's update one by one.
//!
//! Definitions are **prepended, never appended**: under jq the *last*
//! definition of a name wins, so a user's own `def builtins:` in the filter
//! overrides anything here.

/// jq 1.8.1's `builtins` output, captured with `jq -nc 'builtins'`.
///
/// Captured unsorted, in jq's own order: `builtins|sort == builtins` is `false`
/// under jq, and a sorted copy would answer `true`.
pub const JQ_BUILTINS: &str = include_str!("embed/jq-builtins.json");

/// One injectable definition.
struct Def {
    /// The name that triggers injection.
    name: &'static str,
    /// The definition's jq source, or `None` when it is built per run.
    src: Option<&'static str>,
}

/// Names a filter may call that the engine leaves to the prelude.
static POLYFILLS: &[Def] = &[
    // Built per run; see `render`.
    Def {
        name: "builtins",
        src: None,
    },
];

/// Helpers the rewriter asks for by name; a filter cannot call them, being
/// `_`-prefixed (P-066).
static INTERNAL: &[Def] = &[Def {
    name: "_pf_modify",
    // jq 1.8.1's `_modify`, on the engine's `setpath` and `delpaths`, which
    // are jq's.
    src: Some(
        "def _pf_modify(paths; update): reduce path(paths) as $p ([., []]; \
             . as $__pf_dot | null | label $__pf_out | ($__pf_dot[0] | getpath($p)) as $__pf_v \
             | (($__pf_v | update | (., break $__pf_out) as $__pf_v | $__pf_dot \
             | setpath([0] + $p; $__pf_v)), \
             ($__pf_dot | setpath([1, (.[1] | length)]; $p)))) \
             | . as $__pf_dot | $__pf_dot[0] | delpaths($__pf_dot[1]);",
    ),
}];

/// What the program's assembly needs to know of its command line.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// jaq's parser reads the program (`crate::front::FRONT_END_ENV`), so it
    /// takes the syntax rewrites.
    pub jaq_front_end: bool,
    /// The variables the command line binds (`--arg`, `--argjson`,
    /// `--slurpfile`, `--rawfile`), which a program may use undeclared.
    pub named: Vec<String>,
}

impl Context {
    /// The rewrites the program needs (`crate::syntax::rewrite`).
    pub const fn rewrites(&self) -> crate::syntax::rewrite::Rewrites {
        crate::syntax::rewrite::Rewrites {
            comma_update: true,
            syntax: self.jaq_front_end,
        }
    }
}

fn lookup(name: &str) -> Option<&'static Def> {
    POLYFILLS.iter().chain(INTERNAL).find(|d| d.name == name)
}

/// Names jq 1.8.1 defines that Pathfinder cannot provide.
///
/// Returned so the caller can emit one precise diagnostic naming the construct
/// rather than an undefined-name error jq would not give.
pub fn inexpressible(name: &str) -> Option<&'static str> {
    match name {
        "modulemeta" | "get_search_list" | "get_jq_origin" | "get_prog_origin" => {
            Some("Pathfinder's module system exposes no introspection surface")
        }
        "lgamma_r" => {
            Some("Pathfinder provides no lgamma_r; use `lgamma` if an approximation will do")
        }
        _ => None,
    }
}

/// Render the definitions a filter needs, as a single line.
///
/// One line matters: errors are reported against the program text, so keeping
/// the prelude on line 1 alongside the filter's own first line leaves the
/// user's line numbers unchanged rather than shifting them.
///
/// Returns an empty string when nothing is needed — the common case, and the
/// one that keeps the program byte-identical to what the user typed.
pub fn render(wanted: &[String]) -> String {
    let mut out = String::new();
    for def in wanted.iter().filter_map(|n| lookup(n)) {
        if !out.is_empty() {
            out.push(' ');
        }
        if let Some(src) = def.src {
            out.push_str(src);
        } else {
            out.push_str("def builtins: ");
            out.push_str(JQ_BUILTINS.trim());
            out.push(';');
        }
    }
    out
}

/// Every name a filter can call that triggers an injection.
pub fn known_names() -> Vec<&'static str> {
    POLYFILLS.iter().map(|d| d.name).collect()
}

#[cfg(test)]
mod tests {
    use super::{inexpressible, known_names, render};

    fn want(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn nothing_wanted_renders_nothing() {
        assert_eq!(render(&[]), "");
        assert_eq!(render(&want(&["tostream", "setpath"])), "");
    }

    #[test]
    fn renders_on_a_single_line() {
        let out = render(&want(&["builtins", "_pf_modify"]));
        assert!(!out.contains('\n'));
        assert!(out.starts_with("def builtins: ["));
        assert!(out.contains(" def _pf_modify(paths; update):"));
    }

    #[test]
    fn builtins_is_jqs_list_in_jqs_order() {
        let out = render(&want(&["builtins"]));
        let list = out
            .strip_prefix("def builtins: ")
            .and_then(|s| s.strip_suffix(';'))
            .expect("one definition");
        let names: Vec<String> = serde_json::from_str(list).expect("a JSON array");
        let mut sorted = names.clone();
        sorted.sort();
        assert_ne!(names, sorted, "jq's `builtins` is unsorted");
        assert!(names.iter().any(|n| n == "tostream/0"));
    }

    #[test]
    fn modify_is_jqs_on_the_engines_builtins() {
        let out = render(&want(&["_pf_modify"]));
        assert!(out.contains("setpath([0] + $p;"));
        assert!(out.contains("delpaths($__pf_dot[1])"));
        assert!(!out.contains("_pf_setpath") && !out.contains("_pf_delpaths"));
    }

    #[test]
    fn only_builtins_is_a_name_a_filter_can_call() {
        assert_eq!(known_names(), ["builtins"]);
    }

    #[test]
    fn inexpressible_names_are_named_not_polyfilled() {
        assert!(inexpressible("modulemeta").is_some());
        assert!(inexpressible("input_line_number").is_none());
        assert!(inexpressible("tostream").is_none());
    }
}

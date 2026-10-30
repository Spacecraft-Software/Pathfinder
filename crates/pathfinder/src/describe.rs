// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `pathfinder --describe` and `pathfinder --schema`: Pathfinder describing
//! itself to an agent (CLI Standard §2 rule 4).
//!
//! Both are rendered from one table, [`COMMANDS`], so the manifest and the
//! schema cannot drift apart. The verbs are flags rather than the Standard's
//! `describe`/`schema` words because a bare word is a jq program: `pathfinder
//! describe` must stay the jq call it is.
//!
//! Each command's `parameters` is a JSON Schema (Draft 2020-12) usable as an
//! LLM tool's input schema as written: no `$ref`, a `type` and a
//! `description` on every property. `--schema --format` wraps them for a
//! provider's function-calling API.

use crate::json::{self, Object, push_str};
use crate::mode::Mode;
use crate::native::{MAINTAINER, Meta, WEBSITE, envelope};

/// One command of the surface, as the manifest and the schema describe it.
struct Command {
    /// The tool name an LLM calls it by.
    tool: &'static str,
    /// The command line, as a synopsis.
    synopsis: &'static str,
    description: &'static str,
    /// JSON Schema of the accepted input.
    parameters: &'static str,
    /// JSON Schema of the output: `data` of the JSON report, or stdout.
    output_schema: &'static str,
    exit_codes: &'static [(u8, &'static str)],
    /// `(command, description)`; runnable as written.
    examples: &'static [(&'static str, &'static str)],
    supports_json: bool,
    idempotent: bool,
    destructive: bool,
}

/// Parameters shared by the commands that take a jq call.
const JQ_CALL: &str = r#"{"type":"object","required":["filter"],"properties":{"filter":{"type":"string","description":"The jq program, for example \".items[] | .name\"."},"options":{"type":"array","items":{"type":"string"},"description":"jq 1.8.1 command-line options, in order, for example [\"-c\", \"--arg\", \"name\", \"value\"]."},"files":{"type":"array","items":{"type":"string"},"description":"Input files, read as one stream; standard input when empty."}},"additionalProperties":false}"#;

/// No parameters.
const NONE: &str = r#"{"type":"object","properties":{},"additionalProperties":false}"#;

/// The surface, in the order `--help` lists it.
const COMMANDS: &[Command] = &[
    Command {
        tool: "jq",
        synopsis: "jq [OPTIONS] FILTER [FILE...]",
        description: "Run a jq 1.8.1 filter over JSON input; stdout, stderr and exit status are jq 1.8.1's.",
        parameters: JQ_CALL,
        output_schema: r#"{"description":"jq's output on stdout: one JSON text per result, or raw text under -r/-j; never an envelope. Under the name jq nothing of Pathfinder's own surface applies."}"#,
        exit_codes: &[
            (0, "SUCCESS"),
            (1, "with -e, the last output was false or null"),
            (2, "USAGE_ERROR, or an input file could not be read"),
            (3, "COMPILE_ERROR: jq refuses the program"),
            (4, "with -e, there was no output"),
            (5, "an uncaught runtime error or malformed input"),
        ],
        examples: &[
            (
                "jq -c '.items[] | {name}' data.json",
                "One compact object per item",
            ),
            (
                "pathfinder --explain --json -c '.items[] | {name}'",
                "How the same call would run, as JSON",
            ),
        ],
        supports_json: false,
        idempotent: true,
        destructive: false,
    },
    Command {
        tool: "pathfinder_explain",
        synopsis: "pathfinder --explain [--json] [OPTIONS] FILTER",
        description: "Show how a jq call would run (engine, rewritten program, input and output framing) without running it.",
        parameters: JQ_CALL,
        output_schema: r#"{"type":"object","properties":{"engine":{"type":"string","enum":["native"]},"program":{"type":"object","properties":{"rewritten":{"type":"boolean"},"injected":{"type":"array","items":{"type":"string"}},"shadowed":{"type":"array","items":{"type":"string"}},"syntax_rewritten":{"type":"boolean"},"text":{"type":"string"}},"required":["rewritten"]},"execution":{"type":"string","enum":["in-process"]},"stdout":{"type":"object","properties":{"ascii":{"type":"boolean"},"seq":{"type":"boolean"}}},"stdin":{"type":"object","properties":{"strip_rs":{"type":"boolean"},"concat_files":{"type":"integer"}}}},"required":["engine","program","execution","stdout","stdin"]}"#,
        exit_codes: &[
            (0, "SUCCESS"),
            (2, "USAGE_ERROR or UNSUPPORTED"),
            (3, "COMPILE_ERROR: jq refuses the program"),
        ],
        examples: &[
            (
                "pathfinder --explain -c '.a.b = 1'",
                "The rewritten assignment, as text",
            ),
            (
                "pathfinder --explain --json -c '.a.b = 1'",
                "The same, as one JSON document",
            ),
        ],
        supports_json: true,
        idempotent: true,
        destructive: false,
    },
    Command {
        tool: "pathfinder_version",
        synopsis: "pathfinder --version [--json]",
        description: "Report Pathfinder's version, the jq it reproduces, and the engine a jq call runs on.",
        parameters: NONE,
        output_schema: r#"{"type":"object","properties":{"version":{"type":"string"},"jq_baseline":{"type":"string"},"engine":{"type":"object","properties":{"name":{"type":"string","enum":["native"]}},"required":["name"]}},"required":["version","jq_baseline","engine"]}"#,
        exit_codes: &[
            (0, "SUCCESS"),
            (
                2,
                "USAGE_ERROR: PATHFINDER_ENGINE asks for the removed jaq engine",
            ),
        ],
        examples: &[
            ("pathfinder --version", "As text"),
            ("pathfinder --version --json", "As one JSON document"),
        ],
        supports_json: true,
        idempotent: true,
        destructive: false,
    },
    Command {
        tool: "pathfinder_install_shim",
        synopsis: "pathfinder --install-shim [DIR] [--force] [--json]",
        description: "Link DIR/jq to this binary so that `jq` runs Pathfinder; refuses to replace an existing jq without --force.",
        parameters: r#"{"type":"object","properties":{"dir":{"type":"string","default":".","description":"The directory to create the jq link in; put it on PATH ahead of any real jq."},"force":{"type":"boolean","default":false,"description":"Replace an existing DIR/jq."}},"additionalProperties":false}"#,
        output_schema: r#"{"type":"object","properties":{"installed":{"type":"string"},"target":{"type":"string"}},"required":["installed","target"]}"#,
        exit_codes: &[
            (0, "SUCCESS"),
            (1, "GENERAL: the link could not be made"),
            (4, "PERMISSION_DENIED"),
            (5, "CONFLICT: DIR/jq exists and --force was not given"),
        ],
        examples: &[
            (
                "pathfinder --install-shim ~/.local/bin",
                "Answer to jq from ~/.local/bin",
            ),
            (
                "pathfinder --install-shim ~/.local/bin --json",
                "The same, reported as JSON",
            ),
        ],
        supports_json: true,
        idempotent: false,
        destructive: true,
    },
    Command {
        tool: "pathfinder_describe",
        synopsis: "pathfinder --describe [--json]",
        description: "List Pathfinder's commands, environment and conventions: the manifest an agent reads first.",
        parameters: NONE,
        output_schema: r#"{"type":"object","properties":{"tool":{"type":"string"},"version":{"type":"string"},"description":{"type":"string"},"license":{"type":"string"},"homepage":{"type":"string"},"commands":{"type":"array","items":{"type":"object"}},"environment":{"type":"array","items":{"type":"object"}},"global_flags":{"type":"array","items":{"type":"string"}},"output_formats":{"type":"array","items":{"type":"string"}},"mcp_available":{"type":"boolean"},"schema_command":{"type":"string"},"context_files":{"type":"array","items":{"type":"string"}}},"required":["tool","version","commands"]}"#,
        exit_codes: &[(0, "SUCCESS")],
        examples: &[
            ("pathfinder --describe", "As text"),
            ("pathfinder --describe --json", "As one JSON document"),
        ],
        supports_json: true,
        idempotent: true,
        destructive: false,
    },
    Command {
        tool: "pathfinder_schema",
        synopsis: "pathfinder --schema [COMMAND] [--format json|anthropic|openai|gemini|mcp]",
        description: "Print the JSON Schema of every command, or of COMMAND, raw or wrapped for an LLM provider's tool API.",
        parameters: r#"{"type":"object","properties":{"command":{"type":"string","enum":["jq","pathfinder_explain","pathfinder_version","pathfinder_install_shim","pathfinder_describe","pathfinder_schema"],"description":"One command's schema instead of all."},"format":{"type":"string","enum":["json","anthropic","openai","gemini","mcp"],"default":"json","description":"json: the raw schema; the others: the tools wrapped for that provider."}},"additionalProperties":false}"#,
        output_schema: r#"{"description":"A JSON Schema document (json), or the commands as the named provider's tool definitions."}"#,
        exit_codes: &[
            (0, "SUCCESS"),
            (2, "USAGE_ERROR: unknown command or format"),
        ],
        examples: &[
            ("pathfinder --schema", "Every command's schema, as JSON"),
            (
                "pathfinder --schema --format anthropic",
                "The commands as Anthropic tool definitions",
            ),
        ],
        supports_json: true,
        idempotent: true,
        destructive: false,
    },
];

/// The environment variables that change what Pathfinder does.
const ENVIRONMENT: &[(&str, &str)] = &[
    (
        "PATHFINDER_ENGINE",
        "unset or native; any other value fails (the jaq engine was removed in 0.2.0)",
    ),
    (
        "PATHFINDER_NO_REWRITE",
        "set to anything: run the filter as written, without jq's repairs",
    ),
    (
        "AI_AGENT, AGENT",
        "set to anything: the native verbs report as JSON; never changes a jq call",
    ),
    (
        "CI",
        "true unless false or 0: the native verbs report as JSON; never changes a jq call",
    ),
];

/// A provider's tool-definition format for `--schema --format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The raw JSON Schema document.
    Json,
    Anthropic,
    OpenAi,
    Gemini,
    Mcp,
}

impl Format {
    /// The format named on the command line.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "json" => Some(Self::Json),
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            "gemini" => Some(Self::Gemini),
            "mcp" => Some(Self::Mcp),
            _ => None,
        }
    }
}

/// `pathfinder --describe`.
pub fn describe(mode: Mode) {
    match mode {
        Mode::Human => print!("{}", describe_human()),
        Mode::Json => println!(
            "{}",
            envelope(&Meta::now("pathfinder --describe"), manifest())
        ),
    }
}

/// The manifest, as the JSON report's `data`.
fn manifest() -> Object {
    let commands = json::array(COMMANDS.iter().map(|c| {
        Object::new()
            .str("name", c.synopsis)
            .str("tool", c.tool)
            .str("description", c.description)
            .bool("supports_json", c.supports_json)
            .bool("supports_dry_run", false)
            .bool("idempotent", c.idempotent)
            .bool("destructive", c.destructive)
            .finish()
    }));
    let environment = json::array(ENVIRONMENT.iter().map(|(name, effect)| {
        Object::new()
            .str("name", name)
            .str("effect", effect)
            .finish()
    }));
    Object::new()
        .str("tool", "pathfinder")
        .str("version", env!("CARGO_PKG_VERSION"))
        .str("description", env!("CARGO_PKG_DESCRIPTION"))
        .str("license", env!("CARGO_PKG_LICENSE"))
        .str("homepage", WEBSITE)
        .str("maintainer", MAINTAINER)
        .str("jq_name", "Installed as jq, Pathfinder is jq 1.8.1: none of its own flags, reports or environment detection apply to a jq call.")
        .raw("commands", &commands)
        .raw("environment", &environment)
        .strs("global_flags", &["--json"])
        .strs("output_formats", &["text", "json"])
        .bool("mcp_available", false)
        .str("schema_command", "pathfinder --schema")
        .strs(
            "context_files",
            &["AGENTS.md", "CLAUDE.md", "SKILL.md", "CONTRIBUTING.md"],
        )
}

fn describe_human() -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "pathfinder {}: {}\n\nCommands:\n",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_DESCRIPTION")
    );
    for c in COMMANDS {
        // Writing into a `String` cannot fail.
        let _ = writeln!(out, "  {}\n      {}", c.synopsis, c.description);
    }
    out.push_str("\nEnvironment:\n");
    for (name, effect) in ENVIRONMENT {
        let _ = writeln!(out, "  {name}\n      {effect}");
    }
    let _ = write!(
        out,
        "\nUnder the name jq, none of this applies: the command line, output and exit\n\
         status are jq 1.8.1's.\n\
         \n\
         pathfinder --schema prints every command's JSON Schema.\n\
         \n\
         Maintained by {MAINTAINER}\n{WEBSITE}\n"
    );
    out
}

/// `pathfinder --schema`: the document for `command` (every command when
/// `None`) in `format`, or `None` when no command has that tool name.
pub fn schema(command: Option<&str>, format: Format) -> Option<String> {
    let chosen: Vec<&Command> = match command {
        None => COMMANDS.iter().collect(),
        Some(name) => vec![COMMANDS.iter().find(|c| c.tool == name)?],
    };
    let text = match format {
        Format::Json => match chosen.as_slice() {
            [one] if command.is_some() => command_schema(one),
            _ => Object::new()
                .str("$schema", DRAFT)
                .str("tool", "pathfinder")
                .str("version", env!("CARGO_PKG_VERSION"))
                .raw(
                    "commands",
                    &json::array(chosen.iter().map(|c| command_schema(c))),
                )
                .finish(),
        },
        Format::Anthropic => json::array(chosen.iter().map(|c| {
            Object::new()
                .str("name", c.tool)
                .str("description", c.description)
                .raw("input_schema", c.parameters)
                .finish()
        })),
        Format::OpenAi => json::array(chosen.iter().map(|c| {
            let function = Object::new()
                .str("name", c.tool)
                .str("description", c.description)
                .raw("parameters", c.parameters);
            Object::new()
                .str("type", "function")
                .obj("function", function)
                .finish()
        })),
        Format::Gemini => Object::new()
            .raw(
                "function_declarations",
                &json::array(chosen.iter().map(|c| {
                    Object::new()
                        .str("name", c.tool)
                        .str("description", c.description)
                        .raw("parameters", c.parameters)
                        .finish()
                })),
            )
            .finish(),
        Format::Mcp => Object::new()
            .raw(
                "tools",
                &json::array(chosen.iter().map(|c| {
                    Object::new()
                        .str("name", c.tool)
                        .str("description", c.description)
                        .raw("inputSchema", c.parameters)
                        .finish()
                })),
            )
            .finish(),
    };
    Some(text)
}

/// JSON Schema's dialect identifier.
const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// One command's full schema (CLI Standard: schema introspection §1).
fn command_schema(c: &Command) -> String {
    let mut exit_codes = Object::new();
    for (code, meaning) in c.exit_codes {
        exit_codes = exit_codes.str(&code.to_string(), meaning);
    }
    let examples = json::array(c.examples.iter().map(|(command, description)| {
        Object::new()
            .str("command", command)
            .str("description", description)
            .finish()
    }));
    Object::new()
        .str("$schema", DRAFT)
        .str("command", c.synopsis)
        .str("tool", c.tool)
        .str("description", c.description)
        .raw("parameters", c.parameters)
        .raw("output_schema", c.output_schema)
        .obj("exit_codes", exit_codes)
        .raw("examples", &examples)
        .bool("supports_json", c.supports_json)
        .bool("supports_dry_run", false)
        .bool("idempotent", c.idempotent)
        .bool("destructive", c.destructive)
        .finish()
}

/// The tool names `--schema COMMAND` accepts, for its error's hint.
pub fn tool_names() -> String {
    let mut out = String::new();
    for (i, c) in COMMANDS.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        push_str(&mut out, c.tool);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, Format, manifest, schema};

    fn parse(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
    }

    /// Every parameter schema is one an LLM provider takes as written: an
    /// object, with a type and a description on every property.
    #[test]
    fn parameters_are_tool_input_schemas() {
        for c in COMMANDS {
            let p = parse(c.parameters);
            assert_eq!(p["type"], "object", "{}", c.tool);
            for (name, prop) in p["properties"].as_object().expect("properties") {
                assert!(prop.get("type").is_some(), "{}.{name}", c.tool);
                assert!(prop.get("description").is_some(), "{}.{name}", c.tool);
            }
            parse(c.output_schema);
            assert!(c.examples.len() >= 2, "{}", c.tool);
        }
    }

    #[test]
    fn the_schema_documents_every_command() {
        let all = parse(&schema(None, Format::Json).expect("all"));
        assert_eq!(
            all["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(
            all["commands"].as_array().map(Vec::len),
            Some(COMMANDS.len())
        );
        for c in COMMANDS {
            let one = parse(&schema(Some(c.tool), Format::Json).expect("one"));
            for key in [
                "command",
                "description",
                "parameters",
                "output_schema",
                "exit_codes",
                "examples",
            ] {
                assert!(one.get(key).is_some(), "{}: {key}", c.tool);
            }
        }
        assert!(schema(Some("nope"), Format::Json).is_none());
    }

    #[test]
    fn provider_formats_wrap_the_same_parameters() {
        let anthropic = parse(&schema(None, Format::Anthropic).expect("anthropic"));
        assert_eq!(anthropic[0]["name"], "jq");
        assert_eq!(anthropic[0]["input_schema"]["required"][0], "filter");
        let openai = parse(&schema(None, Format::OpenAi).expect("openai"));
        assert_eq!(openai[1]["type"], "function");
        assert_eq!(openai[1]["function"]["name"], "pathfinder_explain");
        let gemini = parse(&schema(None, Format::Gemini).expect("gemini"));
        assert!(gemini["function_declarations"][0]["parameters"].is_object());
        let mcp = parse(&schema(Some("pathfinder_version"), Format::Mcp).expect("mcp"));
        assert_eq!(mcp["tools"][0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn the_manifest_lists_every_command() {
        let m = parse(&manifest().finish());
        assert_eq!(m["tool"], "pathfinder");
        assert_eq!(m["commands"].as_array().map(Vec::len), Some(COMMANDS.len()));
        assert_eq!(m["schema_command"], "pathfinder --schema");
    }
}

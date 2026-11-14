//! Hadolint rules that need only the Dockerfile AST, reimplemented from hadolint's rule
//! documentation (DL3002, DL3003, DL3004, DL3008, DL3013-DL3016, DL3018-DL3020, DL3027, DL3042,
//! DL4006). Package-manager checks live in `packages`.

use dockerfile_parser::{Instruction, Span};
use poly_engine_api::engine::Severity;

use super::packages::{self, Hit, RunContext};
use super::shell::{self, Script};

/// A rule match anchored to the instruction that produced it.
pub(super) struct Finding {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub span: Span,
}

impl Finding {
    fn from_hit(hit: Hit, span: Span) -> Self {
        Self {
            code: hit.code,
            severity: hit.severity,
            message: hit.message,
            span,
        }
    }
}

/// File-level state the rules carry from instruction to instruction.
#[derive(Default)]
pub(super) struct RuleState {
    pip_no_cache_env: bool,
    pipefail: bool,
    non_posix_shell: bool,
    last_user: Option<(bool, Span)>,
}

impl RuleState {
    /// Runs every extra rule that applies to `ins` and returns its findings.
    pub(super) fn on_instruction(&mut self, ins: &Instruction) -> Vec<Finding> {
        match ins {
            Instruction::From(_) => {
                self.pipefail = false;
                self.non_posix_shell = false;
                self.last_user = None;
                Vec::new()
            }
            Instruction::Env(env) => {
                self.pip_no_cache_env |= env.vars.iter().any(|v| v.key.content == "PIP_NO_CACHE_DIR");
                Vec::new()
            }
            Instruction::Run(run) => match run.as_shell() {
                Some(shell) => self.check_run(&shell.to_string(), run.span),
                None => Vec::new(),
            },
            Instruction::Misc(misc) => {
                let args = misc.arguments.to_string();
                match misc.instruction.content.to_ascii_uppercase().as_str() {
                    "ADD" => check_add(&args, misc.span),
                    "USER" => {
                        self.last_user = Some((is_root(&args), misc.span));
                        Vec::new()
                    }
                    "SHELL" => {
                        let lower = args.to_ascii_lowercase();
                        self.pipefail |= lower.contains("pipefail");
                        self.non_posix_shell |= lower.contains("powershell") || lower.contains("cmd");
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    /// DL3002 for the final stage; call once after the last instruction.
    pub(super) fn finish(&mut self) -> Option<Finding> {
        let (root, span) = self.last_user.take()?;
        root.then(|| Finding {
            code: "DL3002",
            severity: Severity::Warning,
            message: "Last `USER` should not be root; switch to an unprivileged user.".into(),
            span,
        })
    }

    fn check_run(&self, text: &str, span: Span) -> Vec<Finding> {
        if text.trim_start().starts_with("<<") {
            return Vec::new();
        }
        let Script { commands, has_pipe } = shell::parse(text);
        let ctx = RunContext {
            text,
            pip_no_cache_env: self.pip_no_cache_env,
        };
        let mut hits = Vec::new();
        for words in &commands {
            check_command(shell::without_assignments(words), &ctx, &mut hits);
        }
        if has_pipe && !self.pipefail && !self.non_posix_shell && !text.contains("pipefail") {
            hits.push(Hit {
                code: "DL4006",
                severity: Severity::Warning,
                message: "Set the `SHELL` option `-o pipefail` before `RUN` with a pipe in it.".into(),
            });
        }
        let mut seen = Vec::new();
        hits.into_iter()
            .filter(|h| {
                let fresh = !seen.contains(&h.code);
                seen.push(h.code);
                fresh
            })
            .map(|h| Finding::from_hit(h, span))
            .collect()
    }
}

fn check_command(words: &[String], ctx: &RunContext<'_>, hits: &mut Vec<Hit>) {
    let Some((name, args)) = words.split_first() else {
        return;
    };
    match name.as_str() {
        "cd" => hits.push(Hit {
            code: "DL3003",
            severity: Severity::Warning,
            message: "Use `WORKDIR` to switch to a directory instead of `cd`.".into(),
        }),
        "sudo" => hits.push(Hit {
            code: "DL3004",
            severity: Severity::Error,
            message: "Do not use `sudo`; run as the right `USER` or use `gosu`.".into(),
        }),
        "apt" => hits.push(Hit {
            code: "DL3027",
            severity: Severity::Warning,
            message: "Do not use `apt` in scripts; use `apt-get` or `apt-cache`.".into(),
        }),
        "apt-get" => packages::apt_get(args, hits),
        "apk" => packages::apk(args, ctx, hits),
        "npm" => packages::npm(args, hits),
        n if is_pip(n) => packages::pip(args, ctx, hits),
        n if n.starts_with("python")
            && args.first().map(String::as_str) == Some("-m")
            && args.get(1).map(String::as_str) == Some("pip") =>
        {
            packages::pip(&args[2..], ctx, hits);
        }
        _ => {}
    }
}

fn is_pip(name: &str) -> bool {
    name.strip_prefix("pip")
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

fn is_root(user_args: &str) -> bool {
    let user = user_args.trim().split(':').next().unwrap_or_default();
    user == "root" || user == "0"
}

/// DL3020: `ADD` is only for remote URLs and local archives; use `COPY` otherwise.
fn check_add(args: &str, span: Span) -> Vec<Finding> {
    let mut rest = args.trim();
    while rest.starts_with("--") {
        rest = rest.split_once(char::is_whitespace).map_or("", |(_, r)| r.trim_start());
    }
    let words: Vec<String> = if rest.starts_with('[') {
        rest.trim_matches(['[', ']'])
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_owned())
            .collect()
    } else {
        rest.split_whitespace().map(str::to_owned).collect()
    };
    let Some((_, sources)) = words.split_last() else {
        return Vec::new();
    };
    let plain_copy = sources.iter().any(|s| !add_source_is_legitimate(s));
    if !plain_copy {
        return Vec::new();
    }
    vec![Finding {
        code: "DL3020",
        severity: Severity::Error,
        message: "Use `COPY` instead of `ADD` for files and folders.".into(),
        span,
    }]
}

fn add_source_is_legitimate(source: &str) -> bool {
    const ARCHIVES: &[&str] = &[
        ".tar",
        ".tar.gz",
        ".tgz",
        ".tar.bz2",
        ".tbz",
        ".tbz2",
        ".tar.xz",
        ".txz",
        ".tar.zst",
        ".tzst",
        ".tar.lz",
        ".tar.lzma",
        ".gz",
        ".bz2",
        ".xz",
    ];
    let lower = source.to_ascii_lowercase();
    source.contains("://")
        || source.starts_with("git@")
        || source.contains('$')
        || source.starts_with("<<")
        || ARCHIVES.iter().any(|e| lower.ends_with(e))
}

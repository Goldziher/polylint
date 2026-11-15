use std::path::PathBuf;

use super::*;
use crate::config::GlobalDefaults;
use crate::language::Language;

fn cfg() -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options: toml::Table::new(),
    }
}

fn source(content: &str) -> SourceFile {
    SourceFile {
        path: PathBuf::from("script.sh"),
        language: Language::Other("bash".into()),
        content: content.into(),
    }
}

/// The per-line range scans `emit_reindented` used before the row tables:
/// every line re-checks each keep, auto and opener range.
fn reference_emit(
    src: &SourceFile,
    cfg: &EngineConfig,
    grammar: &str,
    adjustments: &Adjustments,
    protected: &[(usize, usize)],
) -> String {
    let Adjustments {
        openers,
        closers,
        auto_ranges,
        keep_ranges,
    } = adjustments;
    let unit = super::super::indent_unit(grammar, cfg.indent_width);
    let line_ending = cfg.globals.line_ending.as_str();

    let mut closer_by_row: HashMap<usize, usize> = HashMap::new();
    for &(row, _) in closers {
        *closer_by_row.entry(row).or_insert(0) += 1;
    }

    let mut out = String::new();
    let mut first = true;
    let mut byte = 0usize;
    for (line_idx, raw) in src.content.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let line_start = byte;
        byte += raw.len() + 1;
        if !first {
            out.push_str(line_ending);
        }
        first = false;
        if is_interior(protected, line_start) {
            out.push_str(line);
            continue;
        }
        if keep_ranges.iter().any(|&(s, e)| s < line_idx && line_idx <= e) {
            out.push_str(line);
            continue;
        }
        if auto_ranges.iter().any(|&(s, e)| s < line_idx && line_idx < e) {
            out.push_str(line);
            continue;
        }
        let opener_count: usize = openers
            .iter()
            .filter(|o| o.start_row < line_idx && line_idx <= o.end_row)
            .count();
        let close_count: usize = closer_by_row.get(&line_idx).copied().unwrap_or(0);
        let level = opener_count.saturating_sub(close_count);
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            for _ in 0..level {
                out.push_str(&unit);
            }
            out.push_str(trimmed);
        }
    }
    super::super::apply_trailing_newline(&mut out, &src.content, line_ending, cfg.globals.final_newline);
    out
}

/// Deterministic xorshift so generated inputs are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A bash script of about `lines` lines mixing nested blocks, comments, strings,
/// heredocs, continuations and ragged indentation.
fn generate_script(seed: u64, lines: usize) -> String {
    let mut rng = Rng(seed | 1);
    let mut out = String::new();
    let mut depth = 0usize;
    let mut n = 0usize;
    while n < lines {
        let pad = " ".repeat(rng.below(9));
        match rng.below(14) {
            0 => {
                out.push_str(&format!("{pad}if [ \"$x{n}\" = 1 ]; then\n"));
                depth += 1;
                n += 1;
            }
            1 => {
                out.push_str(&format!("{pad}for i in 1 2 3; do\n"));
                depth += 1;
                n += 1;
            }
            2 => {
                out.push_str(&format!("{pad}f{n}() {{\n"));
                depth += 1;
                n += 1;
            }
            3 if depth > 0 => {
                let closer = ["fi", "done", "}"][rng.below(3)];
                out.push_str(&format!("{pad}{closer}\n"));
                depth -= 1;
                n += 1;
            }
            4 => {
                out.push_str(&format!("{pad}# comment {n}\n"));
                n += 1;
            }
            5 => {
                out.push_str(&format!("{pad}echo \"multi\n  line {n}\n   string\"\n"));
                n += 3;
            }
            6 => {
                out.push_str(&format!("{pad}cat <<EOT\n  body {n}\n    more\nEOT\n"));
                n += 4;
            }
            7 => {
                out.push_str(&format!("{pad}run a \\\n   --flag {n} \\\n  --other\n"));
                n += 3;
            }
            8 => {
                out.push_str(&format!("{pad}x=$(echo {n} |\n    tr a b)\n"));
                n += 2;
            }
            9 => {
                out.push('\n');
                n += 1;
            }
            _ => {
                out.push_str(&format!("{pad}echo {n}\n"));
                n += 1;
            }
        }
    }
    for _ in 0..depth {
        out.push_str("fi\n");
    }
    out
}

fn assert_matches_reference(script: &str) {
    let src = source(script);
    let query_src = BUILTIN_QUERIES.iter().find(|(n, _)| *n == "bash").unwrap().1;
    let (adjustments, protected) = builtin_facts("bash", query_src, &src)
        .expect("generated script must parse without error nodes and load the bash grammar");
    assert!(!adjustments.openers.is_empty(), "generated script must contain openers");
    assert!(
        !adjustments.keep_ranges.is_empty(),
        "generated script must contain keep ranges"
    );
    assert_eq!(
        emit_reindented(&src, &cfg(), "bash", &adjustments, &protected),
        reference_emit(&src, &cfg(), "bash", &adjustments, &protected),
    );
}

#[test]
fn bash_emit_matches_reference_on_small_generated_scripts() {
    for seed in 1..=8 {
        assert_matches_reference(&generate_script(seed, 300));
    }
}

#[test]
fn bash_emit_matches_reference_on_20k_line_script() {
    assert_matches_reference(&generate_script(0x5eed, 20_000));
}

#[test]
fn emit_matches_reference_on_random_ranges() {
    for seed in 1..=20u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let rows = 1 + rng.below(60);
        let content: String = (0..rows).map(|i| format!("  line {i}\n")).collect();
        let span = |rng: &mut Rng| {
            let start = rng.below(rows + 3);
            (start, start + rng.below(rows))
        };
        let adjustments = Adjustments {
            openers: (0..rng.below(20))
                .map(|_| {
                    let (start_row, end_row) = span(&mut rng);
                    Opener { start_row, end_row }
                })
                .collect(),
            closers: (0..rng.below(20)).map(|_| (rng.below(rows + 3), 0)).collect(),
            auto_ranges: (0..rng.below(8)).map(|_| span(&mut rng)).collect(),
            keep_ranges: (0..rng.below(8)).map(|_| span(&mut rng)).collect(),
        };
        let src = source(&content);
        assert_eq!(
            emit_reindented(&src, &cfg(), "bash", &adjustments, &[]),
            reference_emit(&src, &cfg(), "bash", &adjustments, &[]),
            "seed {seed}"
        );
    }
}

//! Insta snapshot fixture for the Dockerfile linter backend.
//!
//! A known-bad Dockerfile exercises the eight original rules and a second one the hadolint
//! rules added later (DL3002..DL4006), asserting exact (code, severity, line) tuples so any
//! regression in rule logic immediately breaks the snapshot.
//!
//! No known-unformatted fixture is needed: the engine is lint-only and returns
//! [`FormatOutput::Unchanged`] unconditionally.
#![cfg(feature = "dockerfile")]

use poly_engine_api::{
    config::{EngineConfig, GlobalDefaults},
    engine::{Engine, FormatOutput, SourceFile},
    language::Language,
};
use poly_engines_misc::dockerfile::DockerfileEngine;

fn engine_cfg() -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options: toml::Table::new(),
    }
}

fn make_src(content: &str) -> SourceFile {
    SourceFile {
        path: "Dockerfile".into(),
        language: Language::Dockerfile,
        content: content.into(),
    }
}

// ---------------------------------------------------------------------------
// Known-bad fixture
//
// Every line is carefully chosen to trigger exactly one rule:
//
//   Line 1  FROM alpine          → DL3006 (no tag)
//   Line 2  FROM alpine:latest   → DL3007 (:latest)
//   Line 3  RUN apt-get …        → DL3009 (missing cache cleanup)
//                                   + wget/curl sighting for DL4001
//   Line 4  RUN echo hello       → DL3059 (consecutive RUN after line 3)
//   Line 5  MAINTAINER …         → DL4000 (deprecated)
//   Line 6  WORKDIR relative     → DL3000 (relative path)
//   Line 7  CMD echo hello       → DL3025 (shell form)
//   (end)                        → DL4001 (both wget+curl in RUN on line 3)
// ---------------------------------------------------------------------------

const KNOWN_BAD: &str = "\
FROM alpine
FROM alpine:latest
RUN apt-get install -y curl wget
RUN echo hello
MAINTAINER old@example.com
WORKDIR relative_dir
CMD echo hello
";

/// The eight rules this fixture was written for; later rules have their own fixture below.
const LEGACY_RULES: &[&str] = &[
    "DL3000", "DL3006", "DL3007", "DL3009", "DL3025", "DL3059", "DL4000", "DL4001",
];

#[test]
fn known_bad_diagnostics() {
    let engine = DockerfileEngine;
    let src = make_src(KNOWN_BAD);
    let diags: Vec<_> = engine
        .lint(&src, &engine_cfg())
        .unwrap()
        .into_iter()
        .filter(|d| LEGACY_RULES.contains(&d.code.as_deref().unwrap_or("")))
        .collect();

    assert!(!diags.is_empty(), "expected at least one diagnostic");

    // Summarise as (engine, code, severity, start_line) so the snapshot is
    // both human-readable and precisely bounded.
    let summary: Vec<_> = diags
        .iter()
        .map(|d| {
            (
                d.engine.as_str(),
                d.code.as_deref().unwrap_or(""),
                d.severity,
                d.span.map(|s| s.start_line),
            )
        })
        .collect();

    insta::assert_debug_snapshot!("known_bad_diagnostics", summary);
}

// ---------------------------------------------------------------------------
// Clean Dockerfile — zero diagnostics
// ---------------------------------------------------------------------------

const CLEAN: &str = "\
FROM alpine:3.20
RUN apk add --no-cache curl=8.9.1-r2
WORKDIR /app
COPY . .
CMD [\"./my-app\"]
";

#[test]
fn clean_dockerfile_no_diagnostics() {
    let engine = DockerfileEngine;
    let src = make_src(CLEAN);
    let diags = engine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        diags.is_empty(),
        "expected no diagnostics for clean Dockerfile, got: {diags:#?}"
    );
}

// ---------------------------------------------------------------------------
// format() must always return Unchanged (lint-only engine)
// ---------------------------------------------------------------------------

#[test]
fn format_returns_unchanged() {
    let engine = DockerfileEngine;
    let src = make_src(KNOWN_BAD);
    let result = engine.format(&src, &engine_cfg()).unwrap();
    assert!(
        matches!(result, FormatOutput::Unchanged),
        "Dockerfile engine must return Unchanged from format()"
    );
}

// ---------------------------------------------------------------------------
// Individual rule smoke tests — each verifies one rule fires / does not fire
// ---------------------------------------------------------------------------

#[test]
fn dl3006_no_tag_fires() {
    let src = make_src("FROM alpine\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("DL3006")),
        "DL3006 must fire for FROM without tag"
    );
}

#[test]
fn dl3006_with_tag_ok() {
    let src = make_src("FROM alpine:3.20\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        !diags.iter().any(|d| d.code.as_deref() == Some("DL3006")),
        "DL3006 must not fire when tag is present"
    );
}

#[test]
fn dl3007_latest_fires() {
    let src = make_src("FROM alpine:latest\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("DL3007")),
        "DL3007 must fire for :latest tag"
    );
}

#[test]
fn dl3009_apt_no_cleanup_fires() {
    let src = make_src("FROM alpine:3.20\nRUN apt-get install -y curl\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("DL3009")),
        "DL3009 must fire for apt-get without cache cleanup"
    );
}

#[test]
fn dl3009_apt_with_cleanup_ok() {
    let src = make_src("FROM alpine:3.20\nRUN apt-get install -y curl && rm -rf /var/lib/apt/lists/*\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        !diags.iter().any(|d| d.code.as_deref() == Some("DL3009")),
        "DL3009 must not fire when cache is cleaned up"
    );
}

#[test]
fn dl3025_cmd_shell_form_fires() {
    let src = make_src("FROM alpine:3.20\nCMD echo hello\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("DL3025")),
        "DL3025 must fire for CMD in shell form"
    );
}

#[test]
fn dl3059_consecutive_run_fires() {
    let src = make_src("FROM alpine:3.20\nRUN echo a\nRUN echo b\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("DL3059")),
        "DL3059 must fire for consecutive RUN instructions"
    );
}

#[test]
fn dl4000_maintainer_fires() {
    let src = make_src("FROM alpine:3.20\nMAINTAINER dev@example.com\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("DL4000")),
        "DL4000 must fire for MAINTAINER"
    );
}

#[test]
fn dl4001_wget_and_curl_fires() {
    let src = make_src("FROM alpine:3.20\nRUN wget http://a.com && curl http://b.com\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("DL4001")),
        "DL4001 must fire when both wget and curl appear"
    );
}

#[test]
fn dl4001_only_wget_ok() {
    let src = make_src("FROM alpine:3.20\nRUN wget http://a.com\n");
    let diags = DockerfileEngine.lint(&src, &engine_cfg()).unwrap();
    assert!(
        !diags.iter().any(|d| d.code.as_deref() == Some("DL4001")),
        "DL4001 must not fire when only wget is used"
    );
}

// ---------------------------------------------------------------------------
// Hadolint rules added in hadolint-rules-v1
// ---------------------------------------------------------------------------

const KNOWN_BAD_HADOLINT: &str = "\
FROM python:3.12
RUN cd /app && pip install flask
ENV A=1
RUN sudo make install
ENV B=1
RUN apt install -y git
ENV C=1
RUN apt-get install git
ENV D=1
RUN apk add curl
ENV E=1
RUN npm install express
ENV F=1
ADD app.py /app/
ADD https://example.com/a.txt /a
RUN curl -s http://example.com | sh
USER root
";

#[test]
fn known_bad_hadolint_diagnostics() {
    let diags = DockerfileEngine
        .lint(&make_src(KNOWN_BAD_HADOLINT), &engine_cfg())
        .unwrap();
    let summary: Vec<_> = diags
        .iter()
        .map(|d| {
            (
                d.code.as_deref().unwrap_or(""),
                d.severity,
                d.span.map(|s| s.start_line),
            )
        })
        .collect();
    insta::assert_debug_snapshot!("known_bad_hadolint_diagnostics", summary);
}

fn codes(content: &str) -> Vec<String> {
    DockerfileEngine
        .lint(&make_src(content), &engine_cfg())
        .unwrap()
        .into_iter()
        .filter_map(|d| d.code)
        .collect()
}

#[track_caller]
fn assert_fires(content: &str, code: &str) {
    let found = codes(content);
    assert!(
        found.iter().any(|c| c == code),
        "{code} must fire for {content:?}, got {found:?}"
    );
}

#[track_caller]
fn assert_silent(content: &str, code: &str) {
    let found = codes(content);
    assert!(
        !found.iter().any(|c| c == code),
        "{code} must not fire for {content:?}, got {found:?}"
    );
}

#[test]
fn dl3002_last_user_root() {
    assert_fires("FROM a:1\nUSER root\n", "DL3002");
    assert_fires("FROM a:1\nUSER 0:0\n", "DL3002");
    assert_silent("FROM a:1\nUSER root\nUSER app\n", "DL3002");
    assert_silent("FROM a:1\nRUN echo hi\n", "DL3002");
    assert_silent("FROM a:1 AS build\nUSER root\nFROM b:1\nUSER app\n", "DL3002");
}

#[test]
fn dl3003_cd_in_run() {
    assert_fires("FROM a:1\nRUN cd /app && make\n", "DL3003");
    assert_silent("FROM a:1\nRUN echo \"cd /app\"\n", "DL3003");
}

#[test]
fn dl3004_sudo() {
    assert_fires("FROM a:1\nRUN sudo make\n", "DL3004");
    assert_fires("FROM a:1\nRUN make && sudo make install\n", "DL3004");
    assert_silent("FROM a:1\nRUN echo pseudo\n", "DL3004");
}

#[test]
fn apt_get_rules() {
    let bad = "FROM a:1\nRUN apt-get install git\n";
    assert_fires(bad, "DL3008");
    assert_fires(bad, "DL3014");
    assert_fires(bad, "DL3015");
    let good = "FROM a:1\nRUN apt-get install -y --no-install-recommends git=1:2.39.2 && rm -rf /var/lib/apt/lists/*\n";
    for code in ["DL3008", "DL3014", "DL3015"] {
        assert_silent(good, code);
    }
    assert_silent("FROM a:1\nRUN apt-get update\n", "DL3008");
}

#[test]
fn apt_get_rules_span_line_continuations() {
    let good = "FROM a:1\nRUN apt-get update && \\\n    apt-get install -y --no-install-recommends curl=7.88.1 && \\\n    rm -rf /var/lib/apt/lists/*\n";
    for code in ["DL3008", "DL3014", "DL3015"] {
        assert_silent(good, code);
    }
}

#[test]
fn dl3013_and_dl3042_pip() {
    assert_fires("FROM a:1\nRUN pip install flask\n", "DL3013");
    assert_fires("FROM a:1\nRUN python3 -m pip install flask\n", "DL3013");
    assert_silent("FROM a:1\nRUN pip install --no-cache-dir flask==3.0.0\n", "DL3013");
    assert_silent("FROM a:1\nRUN pip install -r requirements.txt\n", "DL3013");
    assert_silent("FROM a:1\nRUN pip install .\n", "DL3013");
    assert_fires("FROM a:1\nRUN pip install flask==3.0.0\n", "DL3042");
    assert_silent("FROM a:1\nRUN pip install --no-cache-dir flask==3.0.0\n", "DL3042");
    assert_silent(
        "FROM a:1\nENV PIP_NO_CACHE_DIR=1\nRUN pip install flask==3.0.0\n",
        "DL3042",
    );
    assert_silent(
        "FROM a:1\nRUN --mount=type=cache,target=/root/.cache/pip pip install flask==3.0.0\n",
        "DL3042",
    );
}

#[test]
fn dl3016_npm() {
    assert_fires("FROM a:1\nRUN npm install express\n", "DL3016");
    assert_fires("FROM a:1\nRUN npm install @scope/pkg\n", "DL3016");
    assert_silent("FROM a:1\nRUN npm install express@4.18.2\n", "DL3016");
    assert_silent("FROM a:1\nRUN npm install @scope/pkg@1.2.3\n", "DL3016");
    assert_silent("FROM a:1\nRUN npm install\n", "DL3016");
}

#[test]
fn apk_rules() {
    assert_fires("FROM a:1\nRUN apk add curl\n", "DL3018");
    assert_fires("FROM a:1\nRUN apk add curl\n", "DL3019");
    assert_silent("FROM a:1\nRUN apk add --no-cache curl=8.9.1-r2\n", "DL3018");
    assert_silent("FROM a:1\nRUN apk add --no-cache curl=8.9.1-r2\n", "DL3019");
    assert_silent(
        "FROM a:1\nRUN apk add --no-cache --virtual .build gcc=13.2.1-r0\n",
        "DL3018",
    );
}

#[test]
fn dl3020_add() {
    assert_fires("FROM a:1\nADD app.py /app/\n", "DL3020");
    assert_fires("FROM a:1\nADD --chown=1:1 src/ /app/\n", "DL3020");
    assert_silent("FROM a:1\nADD https://example.com/a.txt /a\n", "DL3020");
    assert_silent("FROM a:1\nADD rootfs.tar.gz /\n", "DL3020");
    assert_silent("FROM a:1\nCOPY app.py /app/\n", "DL3020");
}

#[test]
fn dl3027_apt() {
    assert_fires("FROM a:1\nRUN apt install -y git\n", "DL3027");
    assert_silent("FROM a:1\nRUN apt-get install -y git\n", "DL3027");
}

#[test]
fn dl4006_pipefail() {
    assert_fires("FROM a:1\nRUN curl -s http://x | sh\n", "DL4006");
    assert_silent("FROM a:1\nRUN curl -s http://x || true\n", "DL4006");
    assert_silent("FROM a:1\nRUN set -o pipefail && curl -s http://x | sh\n", "DL4006");
    assert_silent(
        "FROM a:1\nSHELL [\"/bin/bash\", \"-o\", \"pipefail\", \"-c\"]\nRUN curl -s http://x | sh\n",
        "DL4006",
    );
    assert_silent("FROM a:1\nRUN [\"sh\", \"-c\", \"a | b\"]\n", "DL4006");
}

#[test]
fn hadolint_rules_honor_ignore_and_level() {
    let src = "FROM a:1\nRUN sudo make\n";
    let cfg = |toml_str: &str| EngineConfig {
        options: toml::from_str(toml_str).unwrap(),
        ..engine_cfg()
    };
    let ignored = DockerfileEngine
        .lint(&make_src(src), &cfg("ignore = [\"DL3004\"]"))
        .unwrap();
    assert!(
        ignored.iter().all(|d| d.code.as_deref() != Some("DL3004")),
        "{ignored:?}"
    );
    let leveled = DockerfileEngine
        .lint(&make_src(src), &cfg("[rules.DL3004]\nlevel = \"warning\""))
        .unwrap();
    let d = leveled.iter().find(|d| d.code.as_deref() == Some("DL3004")).unwrap();
    assert_eq!(d.severity, poly_engine_api::engine::Severity::Warning);
}

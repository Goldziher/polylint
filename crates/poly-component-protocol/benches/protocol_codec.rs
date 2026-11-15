//! MessagePack codec cost of the component protocol: encoding a request header and decoding
//! responses that carry 0, 10 and 1000 diagnostics.
//!
//! Run with: `cargo bench -p poly-component-protocol --bench protocol_codec`.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use poly_component_protocol::{
    Payload, Request, Response, WireDiagnostic, WireFile, WireSeverity, WireSpan, decode_request, decode_response,
    encode_request, encode_response,
};

const DIAGNOSTIC_COUNTS: [usize; 3] = [0, 10, 1000];

fn diagnostic(i: usize) -> WireDiagnostic {
    WireDiagnostic {
        engine: "ruff".to_owned(),
        code: Some(format!("E{:03}", i % 999)),
        severity: WireSeverity::Warning,
        title: format!("unused variable `value_{i}`"),
        description: Some("the binding is never read after its definition".to_owned()),
        span: Some(WireSpan {
            start_line: u32::try_from(i).unwrap_or(u32::MAX),
            start_col: 4,
            end_line: u32::try_from(i).unwrap_or(u32::MAX),
            end_col: 20,
        }),
        url: Some("https://example.invalid/rules/unused".to_owned()),
        fix: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

fn response_with(count: usize) -> Response {
    Response {
        result: Ok(Payload::Diagnostics((0..count).map(diagnostic).collect())),
        logs: Vec::new(),
    }
}

fn bench_codec(c: &mut Criterion) {
    let request = Request::Lint {
        engine: "ruff".to_owned(),
        config: 1,
        file: WireFile {
            path: "src/some/module/file.py".to_owned(),
            language: "python".to_owned(),
            len: 4096,
        },
    };
    let encoded = encode_request(&request).expect("encode request");

    let mut group = c.benchmark_group("protocol_codec");
    group.bench_function("encode_request", |b| {
        b.iter(|| encode_request(black_box(&request)).expect("encode"));
    });
    group.bench_function("decode_request", |b| {
        b.iter(|| decode_request(black_box(&encoded)).expect("decode"));
    });

    for count in DIAGNOSTIC_COUNTS {
        let response = response_with(count);
        let bytes = encode_response(&response).expect("encode response");
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_with_input(BenchmarkId::new("encode_response", count), &response, |b, response| {
            b.iter(|| encode_response(black_box(response)).expect("encode"));
        });
        group.bench_with_input(BenchmarkId::new("decode_response", count), &bytes, |b, bytes| {
            b.iter(|| decode_response(black_box(bytes)).expect("decode"));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_codec);
criterion_main!(benches);

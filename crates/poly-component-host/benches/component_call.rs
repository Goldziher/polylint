//! Host-side overhead of a component call: `Connection::call` over an in-memory echo transport
//! (header encode, transport hop, response decode, config registration lookup) with 1 KiB, 64 KiB
//! and 1 MiB bodies, driven from 1 and 8 rayon threads, plus `EngineConfig::fingerprint` with 0, 10
//! and 100 options.
//!
//! The transport answers `Format` by echoing the body, so the numbers are the host's cost with the
//! component's own work taken out. Run with:
//! `cargo bench -p poly-component-host --bench component_call`.

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use poly_component_host::{ComponentSlot, ComponentTransport, Connection, InitParams};
use poly_component_protocol::{
    Describe, PROTOCOL_VERSION, Payload, Request, Response, WireFile, WireFormatOutput, decode_request,
    encode_describe, encode_response,
};
use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use rayon::prelude::*;

const BODY_SIZES: [usize; 3] = [1024, 64 * 1024, 1024 * 1024];
const CALLS_PER_ITER: usize = 64;

struct EchoTransport;

impl ComponentTransport for EchoTransport {
    fn describe(&self) -> anyhow::Result<Vec<u8>> {
        Ok(encode_describe(&Describe {
            protocol: PROTOCOL_VERSION,
            component_version: "0.0.0".to_owned(),
            engines: Vec::new(),
        })?)
    }

    fn call(&self, header: &[u8], body: &[u8]) -> anyhow::Result<Vec<u8>> {
        let payload = match decode_request(header)? {
            Request::Init { .. } => Payload::Init {
                protocol: PROTOCOL_VERSION,
                component_version: "0.0.0".to_owned(),
            },
            Request::Format { .. } => {
                Payload::Formatted(WireFormatOutput::Formatted(String::from_utf8_lossy(body).into_owned()))
            }
            _ => Payload::Ok,
        };
        Ok(encode_response(&Response {
            result: Ok(payload),
            logs: Vec::new(),
        })?)
    }
}

fn config_with(options: usize) -> EngineConfig {
    let mut table = toml::Table::new();
    for i in 0..options {
        table.insert(format!("option_{i}"), toml::Value::String(format!("value-{i}")));
    }
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options: table,
    }
}

fn format_call(connection: &Connection, cfg: &EngineConfig, body: &[u8]) {
    let payload = connection
        .call(
            "echo",
            cfg,
            |config| Request::Format {
                engine: "echo".to_owned(),
                config,
                file: WireFile {
                    path: "a.txt".to_owned(),
                    language: "text".to_owned(),
                    len: body.len() as u64,
                },
            },
            body,
        )
        .expect("echo call");
    black_box(payload);
}

fn bench_call(c: &mut Criterion) {
    let slot = ComponentSlot::from_transport("echo", None, InitParams::default(), Arc::new(EchoTransport));
    let connection = slot.connection().expect("echo connection");
    let cfg = config_with(0);

    let mut group = c.benchmark_group("component_call");
    for size in BODY_SIZES {
        let body = vec![b'x'; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("single", size), &body, |b, body| {
            b.iter(|| format_call(&connection, &cfg, body));
        });
        for threads in [1usize, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("rayon pool");
            group.throughput(Throughput::Bytes((size * CALLS_PER_ITER) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("batch_{CALLS_PER_ITER}_threads_{threads}"), size),
                &body,
                |b, body| {
                    b.iter(|| {
                        pool.install(|| {
                            (0..CALLS_PER_ITER)
                                .into_par_iter()
                                .for_each(|_| format_call(&connection, &cfg, body));
                        });
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_fingerprint(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_config_fingerprint");
    for options in [0usize, 10, 100] {
        let cfg = config_with(options);
        group.bench_with_input(BenchmarkId::from_parameter(options), &cfg, |b, cfg| {
            b.iter(|| black_box(cfg.fingerprint()));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_call, bench_fingerprint);
criterion_main!(benches);

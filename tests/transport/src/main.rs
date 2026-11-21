// SPDX-License-Identifier: Apache-2.0
use antalya_rs::Format;
use serde_json::json;
use std::io::{self, Write};
use wildcard_transport_format::{Bench, Error, Limits, Result, Scenario};

const VERSION: &str = "26.6.8.20004.altinityantalya";
const IMAGE: &str = "altinity/clickhouse-server@sha256:aec9534624af9142abe87aab1b51ff5bcca9e74ca3b7e58fead2c1ec5e157cdf";
fn emit(value: serde_json::Value) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &value).map_err(|_| Error::Io("report output failed"))?;
    writeln!(out).map_err(|_| Error::Io("report output failed"))
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(message) = run().await {
        let _ = writeln!(io::stderr().lock(), "benchmark failed: {message}");
        std::process::exit(1)
    }
}
async fn run() -> Result<()> {
    let url = std::env::var("ANTALYA_TEST_URL")
        .map_err(|_| Error::Configuration("ANTALYA_TEST_URL required"))?;
    let token = std::env::var("ANTALYA_TEST_TOKEN")
        .map_err(|_| Error::Configuration("ANTALYA_TEST_TOKEN required"))?;
    let samples = std::env::var("BENCH_WARM_SAMPLES")
        .unwrap_or_else(|_| "6".into())
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 2 && *n <= 50)
        .ok_or(Error::Configuration("BENCH_WARM_SAMPLES must be 2..50"))?;
    let limits = Limits::default();
    let bench = Bench::new(&url, &token, limits)?;
    let identity = bench.identity().await?;
    if identity["version"] != VERSION {
        return Err(Error::InvalidData(
            "server version is not the pinned fixture version",
        ));
    }
    emit(
        json!({"kind":"metadata","benchmark":"wildcard-transport-format/v1","scope":"direct engine HTTP; LO controls/service, Python and configured upstream Arrow transport excluded","identity":identity,"fixture_image":IMAGE,"fixture_image_note":"expected image; fixture.py verifies version and launches exact digest; remote image digest is not observable over SQL","limits":limits,"http":"HTTP/1.1; identity compression; one reqwest0.13.5 client/pool; no proxy/redirect/retry","arrow":"59.3.2","cache":"use_query_cache=0; other server/OS caches uncontrolled; validation requests precede timed requests","corpus":"immutable numbers(32768), generated UInt64 + Nullable(String) formula v1; count/page32/bulk32768; SQL and parameter SHA256 per record; no index or pruning claim","connection_reuse":"deterministic HTTP fixture observes one accepted socket; live reqwest does not expose connection identity, so live reuse is unobservable","null_metrics":{"physical_io_bytes":"not observed; read_bytes is logical engine accounting","candidates":"not exposed by this fixture","selected_marks_parts":"null if ProfileEvents key absent","thread_cpu":"unavailable in safe-only harness; null, not zero"},"timing":"download buffered before decode; headers/first-byte/request_body are nested origin-relative clocks; body_after_headers, decode, consumer and completion are separate phases; total includes construction/validation of framing and local overhead; do not add nested clocks or subtract engine time to infer pure transport","warm_samples_per_format_per_scenario":samples}),
    )?;
    let scenarios = Scenario::all();
    // Full fixture qualification for every format/scenario before timed samples.
    for scenario in &scenarios {
        for format in [Format::Json, Format::ArrowStream] {
            let (decoded, sample) = bench.measure(scenario, format).await?;
            bench.validate(scenario, &decoded)?;
            let (sql_sha256, parameters_sha256) = scenario.fingerprints()?;
            emit(
                json!({"kind":"qualification","scenario":scenario,"sql_sha256":sql_sha256,"parameters_sha256":parameters_sha256,"full_expected_values_equal":true,"sample":sample}),
            )?;
        }
    }
    for scenario in &scenarios {
        let (sql_sha256, parameters_sha256) = scenario.fingerprints()?;
        for iteration in 0..samples {
            let order = if iteration % 2 == 0 {
                [Format::Json, Format::ArrowStream]
            } else {
                [Format::ArrowStream, Format::Json]
            };
            for (position, format) in order.into_iter().enumerate() {
                let (decoded, sample) = bench.measure(scenario, format).await?;
                // Outside all recorded clocks; validation never enters Arrow's timed consumer.
                bench.validate(scenario, &decoded)?;
                emit(
                    json!({"kind":"sample","scenario":scenario,"iteration":iteration,"order_in_pair":position,"connection_state":"after qualification; actual live socket reuse unobservable","sql_sha256":sql_sha256,"parameters_sha256":parameters_sha256,"full_expected_values_equal":true,"sample":sample}),
                )?;
            }
        }
    }
    emit(
        json!({"kind":"complete","qualified_formats":2,"qualified_scenarios":3,"raw_timed_samples":samples*2*3,"latency_claim":null,"reason":"raw qualification, no percentile or cross-service ranking"}),
    )
}

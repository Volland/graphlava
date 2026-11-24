//! HDR histograms, their serialized form, and the median-with-bootstrap-CI
//! summary over runs ([[17-benchmarks#Harness and method]]).

use base64::Engine as _;
use hdrhistogram::serialization::{Deserializer, Serializer as _, V2DeflateSerializer};
use hdrhistogram::Histogram;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Latencies in nanoseconds, 1 ns .. 10 min, 3 significant digits.
pub fn new_hist() -> Histogram<u64> {
    Histogram::<u64>::new_with_bounds(1, 600_000_000_000, 3).expect("histogram bounds")
}

/// Records a latency, clamping to the histogram range instead of failing.
pub fn record(h: &mut Histogram<u64>, ns: u64) {
    let v = ns.clamp(1, h.high());
    h.record(v).expect("clamped value");
}

/// Base64 of the V2 deflate encoding (the HdrHistogram interchange format;
/// `HistogramLogProcessor` and every HdrHistogram port can read it).
pub fn hist_b64(h: &Histogram<u64>) -> String {
    let mut v = Vec::new();
    V2DeflateSerializer::new()
        .serialize(h, &mut v)
        .expect("serialize histogram");
    base64::engine::general_purpose::STANDARD.encode(v)
}

#[allow(dead_code)] // for result readers and tests
pub fn hist_from_b64(s: &str) -> anyhow::Result<Histogram<u64>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s)?;
    Ok(Deserializer::new().deserialize(&mut &bytes[..])?)
}

/// Summary in microseconds plus the raw histogram.
pub fn hist_json(h: &Histogram<u64>) -> Value {
    let us = |ns: u64| ns as f64 / 1000.0;
    json!({
        "count": h.len(),
        "p50_us": us(h.value_at_quantile(0.50)),
        "p90_us": us(h.value_at_quantile(0.90)),
        "p99_us": us(h.value_at_quantile(0.99)),
        "p999_us": us(h.value_at_quantile(0.999)),
        "max_us": us(h.max()),
        "mean_us": h.mean() / 1000.0,
        "hdr_v2_deflate_b64": hist_b64(h),
    })
}

/// A named set of histograms (one per operation class).
#[derive(Debug, Default)]
pub struct Hists {
    pub by_class: BTreeMap<String, Histogram<u64>>,
}

impl Hists {
    pub fn rec(&mut self, class: &str, ns: u64) {
        let h = self
            .by_class
            .entry(class.to_string())
            .or_insert_with(new_hist);
        record(h, ns);
    }
    pub fn merge(&mut self, o: &Hists) {
        for (k, h) in &o.by_class {
            self.by_class
                .entry(k.clone())
                .or_insert_with(new_hist)
                .add(h)
                .expect("same bounds");
        }
    }
    pub fn all(&self) -> Histogram<u64> {
        let mut a = new_hist();
        for h in self.by_class.values() {
            a.add(h).expect("same bounds");
        }
        a
    }
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        for (k, h) in &self.by_class {
            m.insert(k.clone(), hist_json(h));
        }
        m.insert("all".into(), hist_json(&self.all()));
        Value::Object(m)
    }
}

/// Median of a sample (mean of the two middle values for an even count).
pub fn median(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Percentile bootstrap 95% CI of the median: 10 000 resamples with
/// replacement, fixed seed so the summary is reproducible from the runs.
pub fn bootstrap_ci(xs: &[f64]) -> (f64, f64) {
    if xs.len() < 2 {
        let m = median(xs);
        return (m, m);
    }
    let mut rng = SmallRng::seed_from_u64(0x5EED_B007);
    let b = 10_000;
    let mut meds = Vec::with_capacity(b);
    let mut s = vec![0.0; xs.len()];
    for _ in 0..b {
        for x in s.iter_mut() {
            *x = xs[rng.gen_range(0..xs.len())];
        }
        meds.push(median(&s));
    }
    meds.sort_by(|a, b| a.total_cmp(b));
    (meds[b * 25 / 1000], meds[b * 975 / 1000 - 1])
}

pub fn summarize(xs: &[f64]) -> Value {
    let (lo, hi) = bootstrap_ci(xs);
    json!({"median": median(xs), "ci95": [lo, hi], "n": xs.len(), "runs": xs})
}

/// Collects one scalar per run under a metric path, then summarizes each.
#[derive(Debug, Default)]
pub struct RunSeries {
    pub metrics: BTreeMap<String, Vec<f64>>,
}

impl RunSeries {
    pub fn push(&mut self, k: impl Into<String>, v: f64) {
        self.metrics.entry(k.into()).or_default().push(v);
    }
    /// Adds p50/p99/p99.9 of every class of `h` and the throughput.
    pub fn push_hists(&mut self, h: &Hists) {
        for (k, hh) in h.by_class.iter().chain(std::iter::once((&"all".to_string(), &h.all()))) {
            if hh.is_empty() {
                continue;
            }
            self.push(format!("{k}.p50_us"), hh.value_at_quantile(0.5) as f64 / 1e3);
            self.push(format!("{k}.p99_us"), hh.value_at_quantile(0.99) as f64 / 1e3);
            self.push(format!("{k}.p999_us"), hh.value_at_quantile(0.999) as f64 / 1e3);
        }
    }
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        for (k, v) in &self.metrics {
            m.insert(k.clone(), summarize(v));
        }
        Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_and_ci() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
        let (lo, hi) = bootstrap_ci(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(lo <= 3.0 && hi >= 3.0 && lo >= 1.0 && hi <= 5.0);
    }

    #[test]
    fn hist_roundtrip() {
        let mut h = new_hist();
        for i in 1..1000 {
            record(&mut h, i * 1000);
        }
        let back = hist_from_b64(&hist_b64(&h)).unwrap();
        assert_eq!(back.len(), h.len());
        assert_eq!(back.value_at_quantile(0.99), h.value_at_quantile(0.99));
    }
}

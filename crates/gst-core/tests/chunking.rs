//! Chunk splitting driven through the real pipeline.
//!
//! The demo workbook fits the limit, pinning the single-file path against the
//! same build the golden test verifies byte-for-byte. The split path is then
//! exercised by inflating the b2b section in memory — cloned real envelopes,
//! no committed oversized fixture — and checking the part invariants: every
//! part fits the reference's measure, parses, carries the identical header,
//! and the parts' union is exactly the unsplit file.

mod common;

use gst_core::upload::{self, Turnover};

#[test]
fn the_demo_workbook_fits_in_a_single_part() {
    let ctx = common::ctx(6, 2025);
    let run = common::clean_run("fixtures/gstr1/demo-workbook.xlsx", &ctx);
    let chunked = run.chunks(&ctx, Turnover::default()).expect("chunks");
    assert_eq!(chunked.bodies.len(), 1);
    assert_eq!(
        chunked.bodies[0],
        run.build(&ctx, Turnover::default()).to_json()
    );
}

#[test]
fn an_inflated_workbook_splits_into_valid_parts() {
    let ctx = common::ctx(6, 2025);
    let mut run = common::clean_run("fixtures/gstr1/demo-workbook.xlsx", &ctx);

    // Double the b2b section's (envelopes, members) in lockstep until the
    // section alone is comfortably past the ~4.9 MB measured limit.
    let b2b = run.sections.get_mut("b2b").expect("demo has b2b");
    while b2b
        .envelopes
        .iter()
        .map(|e| e.to_json().len())
        .sum::<usize>()
        < 6_000_000
    {
        b2b.envelopes.extend_from_slice(&b2b.envelopes.clone());
        b2b.members.extend_from_slice(&b2b.members.clone());
    }

    let chunked = run.chunks(&ctx, Turnover::default()).expect("chunks");
    assert!(
        chunked.bodies.len() >= 2,
        "{} part(s)",
        chunked.bodies.len()
    );
    assert!(chunked.unsplit_measure > upload::max_chunk_bytes());

    let whole: serde_json::Value =
        serde_json::from_str(&run.build(&ctx, Turnover::default()).to_json()).unwrap();
    let mut merged: Option<serde_json::Value> = None;
    let mut first_header: Option<serde_json::Value> = None;
    for body in &chunked.bodies {
        assert!(upload::reference_size(body) <= upload::max_chunk_bytes());
        let part: serde_json::Value = serde_json::from_str(body).expect("part parses");

        // Identical full header on every part.
        let header: serde_json::Value = serde_json::json!({
            "gstin": part["gstin"], "fp": part["fp"],
            "version": part["version"], "hash": part["hash"],
        });
        assert!(header.as_object().unwrap().values().all(|v| !v.is_null()));
        match &first_header {
            None => first_header = Some(header),
            Some(first) => assert_eq!(first, &header),
        }

        merged = Some(match merged.take() {
            None => part,
            Some(mut acc) => {
                merge(&mut acc, &part);
                acc
            }
        });
    }
    assert_eq!(merged.unwrap(), whole);
}

/// Merge one part into the accumulator: arrays concatenate, nested objects
/// merge member-wise, header scalars must agree.
fn merge(acc: &mut serde_json::Value, add: &serde_json::Value) {
    match (acc, add) {
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            a.extend(b.iter().cloned());
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(k) {
                    None => {
                        a.insert(k.clone(), v.clone());
                    }
                    Some(e) => merge(e, v),
                }
            }
        }
        (a, b) => assert_eq!(*a, *b, "scalar conflict across parts"),
    }
}

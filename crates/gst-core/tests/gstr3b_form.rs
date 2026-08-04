//! GSTR-3B end to end: read the plain-value fixture workbook, judge it, and
//! pin the generated payload. The expected file is OUR clean JSON — the
//! utility's own output can only come from real Excel running its VBA, so the
//! golden README marks this PENDING ORACLE VERIFICATION until a captured file
//! is compared semantically (payload::parse both sides).

mod common;

use gst_core::gstr3b;
use gst_core::payload;
use gst_core::spec::Severity;

fn form() -> gstr3b::FormData {
    gstr3b::read(&common::repo_path("fixtures/gstr3b/form-workbook.xlsx")).expect("fixture reads")
}

#[test]
fn the_fixture_reads_completely() {
    let form = form();
    assert_eq!(form.gstin, "27AAPFU0939F1ZV");
    assert_eq!(form.legal_name, "Test Traders Pvt Ltd");
    assert_eq!(form.fy, "2024-25");
    assert_eq!(form.month, "January");
    assert_eq!(form.period.unwrap().as_mmyyyy(), "012025");
    // Formula cells arrive as cached values.
    assert_eq!(form.record.text("osup_det_samt"), "25000");
    // The negative 3.1(c) value survives reading.
    assert_eq!(form.record.text("osup_nil_exmp_txval"), "-15000");
    // 3.2: exactly the four filled rows.
    assert_eq!(form.pos_rows.len(), 4);
    assert_eq!(form.pos_rows[0].pos, "06-Haryana");
}

#[test]
fn the_fixture_validates_with_only_the_known_warnings() {
    let findings = gstr3b::validate(&form());
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    // E38 is a genuine input (no mirror warning), the cached itc_net matches,
    // and the GSTIN checksum passes — so a clean fixture has no warnings.
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn negatives_are_rejected_before_their_windows() {
    // The same values filed for June 2024 (before the September 2024 outward
    // window): the negative 3.1(c) must fail; and for June 2022 the ITC
    // negative must fail too.
    let mut early = form();
    early.period = gst_core::date::ReturnPeriod::new(6, 2024);
    let findings = gstr3b::validate(&early);
    assert!(
        findings.iter().any(|f| {
            f.rule.as_deref() == Some("gstr3b.amount_valid") && f.column.as_deref() == Some("C13")
        }),
        "{findings:?}"
    );
    // ITC negatives were already live in June 2024 (window opens 012023).
    assert!(
        !findings.iter().any(|f| f.column.as_deref() == Some("C35")),
        "{findings:?}"
    );

    let mut very_early = form();
    very_early.period = gst_core::date::ReturnPeriod::new(6, 2022);
    let findings = gstr3b::validate(&very_early);
    assert!(
        findings.iter().any(|f| f.column.as_deref() == Some("C35")),
        "{findings:?}"
    );
}

#[test]
fn structural_32_findings_fire() {
    let mut broken = form();
    // POS with no amounts.
    broken.pos_rows[0].record.values.clear();
    // Amounts with no POS.
    broken.pos_rows[1].pos = String::new();
    // Duplicate POS.
    broken.pos_rows[3].pos = "32-Kerala".to_owned();
    let findings = gstr3b::validate(&broken);
    for rule in [
        "gstr3b.pos_without_amounts",
        "gstr3b.amounts_without_pos",
        "gstr3b.pos_duplicate",
    ] {
        assert!(
            findings.iter().any(|f| f.rule.as_deref() == Some(rule)),
            "{rule} missing: {findings:?}"
        );
    }
}

#[test]
fn the_igst_cap_rule_fires_and_respects_its_gates() {
    let mut over = form();
    // Push 3.2 IGST beyond D11 + D22 = 68000.
    over.pos_rows[0].record.values.insert(
        "unreg_iamt".into(),
        gst_core::record::Cell::Number(70000.into()),
    );
    let findings = gstr3b::validate(&over);
    assert!(
        findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("gstr3b.inter_sup_igst_within_31")),
        "{findings:?}"
    );

    // From November 2025 the rule is skipped entirely.
    let mut late = over.clone();
    late.period = gst_core::date::ReturnPeriod::new(11, 2025);
    let findings = gstr3b::validate(&late);
    assert!(
        !findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("gstr3b.inter_sup_igst_within_31")),
        "{findings:?}"
    );
}

#[test]
fn broken_mirror_formulas_warn_but_export_verbatim() {
    let mut broken = form();
    broken.record.values.insert(
        "osup_det_samt".into(),
        gst_core::record::Cell::Number(999.into()),
    );
    let findings = gstr3b::validate(&broken);
    assert!(
        findings.iter().any(|f| {
            f.rule.as_deref() == Some("gstr3b.sgst_mirrors_cgst") && f.severity == Severity::Warning
        }),
        "{findings:?}"
    );
    let payload = gstr3b::generate(&broken, broken.period.unwrap());
    assert_eq!(
        payload.get("sup_details.osup_det.samt").unwrap().to_json(),
        "999"
    );
}

#[test]
fn the_payload_matches_the_expected_golden() {
    let form = form();
    let ours = gstr3b::generate(&form, form.period.unwrap()).to_json();
    let expected = std::fs::read_to_string(common::repo_path(
        "fixtures/golden/gstr3b-012025-expected.json",
    ))
    .expect("expected golden present — regenerate with the ignored test below");
    assert_eq!(ours, expected.trim_end());
    // And the payload is valid JSON by our own parser (roundtrip).
    assert_eq!(payload::parse(&ours).unwrap().to_json(), ours);
}

#[test]
fn emission_gates_shape_the_payload() {
    let form = form();
    // Pre-July-2022: no eco_dtls block at all.
    let early = gstr3b::generate(&form, gst_core::date::ReturnPeriod::new(6, 2022).unwrap());
    assert!(early.get("eco_dtls").is_none());
    assert!(early.get("inter_sup").is_some());
    // From November 2025: inter_sup is gone.
    let late = gstr3b::generate(&form, gst_core::date::ReturnPeriod::new(11, 2025).unwrap());
    assert!(late.get("eco_dtls").is_some());
    assert!(late.get("inter_sup").is_none());
}

#[test]
fn targeted_payload_semantics() {
    let form = form();
    let payload = gstr3b::generate(&form, form.period.unwrap());
    // Excel rounding on the double: 500000.555 is 500000.55499…, so .55.
    assert_eq!(
        payload.get("sup_details.osup_det.txval").unwrap().to_json(),
        "500000.55"
    );
    // 100.005's double sits below the midpoint: 100.
    assert_eq!(
        payload
            .get("intr_ltfee.intr_details.iamt")
            .unwrap()
            .to_json(),
        "100"
    );
    // The late fee is UNROUNDED and samt is omitted.
    assert_eq!(
        payload.get("intr_ltfee.ltfee_details").unwrap().to_json(),
        r#"{"camt":125.456}"#
    );
    // IMPG carries camt/samt as zeros — every ty row emits all four keys.
    assert_eq!(
        payload.get("itc_elg.itc_avl").unwrap().to_json(),
        r#"[{"ty":"IMPG","iamt":5000,"camt":0,"samt":0,"csamt":100},{"ty":"IMPS","iamt":2000,"camt":0,"samt":0,"csamt":0},{"ty":"ISRC","iamt":8000,"camt":1500,"samt":1500,"csamt":200},{"ty":"ISD","iamt":1000,"camt":500,"samt":500,"csamt":0},{"ty":"OTH","iamt":-3000,"camt":2500,"samt":2500,"csamt":300}]"#
    );
    // itc_net is the computed 4(C) formula.
    assert_eq!(
        payload.get("itc_elg.itc_net").unwrap().to_json(),
        r#"{"iamt":11500,"camt":4200,"samt":4210,"csamt":550}"#
    );
    // 3.2: pos as two-digit strings; blank pair members coerced to 0; the
    // West Bengal row appears in two sub-arrays.
    assert_eq!(
        payload.get("inter_sup.comp_details").unwrap().to_json(),
        r#"[{"pos":"09","txval":5000,"iamt":0}]"#
    );
    assert_eq!(
        payload.get("inter_sup.uin_details").unwrap().to_json(),
        r#"[{"pos":"32","txval":2000,"iamt":360},{"pos":"19","txval":500,"iamt":90}]"#
    );
    // inward_sup always carries both rows with both keys.
    assert_eq!(
        payload.get("inward_sup.isup_details").unwrap().to_json(),
        r#"[{"ty":"GST","inter":12000,"intra":6000},{"ty":"NONGST","inter":3000,"intra":1500}]"#
    );
    assert_eq!(
        gstr3b::filename(&form),
        "January_2024-GSTR3B27AAPFU0939F1ZV-Details.json"
    );
}

/// Regenerate the expected golden after a deliberate semantic change:
/// `cargo test -p gst-core --test gstr3b_form regenerate -- --ignored`
#[test]
#[ignore = "writes fixtures/golden/gstr3b-012025-expected.json"]
fn regenerate_expected_golden() {
    let form = form();
    let ours = gstr3b::generate(&form, form.period.unwrap()).to_json();
    std::fs::write(
        common::repo_path("fixtures/golden/gstr3b-012025-expected.json"),
        ours,
    )
    .expect("golden written");
}

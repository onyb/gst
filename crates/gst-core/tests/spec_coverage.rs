//! Guards that hold for every registered section at once, so a new section is
//! covered the moment it lands in the registry.

use gst_core::generate::unimplemented_derivations;
use gst_core::spec;
use gst_core::summary;

#[test]
fn every_derivation_every_spec_names_is_implemented() {
    for section in spec::sections() {
        let missing = unimplemented_derivations(section);
        assert!(
            missing.is_empty(),
            "{} names unimplemented derivation(s): {missing:?}",
            section.section
        );
    }
}

#[test]
fn every_registered_section_has_a_summary_decision() {
    // Summed or explicitly excluded — a new section cannot land without the
    // summary spec saying which.
    let covered = summary::covered_sections();
    for section in spec::sections() {
        assert!(
            covered.iter().any(|(cd, _)| *cd == section.section),
            "{} has no row in spec/gstr1/summary.json",
            section.section
        );
    }
}

#[test]
fn every_summary_row_names_a_registered_section_or_the_inert_merged_hsn() {
    // "hsn" is the pre-bifurcation merged row: declared for fidelity to the
    // reference's list, but no section registers that code, so it never fires.
    for (cd, _) in summary::covered_sections() {
        assert!(
            cd == "hsn" || spec::section(cd).is_some(),
            "summary row '{cd}' names no registered section"
        );
    }
}

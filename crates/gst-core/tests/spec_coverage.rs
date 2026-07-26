//! Guards that hold for every registered section at once, so a new section is
//! covered the moment it lands in the registry.

use gst_core::generate::unimplemented_derivations;
use gst_core::spec;

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

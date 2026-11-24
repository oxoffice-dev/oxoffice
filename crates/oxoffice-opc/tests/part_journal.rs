//! A part journal puts back what a call wrote: each part's bytes before its
//! first write, or its absence.

use oxoffice_opc::OpcPackage;

#[test]
fn undoing_a_part_journal_puts_back_written_added_and_removed_parts() {
    let mut package = OpcPackage::new();
    package.set_part("/kept.xml", b"kept".to_vec());
    package.set_part("/written.xml", b"before".to_vec());
    package.set_part("/removed.xml", b"removed".to_vec());
    let before = package.clone();

    package.open_part_journal();
    package.set_part("/written.xml", b"first".to_vec());
    package.set_part("/written.xml", b"second".to_vec());
    package.set_part("/added.xml", b"added".to_vec());
    package.remove_part("/removed.xml");
    let journal = package.close_part_journal();
    let mut written = journal.written_parts().collect::<Vec<_>>();
    written.sort_unstable();
    assert_eq!(written, ["/added.xml", "/removed.xml", "/written.xml"]);
    assert!(!journal.wrote("/kept.xml"));

    package.undo_part_journal(journal);
    assert!(package.same_package_as(&before));
    assert_eq!(package.get_part("/written.xml"), Some(&b"before"[..]));
    assert_eq!(package.get_part("/added.xml"), None);
}

#[test]
fn a_write_after_the_journal_closes_is_not_recorded() {
    let mut package = OpcPackage::new();
    package.open_part_journal();
    let journal = package.close_part_journal();
    package.set_part("/late.xml", b"late".to_vec());
    assert_eq!(journal.written_parts().count(), 0);
    package.undo_part_journal(journal);
    assert_eq!(package.get_part("/late.xml"), Some(&b"late"[..]));
}

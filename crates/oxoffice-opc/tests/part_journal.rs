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

/// Relationships of `/owner.xml` with these ids, each a hyperlink to a page
/// named after it.
fn package_with_relationships(ids: &[&str]) -> OpcPackage {
    let mut package = OpcPackage::new();
    let relationships = package.get_or_create_part_rels("/owner.xml");
    for id in ids {
        relationships.add_with_id(id, "urn:test:link", &format!("{id}.xml"));
    }
    package
}

fn ids(package: &OpcPackage) -> Vec<String> {
    package
        .get_part_rels("/owner.xml")
        .expect("the owner keeps its relationships")
        .items
        .iter()
        .map(|item| item.id.clone())
        .collect()
}

#[test]
fn relationships_removed_one_by_one_go_back_in_place_on_undo() {
    let mut package = package_with_relationships(&["rId1", "rId2", "rId3", "rId4", "rId5"]);
    let before = package.clone();

    package.open_part_journal();
    let removed = package.remove_relationships_where("/owner.xml", |item| {
        item.id == "rId2" || item.id == "rId3" || item.id == "rId5"
    });
    assert_eq!(
        removed
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["rId2", "rId3", "rId5"]
    );
    // A second removal of the same owner, after the first.
    package.remove_relationships_where("/owner.xml", |item| item.id == "rId1");
    assert_eq!(ids(&package), ["rId4"]);
    let journal = package.close_part_journal();
    assert!(package.part_journal_changed(&journal));
    assert!(journal.removed_relationship("/owner.xml", "rId3"));
    assert!(!journal.removed_relationship("/owner.xml", "rId4"));
    // No part was written: the journal holds the removed items, not a copy.
    assert_eq!(journal.written_parts().count(), 0);

    package.undo_part_journal(journal);
    assert_eq!(ids(&package), ["rId1", "rId2", "rId3", "rId4", "rId5"]);
    assert!(package.same_package_as(&before));
}

#[test]
fn removals_after_a_whole_copy_of_the_owner_are_put_back_by_the_copy() {
    let mut package = package_with_relationships(&["rId1", "rId2", "rId3"]);
    package.open_part_journal();
    package.remove_relationships_where("/owner.xml", |item| item.id == "rId1");
    package.record_part_rels("/owner.xml");
    package
        .get_part_rels_mut("/owner.xml")
        .unwrap()
        .add_with_id("rId9", "urn:test:link", "rId9.xml");
    package.remove_relationships_where("/owner.xml", |item| item.id == "rId3");
    assert_eq!(ids(&package), ["rId2", "rId9"]);
    let journal = package.close_part_journal();
    package.undo_part_journal(journal);
    assert_eq!(ids(&package), ["rId1", "rId2", "rId3"]);
}

#[test]
fn a_removal_journal_without_a_removal_changes_nothing() {
    let mut package = package_with_relationships(&["rId1"]);
    package.open_part_journal();
    assert!(
        package
            .remove_relationships_where("/owner.xml", |_| false)
            .is_empty()
    );
    assert!(
        package
            .remove_relationships_where("/absent.xml", |_| true)
            .is_empty()
    );
    let journal = package.close_part_journal();
    assert!(!package.part_journal_changed(&journal));
    package.undo_part_journal(journal);
    assert_eq!(ids(&package), ["rId1"]);
}

fn targets(package: &OpcPackage) -> Vec<String> {
    package
        .get_part_rels("/owner.xml")
        .expect("the owner keeps its relationships")
        .items
        .iter()
        .map(|item| item.target.clone())
        .collect()
}

#[test]
fn a_retargeted_relationship_gets_its_target_back_on_undo() {
    let mut package = package_with_relationships(&["rId1", "rId2"]);
    let before = package.clone();

    package.open_part_journal();
    assert!(package.set_relationship_target("/owner.xml", "rId2", "first.xml"));
    assert!(package.set_relationship_target("/owner.xml", "rId2", "second.xml"));
    assert!(!package.set_relationship_target("/owner.xml", "rId9", "absent.xml"));
    assert!(!package.set_relationship_target("/absent.xml", "rId1", "absent.xml"));
    assert_eq!(targets(&package), ["rId1.xml", "second.xml"]);
    let journal = package.close_part_journal();
    assert!(package.part_journal_changed(&journal));
    // No part was written and the owner was not copied.
    assert_eq!(journal.written_parts().count(), 0);

    package.undo_part_journal(journal);
    assert_eq!(targets(&package), ["rId1.xml", "rId2.xml"]);
    assert!(package.same_package_as(&before));
}

#[test]
fn a_retarget_back_to_the_same_target_changes_nothing() {
    let mut package = package_with_relationships(&["rId1"]);
    package.open_part_journal();
    package.set_relationship_target("/owner.xml", "rId1", "other.xml");
    package.set_relationship_target("/owner.xml", "rId1", "rId1.xml");
    let journal = package.close_part_journal();
    assert!(!package.part_journal_changed(&journal));
}

#[test]
fn a_retargeted_then_removed_relationship_goes_back_with_its_first_target() {
    let mut package = package_with_relationships(&["rId1", "rId2"]);
    package.open_part_journal();
    package.set_relationship_target("/owner.xml", "rId1", "moved.xml");
    package.remove_relationships_where("/owner.xml", |item| item.id == "rId1");
    // A copy of the whole owner after the retarget holds the new target:
    // the retarget is still undone on top of it.
    package.record_part_rels("/owner.xml");
    package.set_relationship_target("/owner.xml", "rId2", "later.xml");
    let journal = package.close_part_journal();
    package.undo_part_journal(journal);
    assert_eq!(ids(&package), ["rId1", "rId2"]);
    assert_eq!(targets(&package), ["rId1.xml", "rId2.xml"]);
}

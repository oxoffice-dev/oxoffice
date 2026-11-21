//! The values that opening and editing a deck move by value stay a few KB.
//!
//! A release wheel opens a deck in a few dozen frames, and each frame holds
//! some of these values, sometimes several copies: when a text list style
//! carried its ten levels inline, a slide master was 82 KB and opening a deck
//! needed about 820 KiB of stack, most of the 1 MiB a Windows program that
//! embeds Python gives its main thread. The release workflow is to measure
//! the stack of each wheel it builds (#15); this test catches the cause in
//! every pull request.

use std::mem::size_of;

use oxoffice_drawing::text::list_style::CT_TextListStyle;
use oxoffice_pptx::Presentation;
use oxoffice_pptx_oxml::notes_parts::CT_NotesMaster;
use oxoffice_pptx_oxml::presentation::CT_Presentation;
use oxoffice_pptx_oxml::slide_parts::CT_SlideMaster;

#[test]
fn values_moved_on_the_stack_stay_small() {
    let sizes = [
        ("CT_TextListStyle", size_of::<CT_TextListStyle>(), 512),
        ("CT_SlideMaster", size_of::<CT_SlideMaster>(), 4096),
        ("CT_NotesMaster", size_of::<CT_NotesMaster>(), 4096),
        ("CT_Presentation", size_of::<CT_Presentation>(), 2048),
        ("Presentation", size_of::<Presentation>(), 4096),
    ];
    let over: Vec<_> = sizes
        .iter()
        .filter(|(_, size, limit)| size > limit)
        .collect();
    assert!(
        over.is_empty(),
        "(type, bytes, limit) over the limit: {over:?}"
    );
}

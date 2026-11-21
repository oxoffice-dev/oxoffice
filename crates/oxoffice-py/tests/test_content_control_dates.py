"""A date content control shows its date in the control's format, Word's long date included, by the tag setter
and by fill_form, on the Word file of the corpus."""

from pathlib import Path

import oxoffice.docx as oxdocx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
TAG = "date_of_incorporation"


def form():
    return oxdocx.Document.from_bytes((CORPUS / "word" / "form.docx").read_bytes())


def shown(doc):
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    return next(control.text for control in saved.content_controls() if control.tag == TAG)


def test_a_long_date_control_takes_a_date_by_tag_and_by_form():
    by_tag = form()
    assert by_tag.set_content_control_value_by_tag(TAG, "2015-01-02") == 1
    assert shown(by_tag) == "2 January 2015"

    by_form = form()
    target = oxdocx.FormFieldTarget.content_control_tag(TAG)
    rule = oxdocx.FormFieldRule(target=target, reset=oxdocx.FormResetValue.placeholder("Pick a date"))
    value = oxdocx.FormFieldAssignment(target=target, value=oxdocx.FormFieldValue.text("2015-01-02"))
    assert by_form.fill_form([rule], [value]) == (1,)
    assert shown(by_form) == "2 January 2015"
    assert by_form.validate_form([rule]) == ()

"""docx: a tracked changes session (Document.track_changes) writes the text and property edits it records as tracked
changes, runs the edits Word records no revision for as without a session, and refuses every other edit, the document
left as it was. The class table below names the calls a session tracks or leaves untracked; every other mutating call of the
scaling registry (test_docx_scaling.TIMED and WHOLE_DOCUMENT) is refused, so that a call added to the API is refused
until a class names it, and test_every_call_behaves_as_its_class runs each one under a session."""
import datetime
import io
import re
import zipfile
from pathlib import Path

import docx
import pytest
from docx.oxml import parse_xml

import oxoffice.docx as oxdocx
import test_docx_scaling as scaling
from builders import edit_cost_docx

AUTHOR, STAMP = "Rev", "2026-11-27T10:00:00Z"

# Run, paragraph, table and section property changes (PR 2b, 2c), checked on the XML of the paragraphs, tables
# and sections.
FORMATTING_CLASSES = ("ColorFormat.", "Font.", "ParagraphFormat.", "TabStops.", "Row.")
FORMATTING = {
    "Paragraph.alignment", "Paragraph.numbering", "Paragraph.style", "Run.bold", "Run.italic", "Run.style_id",
    "Run.underline", "Cell.set_border", "Cell.set_margins", "Cell.shading", "Cell.vertical_alignment", "Cell.width",
    "Table.alignment", "Table.autofit", "Table.banded_columns", "Table.banded_rows", "Table.first_column",
    "Table.first_row", "Table.grid_widths", "Table.indent", "Table.last_column", "Table.last_row", "Table.set_border",
    "Table.set_borders", "Table.set_cell_margins", "Table.set_column_width", "Table.style", "Table.width",
    "Document.update_section", "SectionHandle.bottom_margin", "SectionHandle.different_first_page_header_footer",
    "SectionHandle.footer_distance", "SectionHandle.gutter", "SectionHandle.header_distance",
    "SectionHandle.left_margin", "SectionHandle.margin_bottom", "SectionHandle.margin_left",
    "SectionHandle.margin_right", "SectionHandle.margin_top", "SectionHandle.orientation", "SectionHandle.page_height",
    "SectionHandle.page_width", "SectionHandle.right_margin", "SectionHandle.set_note_policy",
    "SectionHandle.set_page_border", "SectionHandle.top_margin",
}
TRACKED = {
    "Cell.replace_text", "Cell.text", "Document.add_bullet_list_item", "Document.add_heading",
    "Document.add_numbered_list_item", "Document.add_page_break", "Document.add_paragraph", "Document.add_table",
    "Document.insert_paragraph", "Document.insert_table", "Paragraph.insert_paragraph_before",
    "Document.clone_content", "Document.import_fragment", "Document.insert_content", "Document.insert_document",
    "Document.insert_html", "Document.pop_content", "Document.remove_content", "Document.remove_content_range",
    "Table.add_row", "Table.clone_row", "Table.remove_row", "Run.add_field", "Run.add_picture",
    "Paragraph.add_hyperlink", "HeaderFooter.add_paragraph", "HeaderFooter.add_table", "FootnoteEndnote.add_paragraph",
    "FootnoteEndnote.add_table", "Cell.add_paragraph", "Cell.add_table", "Document.add_footnote", "Document.add_endnote",
    "Document.replace_all", "Document.replace_all_regex",
    "Document.replace_all_regex_expected", "Document.replace_text_at", "Document.set_story_text",
    "Document.try_replace_text", "Paragraph.add_run", "Paragraph.replace_text", "Paragraph.replace_xml",
    "Paragraph.text", "Run.add_break", "Run.add_tab", "Run.remove", "Run.text",
}
# What Word records no revision for: properties, settings, styles, numbering definitions, fonts, comments,
# bookmarks, permission ranges, protection, custom XML, bibliography sources, the mail merge package, and the
# resolution of revisions.
UNTRACKED_CLASSES = ("AppProperties.", "CoreProperties.", "CustomProperties.", "Settings.")
UNTRACKED = {
    "Document.accept_all", "Document.accept_revision_id", "Document.accept_revisions_by_author",
    "Document.accept_revisions_in_date_range", "Document.add_bibliography_source", "Document.add_bookmark",
    "Document.add_comment", "Document.add_comment_on_text", "Document.add_custom_xml_store",
    "Document.add_numbering_definition", "Document.add_numbering_instance", "Document.add_story_permission_range",
    "Document.add_style", "Document.default_font_name", "Document.default_font_size", "Document.embed_font",
    "Document.embed_font_at", "Document.embed_system_fonts", "Document.embed_true_type_fonts",
    "Document.link_style_to_numbering", "Document.move_comment", "Document.move_comment_to_text",
    "Document.reject_all", "Document.reject_revision_id", "Document.reject_revisions_by_author",
    "Document.reject_revisions_in_date_range", "Document.remove_bibliography_source",
    "Document.remove_comment", "Document.remove_document_protection", "Document.remove_embedded_font",
    "Document.remove_embedded_font_at", "Document.remove_font", "Document.remove_font_at",
    "Document.remove_mail_merge_package", "Document.remove_style", "Document.replace_bibliography_source",
    "Document.reply_to", "Document.resolve_comment", "Document.save_subset_fonts",
    "Document.set_default_paragraph_format", "Document.set_default_style", "Document.set_document_protection",
    "Document.set_font", "Document.set_font_at", "Document.set_mail_merge_package", "Document.set_note_policy",
    "Document.set_style", "Document.sign", "Document.split_run", "Document.update_fields_on_open",
}
SESSION = {"Document.track_changes", "TrackedChanges.stop"}

# The whole-document calls, which the scaling registry does not time: a refused one refuses before it reads the
# document, so its arguments need only be well formed.
NOW = datetime.datetime(2026, 11, 27)
WHOLE = {
    "Document.accept_all": lambda doc: doc.accept_all(),
    "Document.reject_all": lambda doc: doc.reject_all(),
    "Document.accept_revisions_by_author": lambda doc: doc.accept_revisions_by_author("Editor"),
    "Document.reject_revisions_by_author": lambda doc: doc.reject_revisions_by_author("Editor"),
    "Document.accept_revisions_in_date_range":
        lambda doc: doc.accept_revisions_in_date_range(start="2026-01-01T00:00:00Z", end="2026-12-31T00:00:00Z"),
    "Document.reject_revisions_in_date_range":
        lambda doc: doc.reject_revisions_in_date_range(start="2026-01-01T00:00:00Z", end="2026-12-31T00:00:00Z"),
    "Document.try_replace_text": lambda doc: doc.try_replace_text("alpha beta 0", "gamma beta 0"),
    "Document.replace_all": lambda doc: doc.replace_all([("alpha beta 1", "gamma beta 1", 1)]),
    "Document.replace_all_regex": lambda doc: doc.replace_all_regex([(r"alpha (beta 2)", "gamma $1")]),
    "Document.replace_all_regex_expected": lambda doc: doc.replace_all_regex_expected([(r"alpha (beta 3)", "gamma $1", 1)]),
    "Document.add_comment_on_text": lambda doc: doc.add_comment_on_text("alpha beta 0", author=AUTHOR, text="why"),
    "Document.move_comment_to_text": lambda doc: doc.move_comment_to_text(doc.comments[0].id, "alpha beta 0"),
    "Document.remove_bibliography_source": lambda doc: doc.remove_bibliography_source("source"),
    "Document.compare": lambda doc: doc.compare(oxdocx.Document(), AUTHOR, STAMP),
    "Document.bind_content_controls": lambda doc: doc.bind_content_controls({"probe0": "x"}),
    "Document.fill_content_control_tags": lambda doc: doc.fill_content_control_tags([("probe0", "x")]),
    "Document.set_content_control_value_by_tag": lambda doc: doc.set_content_control_value_by_tag("probe0", "x"),
    "Document.set_content_control_value_by_alias": lambda doc: doc.set_content_control_value_by_alias("a", "x"),
    "Document.set_legacy_form_field_value": lambda doc: doc.set_legacy_form_field_value("name", "x"),
    "Document.protect_form": lambda doc: doc.protect_form([], oxdocx.DocumentProtection(mode="forms", enforcement=True)),
    "Document.update_fields": lambda doc: doc.update_fields(now=NOW),
    "Document.update_all_fields": lambda doc: doc.update_all_fields(now=NOW),
    "Document.update_page_fields": lambda doc: doc.update_page_fields(),
    "Document.update_layout_backed_fields": lambda doc: doc.update_layout_backed_fields(),
    "Document.rebuild_toc": lambda doc: doc.rebuild_toc(),
    "Document.insert_toc": lambda doc: doc.insert_toc(0),
    "Document.update_bibliography": lambda doc: doc.update_bibliography(),
    "Document.set_bibliography_options": lambda doc: doc.set_bibliography_options("APA"),
    "Document.insert_citation_after_text": lambda doc: doc.insert_citation_after_text("alpha beta 0", ["t"]),
}
BOX_NAMESPACES = " ".join(f'xmlns:{prefix}="{uri}"' for prefix, uri in {
    "w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "wp": "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "mc": "http://schemas.openxmlformats.org/markup-compatibility/2006",
    "v": "urn:schemas-microsoft-com:vml",
    "wps": "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
}.items())
# What a whole-document call needs before it runs.
SETUP = {"Document.remove_bibliography_source": lambda doc: doc.add_bibliography_source(scaling.book("source"))}
NOT_RUN = {"Document.sign": "needs a signing certificate; it changes no content"}

CALLS = ({name for name in scaling.TIMED if " (" not in name} | set(scaling.WHOLE_DOCUMENT)) - SESSION


def kind(name):
    if name in FORMATTING or name.startswith(FORMATTING_CLASSES):
        return "formatting"
    if name in TRACKED:
        return "tracked"
    return "untracked" if name in UNTRACKED or name.startswith(UNTRACKED_CLASSES) else "refused"


SECTION_FIELDS = ("orientation", "page_width", "page_height", "margin_top", "margin_right", "margin_bottom",
                  "margin_left", "gutter", "column_count", "column_spacing", "page_number_start", "header_distance",
                  "footer_distance", "different_first_page", "break_type")


def view(data, resolve=None, xml=False):
    """The text of every story paragraph of the document saved as `data`, its revisions resolved by `resolve`; with
    `xml`, the XML of every body paragraph too, which shows their formatting."""
    doc = oxdocx.Document.from_bytes(data)
    if resolve:
        getattr(doc, resolve)()
    # The redline writes every text it rewrites with xml:space="preserve", which changes nothing it reads, and
    # Run.style_id's recipe names a new style for each document it makes ready.
    marks = []
    if xml:
        with zipfile.ZipFile(io.BytesIO(doc.to_bytes())) as z:
            main = z.read("word/document.xml").decode()
        marks = [p.xml for p in doc.paragraphs] + [t.xml for t in doc.tables]
        marks = [re.sub(r"Probestyle\d+", "Probestyle", xml.replace(' xml:space="preserve"', "")) for xml in marks]
        # Each section by what it reads, and the children of its properties the section calls write, as the XML of a
        # section saved again writes its other attributes in another order.
        marks += [tuple(getattr(section, field) for field in SECTION_FIELDS) for section in doc.sections]
        marks += re.findall(r"<w:(?:pgBorders|footnotePr|endnotePr)[ >].*?</w:(?:pgBorders|footnotePr|endnotePr)>",
                            main)
    # Every story's paragraphs by their text and style; the XML is read for the body, which the recipes format.
    return [(p.kind, p.text, p.style) for p in doc.story_paragraphs] + marks


def ready(doc, name):
    """The call `name` on doc, its targets made: it runs when called."""
    if name in WHOLE:
        SETUP.get(name, lambda doc: None)(doc)
        return lambda: WHOLE[name](doc)
    return scaling.TIMED[name](doc)(0)


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    return edit_cost_docx(tmp_path_factory.mktemp("tracked") / "saved.docx", 10, picture_side=8)


def test_the_class_table_names_calls_there_are():
    assert sorted((TRACKED | UNTRACKED | set(NOT_RUN) | set(WHOLE)) - CALLS) == []
    assert sorted(set(scaling.WHOLE_DOCUMENT) - set(WHOLE) - set(NOT_RUN)) == []


@pytest.mark.parametrize("name", sorted(CALLS - set(NOT_RUN)))
def test_every_call_behaves_as_its_class(saved, name):
    """A tracked call writes revisions whose acceptance gives the untracked call's result and whose rejection gives
    the document back; an untracked one writes none; a refused one raises DocxError and changes nothing."""
    doc = oxdocx.Document.open(saved)
    call = ready(doc, name)
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
        if kind(name) == "refused":
            with pytest.raises(oxdocx.DocxError, match="not recorded as a tracked change yet"):
                call()
        else:
            call()
    after = doc.to_bytes()
    if kind(name) == "refused":
        assert after == before
    if kind(name) not in ("tracked", "formatting"):
        assert session.revision_ids == ()
        return
    xml = kind(name) == "formatting"
    # A call that sets what the document already holds writes nothing.
    assert session.revision_ids or view(after, None, xml) == view(before, None, xml)
    # A grid change has an id alone, as Word writes it.
    authors = {r.author for r in doc.revisions if r.id in session.revision_ids and r.kind != "table_grid_change"}
    assert authors <= {AUTHOR}
    twin = oxdocx.Document.open(saved)
    ready(twin, name)()
    assert view(after, "accept_all", xml) == view(twin.to_bytes(), "accept_all", xml)
    assert view(after, "reject_all", xml) == view(before, "reject_all", xml)


def test_a_session_runs_until_it_stops():
    doc = oxdocx.Document()
    doc.add_paragraph("Pay within 30 days.")
    with doc.track_changes(AUTHOR, undated=True) as session:
        assert doc.tracked_changes.active
        with pytest.raises(oxdocx.DocxError, match="already on"):
            doc.track_changes("Other")
        doc.paragraphs[0].text = "Pay within 45 days."
    assert doc.tracked_changes is None and not session.active
    assert [(r.author, r.timestamp, r.text) for r in doc.revisions] == [(AUTHOR, None, "30"), (AUTHOR, None, "45")]
    session.stop()
    with pytest.raises(oxdocx.DocxError, match="has stopped"):
        with session:
            doc.paragraphs[0].text = "Pay within 60 days."
    assert doc.paragraphs[0].text == "Pay within 45 days."
    doc.add_paragraph("Untracked.")
    assert len(doc.revisions) == 2
    for author, date in [("", {}), (AUTHOR, {"timestamp": "27/11/2026"}),
                         (AUTHOR, {"timestamp": STAMP, "undated": True})]:
        with pytest.raises((oxdocx.DocxError, ValueError)):
            doc.track_changes(author, **date)


def test_a_refused_call_leaves_no_revision_ids():
    doc = oxdocx.Document()
    doc.add_paragraph("The quick brown fox.")
    with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
        with pytest.raises(Exception, match="expected 1"):
            doc.replace_all([("quick", "slow", 1), ("missing", "x", 1)])
        assert session.revision_ids == ()
    # The ids it took on the copy it dropped are not lent to it by a later session that takes them.
    with doc.track_changes("Other", timestamp=STAMP) as other:
        doc.replace_all([("quick", "slow", 1)])
    assert session.revision_ids == () and len(other.revision_ids) == 2


def test_paragraphs_appended_after_styled_ones_resolve_both_ways():
    def made(tracked):
        doc = oxdocx.Document()
        doc.add_heading("Start", level=2)
        before = doc.to_bytes()
        session = doc.track_changes(AUTHOR, timestamp=STAMP) if tracked else None
        doc.add_heading("Title", level=1)
        doc.add_paragraph("Body.")
        doc.add_bullet_list_item("Item.")
        if session:
            session.stop()
        return before, doc.to_bytes()
    before, after = made(True)
    assert view(after, "accept_all") == view(made(False)[1], "accept_all")
    assert view(after, "reject_all") == view(before, "reject_all")


def test_copied_tracked_content_keeps_one_mark_and_ids_of_its_own():
    old, new = oxdocx.Document(), oxdocx.Document()
    for doc, texts in ((old, ("One.", "Two.")), (new, ("One.", "New.", "Two."))):
        for text in texts:
            doc.add_paragraph(text)
    old.compare(new, "Other", STAMP)
    doc = oxdocx.Document.from_bytes(old.to_bytes())
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP):
        doc.clone_content(doc.paragraphs[1], 0)
        doc.clone_content(doc.paragraphs[2], len(doc.paragraphs))
    after = doc.to_bytes()
    with zipfile.ZipFile(io.BytesIO(after)) as z:
        main = z.read("word/document.xml").decode()
    assert not re.search(r"<w:rPr>(?:(?!</w:rPr>).)*<w:ins (?:(?!</w:rPr>).)*<w:ins ", main)
    ids = re.findall(r"<w:(?:ins|del|rPrChange|pPrChange) w:id=\"(\d+)\"", main)
    assert len(ids) == len(set(ids))
    # The story's final mark holds no revision.
    last = main[main.rfind("<w:p>"):main.rfind("<w:sectPr")]
    assert "<w:pPr><w:rPr><w:ins" not in last
    assert view(after, "reject_all") == view(before, "reject_all")
    assert [p.text for p in oxdocx.Document.from_bytes(after).paragraphs] == ["New.", "One.", "New.", "Two.", "New."]


def test_copied_content_holding_a_deletion_is_refused():
    # A paragraph whose mark another author deleted: rejecting the session would bring the copy's deletion back.
    w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
    doc = oxdocx.Document()
    for text in ("One.", "Two.", "Three."):
        doc.add_paragraph(text)
    doc.paragraphs[1].replace_xml(
        f'<w:p xmlns:w="{w}"><w:pPr><w:rPr><w:del w:id="5" w:author="Other" w:date="{STAMP}"/></w:rPr></w:pPr>'
        '<w:r><w:t>Two.</w:t></w:r></w:p>')
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
        with pytest.raises(oxdocx.DocxError, match="not recorded as a tracked change yet"):
            doc.clone_content(doc.paragraphs[1], 0)
    assert doc.to_bytes() == before and session.revision_ids == ()


def test_a_removal_keeps_the_block_in_view_until_resolved():
    doc = oxdocx.Document()
    for text in ("A.", "B.", "C."):
        doc.add_paragraph(text)
    doc.add_table(2, 2)
    doc.add_paragraph("D.")
    doc.section(0).header.add_table(1, 1)
    kept = doc.paragraphs[2]
    with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
        assert doc.remove_content(1)
        doc.tables[0].remove_row(0)
        # The removed block keeps its index and reads as its accepted text; the removed row still counts.
        assert [p.text for p in doc.paragraphs] == ["A.", "", "C.", "D."]
        assert len(doc.tables[0].rows) == 2 and kept.text == "C."
        ids = session.revision_ids
        for call in (lambda: doc.pop_content(doc.paragraphs[0]), lambda: doc.section(0).header.tables[0].remove_row(0)):
            with pytest.raises(oxdocx.DocxError, match="not recorded as a tracked change yet"):
                call()
        assert session.revision_ids == ids
    doc.accept_all()
    assert [p.text for p in doc.paragraphs] == ["A.", "C.", "D."] and len(doc.tables[0].rows) == 1


def test_a_removal_of_a_block_declaring_namespaces_of_its_own_is_refused():
    # A paragraph, then a table, holding a text box and declaring the namespaces the box uses, as Word may write
    # them: rewritten in place, the body could no longer be saved.
    declared = ('xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
                'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
                'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"')
    box = ('<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="457200"/>'
           '<wp:docPr id="1" name="Text Box"/><a:graphic><a:graphicData '
           'uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/>'
           '<wps:spPr/><wps:txbx><w:txbxContent><w:p><w:r><w:t>Boxed.</w:t></w:r></w:p></w:txbxContent></wps:txbx>'
           '<wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>')
    paragraph = f'<w:p {declared}><w:r><w:t>Two.</w:t></w:r>{box}</w:p>'
    table = (f'<w:tbl {declared}><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>'
             f'<w:tr><w:tc><w:p>{box}</w:p></w:tc></w:tr></w:tbl>')
    with zipfile.ZipFile(io.BytesIO(oxdocx.Document().to_bytes())) as source:
        parts = {item: source.read(item) for item in source.namelist()}
    main = parts["word/document.xml"].decode()
    start = main.index("<w:body>") + len("<w:body>")
    for block, calls in (
        (paragraph, (lambda doc: doc.remove_content(1), lambda doc: doc.remove_content_range(1, 2),
                     lambda doc: doc.pop_content(1))),
        (table, (lambda doc: doc.tables[0].remove_row(0),)),
    ):
        body = f"<w:p><w:r><w:t>One.</w:t></w:r></w:p>{block}<w:p><w:r><w:t>Three.</w:t></w:r></w:p>"
        parts["word/document.xml"] = (main[:start] + body + main[main.index("<w:sectPr"):]).encode()
        out = io.BytesIO()
        with zipfile.ZipFile(out, "w") as target:
            for name, data in parts.items():
                target.writestr(name, data)
        doc = oxdocx.Document.from_bytes(out.getvalue())
        before = doc.to_bytes()
        with doc.track_changes(AUTHOR, timestamp=STAMP):
            for call in calls:
                with pytest.raises(oxdocx.DocxError, match="not recorded as a tracked change yet"):
                    call(doc)
        assert doc.to_bytes() == before


def main_xml(data):
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        return z.read("word/document.xml").decode()


def test_a_tracked_picture_or_link_goes_into_an_insertion_and_reject_removes_it():
    from builders import noise_png
    for name, call, mark in (
        ("picture", lambda p: p.runs[0].add_picture(noise_png(8, 1), "probe.png"), "<w:drawing"),
        ("link", lambda p: p.add_hyperlink("docs", "https://example.com/"), "HYPERLINK"),
    ):
        doc = oxdocx.Document()
        doc.add_paragraph("See ")
        with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
            call(doc.paragraphs[0])
        assert session.revision_ids, name
        after = doc.to_bytes()
        assert re.search(r"<w:ins [^>]*>(?:(?!</w:ins>).)*" + mark, main_xml(after)), name
        rejected = oxdocx.Document.from_bytes(after)
        rejected.reject_all()
        assert mark not in main_xml(rejected.to_bytes()), name
    # The link reads as an untracked one once accepted: its text takes the Hyperlink style.
    accepted = oxdocx.Document.from_bytes(after)
    accepted.accept_all()
    assert '<w:rStyle w:val="Hyperlink"/>' in main_xml(accepted.to_bytes())


def test_a_field_added_in_a_header_is_tracked_there():
    doc = oxdocx.Document()
    doc.add_paragraph("Body.")
    header = doc.section(0).header
    header.add_paragraph("Page ")
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
        header.paragraphs[-1].runs[0].add_field("PAGE", "1")
    assert session.revision_ids
    assert view(doc.to_bytes(), "reject_all") == view(before, "reject_all")


def test_an_insertion_in_a_comment_is_refused_without_an_id():
    doc = oxdocx.Document()
    doc.add_paragraph("Body.")
    doc.add_comment_on_text("Body", author="Other", text="Note this.")
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
        comment = next(item.paragraph for item in doc.story_items
                       if item.story.kind == "comment" and item.kind == "paragraph")
        with pytest.raises(oxdocx.DocxError, match="not recorded as a tracked change yet"):
            comment.insert_paragraph_before("More.")
    assert doc.to_bytes() == before and session.revision_ids == ()


def made_with_stories(edit, tracked):
    """A document with a header, a note and a table, `edit` made on it under a session or not: the bytes before and
    after, and what `edit` answered."""
    doc = oxdocx.Document()
    doc.add_paragraph("Body.")
    doc.section(0).header.add_paragraph("Head.")
    doc.add_footnote(doc.paragraphs[0], "The note.")
    doc.add_table(1, 1).cell(0, 0).text = "Cell."
    before = doc.to_bytes()
    session = doc.track_changes(AUTHOR, timestamp=STAMP) if tracked else None
    answer = edit(doc)
    if session:
        assert session.revision_ids
        session.stop()
    return before, doc.to_bytes(), answer


def story_paragraph(doc, kind):
    return next(item.paragraph for item in doc.story_items if item.story.kind == kind and item.kind == "paragraph")


@pytest.mark.parametrize("where", ["header", "footnote", "cell"])
def test_a_paragraph_inserted_before_one_of_another_story_resolves_both_ways(where):
    def edit(doc):
        target = doc.tables[0].cell(0, 0).paragraphs[0] if where == "cell" else story_paragraph(doc, where)
        target.insert_paragraph_before("New.")
    before, after, _ = made_with_stories(edit, True)
    assert view(after, "accept_all") == view(made_with_stories(edit, False)[1], "accept_all")
    assert view(after, "reject_all") == view(before, "reject_all")


def test_two_paragraphs_added_to_a_header_resolve_both_ways():
    def edit(doc):
        header = doc.section(0).header
        header.paragraphs[-1].alignment = scaling.WD_ALIGN_PARAGRAPH.CENTER
        added = []
        for text in ("One.", "Two."):
            added.append(header.add_paragraph(text).text)
        return added
    before, after, added = made_with_stories(edit, True)
    assert added == ["One.", "Two."]
    twin = made_with_stories(edit, False)[1]
    assert view(after, "accept_all") == view(twin, "accept_all")
    assert view(after, "reject_all") == view(before, "reject_all")
    def alignments(data, resolve):
        doc = oxdocx.Document.from_bytes(data)
        getattr(doc, resolve)()
        with zipfile.ZipFile(io.BytesIO(doc.to_bytes())) as z:
            header = next(name for name in z.namelist() if re.fullmatch(r"word/header\d+\.xml", name))
            return [re.findall(r'<w:jc w:val="(\w+)"/>', paragraph)
                    for paragraph in re.findall(r"<w:p[ >].*?</w:p>", z.read(header).decode())]
    assert alignments(after, "accept_all") == alignments(twin, "accept_all")


def test_an_insertion_in_a_table_of_a_comment_or_a_text_box_is_refused(tmp_path):
    table = '<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>In.</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>'
    d = docx.Document()
    d.add_paragraph("See ")._p.append(parse_xml(
        f'<w:r {BOX_NAMESPACES}><mc:AlternateContent><mc:Choice Requires="wps">'
        '<w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="Box"/><a:graphic>'
        '<a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp>'
        f'<wps:cNvSpPr txBox="1"/><wps:spPr/><wps:txbx><w:txbxContent>{table}</w:txbxContent></wps:txbx><wps:bodyPr/>'
        '</wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape>'
        f'<v:textbox><w:txbxContent>{table}</w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback>'
        '</mc:AlternateContent></w:r>'))
    d.save(tmp_path / "box.docx")
    boxed = oxdocx.Document.open(tmp_path / "box.docx")
    commented = oxdocx.Document()
    commented.add_paragraph("Body.")
    commented.add_comment_on_text("Body", author="Other", text="Note.")
    with zipfile.ZipFile(io.BytesIO(commented.to_bytes())) as source:
        parts = {name: source.read(name) for name in source.namelist()}
    parts["word/comments.xml"] = re.sub(r"(<w:comment [^>]*>)", lambda found: found.group(1) + table,
                                        parts["word/comments.xml"].decode(), count=1).encode()
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w") as target:
        for name, data in parts.items():
            target.writestr(name, data)
    commented = oxdocx.Document.from_bytes(out.getvalue())
    for doc in (boxed, commented):
        before = doc.to_bytes()
        with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
            cell = next(item.paragraph for item in doc.story_items
                        if item.story.kind == "table_cell" and item.kind == "paragraph")
            with pytest.raises(oxdocx.DocxError, match="not recorded as a tracked change yet"):
                cell.insert_paragraph_before("More.")
        assert doc.to_bytes() == before and session.revision_ids == ()


def test_a_note_goes_with_the_only_reference_a_resolution_removes():
    # As Word removes a comment whose anchor goes (W9): rejecting a note inserted with its reference, or accepting one
    # deleted with it, removes the note, in a file from Word as in a session's own insertion. A note that keeps its
    # text stays.
    for marker, resolve in (("ins", "reject_all"), ("del", "accept_all")):
        doc = oxdocx.Document()
        doc.add_paragraph("Body.")
        note = doc.add_footnote(doc.paragraphs[0], "The note.")
        wrapped = rf'<w:{marker} w:id="ID" w:author="Other" w:date="{STAMP}">\1</w:{marker}>'
        texts = (lambda xml: xml.replace("<w:t>", "<w:delText>").replace("<w:t ", "<w:delText ")
                 .replace("</w:t>", "</w:delText>")) if marker == "del" else (lambda xml: xml)
        for number, paragraph in ((8, note.paragraphs[0]), (9, doc.paragraphs[0])):
            runs = r"(<w:r>(?:(?!</w:r>).)*<w:footnoteRef/>.*</w:r>)" if number == 8 else \
                r"(<w:r>(?:(?!</w:r>).)*<w:footnoteReference [^>]*/>(?:(?!</w:r>).)*</w:r>)"
            paragraph.replace_xml(texts(re.sub(runs, wrapped.replace("ID", str(number)), paragraph.xml)))
        resolved = oxdocx.Document.from_bytes(doc.to_bytes())
        getattr(resolved, resolve)()
        assert [p.kind for p in resolved.story_paragraphs if p.kind == "footnote"] == [], marker
    # A tracked note, rejected, leaves nothing; accepted, it stays.
    doc = oxdocx.Document()
    doc.add_paragraph("Body.")
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP):
        doc.add_footnote(doc.paragraphs[0], "The note.")
    assert re.search(r"<w:ins [^>]*>(?:(?!</w:ins>).)*<w:footnoteReference", main_xml(doc.to_bytes()))
    assert view(doc.to_bytes(), "reject_all") == view(before, "reject_all")
    accepted = oxdocx.Document.from_bytes(doc.to_bytes())
    accepted.accept_all()
    assert [p.text for p in accepted.story_paragraphs if p.kind == "footnote"] == [" The note."]


def test_an_insertion_outside_the_body_is_refused():
    doc = oxdocx.Document()
    doc.add_paragraph("Body.")
    header = doc.section(0).header
    header.add_paragraph("Head.")
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP) as session:
        for call in (lambda: doc.insert_html("<p>New.</p>", after=header.paragraphs[-1]),):
            with pytest.raises(oxdocx.DocxError, match="not recorded as a tracked change yet"):
                call()
    assert doc.to_bytes() == before and session.revision_ids == ()


def test_a_refused_table_style_adds_no_style():
    doc = oxdocx.Document()
    doc.section(0).header.add_table(1, 1)
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP):
        with pytest.raises(oxdocx.DocxError):
            doc.section(0).header.tables[0].style = "Light Shading Accent 1"
    assert doc.to_bytes() == before


def test_a_regular_expression_replacement_in_a_text_box_is_refused(tmp_path):
    d = docx.Document()
    d.add_paragraph("See ")._p.append(parse_xml(
        f'<w:r {BOX_NAMESPACES}><mc:AlternateContent><mc:Choice Requires="wps">'
        '<w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="Box"/><a:graphic>'
        '<a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp>'
        '<wps:cNvSpPr txBox="1"/><wps:spPr/><wps:txbx><w:txbxContent><w:p><w:r><w:t>Boxed old text</w:t></w:r>'
        '</w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline>'
        '</w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r>'
        '<w:t>Boxed old text</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback>'
        '</mc:AlternateContent></w:r>'))
    d.save(tmp_path / "box.docx")
    doc = oxdocx.Document.open(tmp_path / "box.docx")
    before = doc.to_bytes()
    with doc.track_changes(AUTHOR, timestamp=STAMP):
        with pytest.raises(oxdocx.DocxError, match="a replacement in a text box"):
            doc.replace_all_regex([("old (text)", "new $1")])
    assert doc.to_bytes() == before


def test_a_google_grid_change_reads_with_its_kind_and_goes_with_every_change():
    path = Path(__file__).resolve().parents[3] / "tests/corpus/files/google/report.docx"
    doc = oxdocx.Document.open(path)
    grids = [r for r in doc.revisions if r.kind == "table_grid_change"]
    assert grids and all(r.author == "" for r in grids)
    doc.accept_all()
    assert doc.revisions == ()


def test_every_table_and_numbering_change_reads_with_its_kind(tmp_path):
    marks = 'w:author="Ann" w:date="2026-11-20T09:00:00Z"'
    body = (
        '<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="3000"/><w:tblGridChange w:id="4"><w:tblGrid><w:gridCol w:w="2000"/>'
        f'</w:tblGrid></w:tblGridChange></w:tblGrid><w:tr><w:tblPrEx><w:jc w:val="center"/><w:tblPrExChange w:id="3" {marks}>'
        f'<w:tblPrEx/></w:tblPrExChange></w:tblPrEx><w:trPr><w:cantSplit/><w:trPrChange w:id="1" {marks}><w:trPr/></w:trPrChange>'
        f'</w:trPr><w:tc><w:tcPr><w:tcPrChange w:id="2" {marks}><w:tcPr/></w:tcPrChange></w:tcPr><w:p/></w:tc><w:tc><w:tcPr>'
        f'<w:cellIns w:id="6" {marks}/></w:tcPr><w:p/></w:tc><w:tc><w:tcPr><w:cellDel w:id="7" {marks}/></w:tcPr><w:p/></w:tc>'
        '</w:tr></w:tbl><w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/>'
        f'<w:numberingChange w:id="5" {marks} w:original="1."/></w:numPr></w:pPr></w:p>')
    docx.Document().save(tmp_path / "plain.docx")
    with zipfile.ZipFile(tmp_path / "plain.docx") as z:
        parts = {name: z.read(name) for name in z.namelist()}
    main = parts["word/document.xml"].decode()
    start, end = main.index("<w:body>") + len("<w:body>"), main.rindex("<w:sectPr")
    parts["word/document.xml"] = (main[:start] + body + main[end:]).encode()
    with zipfile.ZipFile(tmp_path / "kinds.docx", "w") as z:
        for name, data in parts.items():
            z.writestr(name, data)
    doc = oxdocx.Document.open(tmp_path / "kinds.docx")
    assert [r.kind for r in doc.revisions] == [
        "table_grid_change", "table_property_exception_change", "row_property_change", "cell_property_change",
        "cell_insertion", "cell_deletion", "numbering_change"]

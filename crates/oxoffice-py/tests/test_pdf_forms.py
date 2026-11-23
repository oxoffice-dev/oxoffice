"""to_pdf(fillable_forms=True) exports Word content controls as AcroForm fields that a PDF reader lists, fills and
draws: values live in the widgets' appearances only, and the page draws them exactly as the static export does."""

import io

import pypdf
import pypdfium2
import pytest
from docx import Document as PythonDocxDocument
from docx.oxml import parse_xml

import oxoffice.docx as oxdocx

W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
W14 = 'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"'

# A form as Word saves it. Every field value is unique in the document, so the page text can be searched for it.
BODY = [
    f'<w:p {W}><w:r><w:t xml:space="preserve">Name: </w:t></w:r><w:sdt><w:sdtPr><w:alias w:val="Full name"/>'
    '<w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane Doe</w:t></w:r></w:sdtContent></w:sdt></w:p>',
    f'<w:p {W} {W14}><w:sdt><w:sdtPr><w:tag w:val="agree"/><w14:checkbox><w14:checked w14:val="1"/>'
    '<w14:checkedState w14:val="2612" w14:font="MS Gothic"/><w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/>'
    '</w14:checkbox></w:sdtPr><w:sdtContent><w:r><w:t>☒</w:t></w:r></w:sdtContent></w:sdt>'
    '<w:r><w:t xml:space="preserve"> I agree</w:t></w:r></w:p>',
    f'<w:p {W}><w:r><w:t xml:space="preserve">Country: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="country"/>'
    '<w:dropDownList><w:listItem w:displayText="France" w:value="fr"/><w:listItem w:displayText="Spain" w:value="es"/>'
    '</w:dropDownList></w:sdtPr><w:sdtContent><w:r><w:t>France</w:t></w:r></w:sdtContent></w:sdt></w:p>',
    f'<w:p {W}><w:r><w:t xml:space="preserve">City: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="city"/><w:showingPlcHdr/>'
    '<w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Click here to enter text.</w:t></w:r></w:sdtContent></w:sdt></w:p>',
    f'<w:p {W}><w:sdt><w:sdtPr><w:tag w:val="signature:client"/><w:text/></w:sdtPr><w:sdtContent><w:r>'
    '<w:t>Sign here</w:t></w:r></w:sdtContent></w:sdt></w:p>',
]


@pytest.fixture
def form(tmp_path):
    word = PythonDocxDocument()
    body = word.element.body
    for xml in BODY:
        body.insert(len(body) - 1, parse_xml(xml))
    path = tmp_path / "form.docx"
    word.save(path)
    return oxdocx.Document(path)


def fields(pdf_bytes):
    return pypdf.PdfReader(io.BytesIO(pdf_bytes)).get_fields()


def test_fields_carry_their_names_kinds_and_values(form):
    found = fields(form.to_pdf(fillable_forms=True))
    assert list(found) == ["name", "agree", "country", "city", "signature:client"]
    assert found["name"]["/FT"] == "/Tx" and found["name"]["/V"] == "Jane Doe"
    assert found["name"]["/TU"] == "Full name"
    assert found["agree"]["/FT"] == "/Btn" and found["agree"]["/V"] == "/Yes"
    assert found["country"]["/FT"] == "/Ch" and found["country"]["/V"] == "fr"
    country_flags = int(found["country"].get("/Ff", 0))
    assert country_flags & COMBO and not country_flags & EDIT, "a drop-down list takes no free text"
    assert found["city"].get("/V") is None, "a placeholder is no value"
    assert found["city"]["/TU"] == "Click here to enter text.", "the placeholder is the tooltip"
    assert found["signature:client"]["/FT"] == "/Sig"


def test_without_the_keyword_the_pdf_has_no_field(form):
    assert not fields(form.to_pdf())


def test_field_values_are_not_page_text(form):
    page_text = pypdf.PdfReader(io.BytesIO(form.to_pdf(fillable_forms=True))).pages[0].extract_text()
    assert "Name:" in page_text and "I agree" in page_text
    for value in ("Jane Doe", "France", "Click here", "Sign here"):
        assert value not in page_text, value


def test_a_paragraph_repeating_a_block_control_keeps_its_text(tmp_path):
    word = PythonDocxDocument()
    body = word.element.body
    for xml in [
        f'<w:sdt {W}><w:sdtPr><w:tag w:val="notes"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r>'
        '<w:t>Some notes</w:t></w:r></w:p></w:sdtContent></w:sdt>',
        f'<w:p {W}><w:r><w:t>Some notes</w:t></w:r></w:p>',
    ]:
        body.insert(len(body) - 1, parse_xml(xml))
    path = tmp_path / "repeat.docx"
    word.save(path)
    pdf = oxdocx.Document(path).to_pdf(fillable_forms=True)
    assert fields(pdf)["notes"]["/V"] == "Some notes"
    page_text = pypdf.PdfReader(io.BytesIO(pdf)).pages[0].extract_text()
    assert page_text.count("Some notes") == 1, "the paragraph keeps its text, the field takes its own"


def test_filled_values_read_back(form, tmp_path):
    writer = pypdf.PdfWriter(clone_from=pypdf.PdfReader(io.BytesIO(form.to_pdf(fillable_forms=True))))
    writer.update_page_form_field_values(writer.pages[0], {"name": "Bob Smith", "agree": "/Off", "city": "Lyon"},
                                         auto_regenerate=False)
    out = io.BytesIO()
    writer.write(out)
    found = fields(out.getvalue())
    assert found["name"]["/V"] == "Bob Smith"
    assert found["agree"]["/V"] == "/Off"
    assert found["city"]["/V"] == "Lyon"


def render(pdf_bytes, forms):
    pdf = pypdfium2.PdfDocument(pdf_bytes)
    if forms:
        pdf.init_forms()
    image = pdf[0].render(scale=1, may_draw_forms=forms).to_pil().convert("L")
    pdf.close()
    return image


def field_rects(pdf_bytes, names, height):
    """Pixel boxes (left, top, right, bottom) of named widgets at 72 dpi."""
    rects = []
    for annotation in pypdf.PdfReader(io.BytesIO(pdf_bytes)).pages[0].get("/Annots", []):
        annotation = annotation.get_object()
        if annotation.get("/T") in names:
            x0, y0, x1, y1 = (float(v) for v in annotation["/Rect"])
            rects.append((int(x0) - 1, int(height - y1) - 1, int(x1) + 2, int(height - y0) + 2))
    return rects


def ink(image, box):
    """How many pixels of a box are drawn."""
    return sum(1 for pixel in image.crop(box).tobytes() if pixel < 200)


def test_appearances_draw_what_the_static_export_draws(form):
    fillable = form.to_pdf(fillable_forms=True)
    static = form.to_pdf()
    drawn = render(fillable, forms=True)
    expected = render(static, forms=False)
    empty = field_rects(fillable, {"city", "signature:client"}, drawn.height)
    differing = 0
    for y in range(drawn.height):
        for x in range(drawn.width):
            if any(left <= x < right and top <= y < bottom for left, top, right, bottom in empty):
                continue
            if abs(drawn.getpixel((x, y)) - expected.getpixel((x, y))) > 64:
                differing += 1
    assert differing < 40, f"{differing} pixels differ outside the empty fields"
    for box in empty:
        assert ink(drawn, box) == 0, "a placeholder or signature field shows nothing"


def test_without_forms_drawn_the_field_areas_are_blank(form):
    fillable = form.to_pdf(fillable_forms=True)
    image = render(fillable, forms=False)
    # The widgets still print their appearances, so draw them off: only the page content stays.
    page_only = pypdfium2.PdfDocument(fillable)
    page = page_only[0]
    bitmap = page.render(scale=1, draw_annots=False).to_pil().convert("L")
    page_only.close()
    boxes = field_rects(fillable, {"name", "country"}, image.height)
    assert len(boxes) == 2
    for box in boxes:
        assert ink(bitmap, box) == 0, "a value is drawn by its widget only"


# Annotation flags (PDF 32000-1 12.5.3).
HIDDEN, PRINT, NO_VIEW = 1 << 1, 1 << 2, 1 << 5


def test_every_widget_prints_in_a_box_of_its_own(form):
    widgets = [
        annotation.get_object()
        for annotation in pypdf.PdfReader(io.BytesIO(form.to_pdf(fillable_forms=True))).pages[0]["/Annots"]
    ]
    widgets = [widget for widget in widgets if widget["/Subtype"] == "/Widget"]
    assert len(widgets) == 5
    boxes = []
    for widget in widgets:
        flags = int(widget["/F"])
        assert flags & PRINT and not flags & (HIDDEN | NO_VIEW), "shown and printed"
        x0, y0, x1, y1 = (float(value) for value in widget["/Rect"])
        assert x1 > x0 and y1 > y0
        boxes.append((x0, y0, x1, y1))
    for index, (ax0, ay0, ax1, ay1) in enumerate(boxes):
        for bx0, by0, bx1, by1 in boxes[index + 1 :]:
            overlap = min(ax1, bx1) - max(ax0, bx0) > 0.01 and min(ay1, by1) - max(ay0, by0) > 0.01
            assert not overlap, "widget boxes never overlap"


# Field flags (PDF 32000-1 12.7.3.1, 12.7.4.3, 12.7.4.4).
READ_ONLY, MULTILINE, COMBO, EDIT = 1, 1 << 12, 1 << 17, 1 << 18


def test_each_control_kind_becomes_its_field_kind(tmp_path):
    word = PythonDocxDocument()
    body = word.element.body
    for xml in [
        f'<w:p {W}><w:sdt><w:sdtPr><w:tag w:val="when"/><w:date w:fullDate="2026-11-21T00:00:00Z">'
        '<w:dateFormat w:val="dd/MM/yyyy"/><w:lid w:val="en-GB"/><w:storeMappedDataAs w:val="dateTime"/>'
        '<w:calendar w:val="gregorian"/></w:date></w:sdtPr><w:sdtContent><w:r><w:t>21/11/2026</w:t></w:r>'
        '</w:sdtContent></w:sdt></w:p>',
        f'<w:p {W}><w:sdt><w:sdtPr><w:tag w:val="colour"/><w:comboBox><w:listItem w:displayText="Red" w:value="red"/>'
        '<w:listItem w:displayText="Blue" w:value="blue"/></w:comboBox></w:sdtPr><w:sdtContent><w:r><w:t>Green</w:t>'
        '</w:r></w:sdtContent></w:sdt></w:p>',
        f'<w:sdt {W}><w:sdtPr><w:tag w:val="notes"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>First</w:t>'
        '</w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r></w:p></w:sdtContent></w:sdt>',
        f'<w:p {W}><w:sdt><w:sdtPr><w:tag w:val="locked"/><w:lock w:val="contentLocked"/><w:text/></w:sdtPr>'
        '<w:sdtContent><w:r><w:t>Fixed</w:t></w:r></w:sdtContent></w:sdt></w:p>',
    ]:
        body.insert(len(body) - 1, parse_xml(xml))
    path = tmp_path / "kinds.docx"
    word.save(path)
    found = fields(oxdocx.Document(path).to_pdf(fillable_forms=True))
    flags = {name: int(field.get("/Ff", 0)) for name, field in found.items()}
    assert found["when"]["/FT"] == "/Tx" and found["when"]["/V"] == "21/11/2026"
    assert found["colour"]["/FT"] == "/Ch" and found["colour"]["/V"] == "Green", "a combo box keeps free text"
    assert flags["colour"] & COMBO and flags["colour"] & EDIT
    assert found["notes"]["/FT"] == "/Tx" and flags["notes"] & MULTILINE
    assert found["notes"]["/V"] == "First\nSecond"
    assert flags["locked"] & READ_ONLY
    assert not flags["when"] & READ_ONLY


def test_new_values_replace_the_old_drawing(form):
    writer = pypdf.PdfWriter(clone_from=pypdf.PdfReader(io.BytesIO(form.to_pdf(fillable_forms=True))))
    writer.update_page_form_field_values(writer.pages[0], {"agree": "/Off"}, auto_regenerate=False)
    out = io.BytesIO()
    writer.write(out)
    before = render(form.to_pdf(fillable_forms=True), forms=True)
    after = render(out.getvalue(), forms=True)
    (box,) = field_rects(out.getvalue(), {"agree"}, after.height)
    assert before.crop(box).tobytes() != after.crop(box).tobytes(), "the box shows its other symbol"
    assert ink(after, box) > 0


def test_a_control_left_static_says_so(tmp_path):
    word = PythonDocxDocument()
    body = word.element.body
    body.insert(len(body) - 1, parse_xml(
        f'<w:p {W}><w:r><w:t xml:space="preserve">Notes: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="notes"/><w:text/>'
        '</w:sdtPr><w:sdtContent/></w:sdt></w:p>'
    ))
    path = tmp_path / "empty.docx"
    word.save(path)
    pdf, messages = oxdocx.Document(path).to_pdf_with_diagnostics(fillable_forms=True)
    assert not fields(pdf)
    assert any('"notes" stays static' in message for message in messages), messages


def test_a_legacy_form_field_becomes_a_field(tmp_path):
    word = PythonDocxDocument()
    body = word.element.body
    body.insert(len(body) - 1, parse_xml(
        f'<w:p {W}><w:r><w:t xml:space="preserve">Company: </w:t></w:r><w:bookmarkStart w:id="0" w:name="Company"/>'
        '<w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Company"/><w:enabled/><w:calcOnExit w:val="0"/>'
        '<w:textInput><w:maxLength w:val="40"/></w:textInput></w:ffData></w:fldChar></w:r>'
        '<w:r><w:instrText xml:space="preserve"> FORMTEXT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/>'
        '</w:r><w:r><w:t>Acme</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:bookmarkEnd w:id="0"/></w:p>'
    ))
    path = tmp_path / "legacy.docx"
    word.save(path)
    pdf = oxdocx.Document(path).to_pdf(fillable_forms=True)
    found = fields(pdf)
    assert found["Company"]["/FT"] == "/Tx" and found["Company"]["/V"] == "Acme"
    acroform = pypdf.PdfReader(io.BytesIO(pdf)).trailer["/Root"]["/AcroForm"]
    (company,) = (field.get_object() for field in acroform["/Fields"] if field.get_object()["/T"] == "Company")
    assert company["/MaxLen"] == 40


def test_the_tracked_view_refuses_fillable_forms(form):
    with pytest.raises(ValueError, match="fillable_forms"):
        form.to_pdf(fillable_forms=True, revision_view="tracked")


def test_pdfa_keeps_the_fields(form):
    found = fields(form.to_pdfa_deterministic("pdfa-2b", fillable_forms=True))
    assert len(found) == 5


# veraPDF, pinned by digest, run through Docker on Linux. Elsewhere the check is skipped (the Windows runner's Docker
# runs Windows containers), but a Linux CI run without Docker fails rather than skipping silently.
VERAPDF = "verapdf/cli:v1.30.3@sha256:d74b7026c18a0f171e73c153b617fe6520e1511feb24ca98a348b2bb897669fb"

# The annotation and form clauses: no failure there with fields, whatever the static export does.
FORM_CLAUSES = {"2b": ("6.3", "6.4"), "ua1": ("7.18",)}


def require_verapdf():
    import os
    import shutil
    import sys

    if not sys.platform.startswith("linux"):
        pytest.skip("veraPDF runs through Docker on Linux")
    if shutil.which("docker") is None:
        if os.environ.get("CI"):
            pytest.fail("the PDF/A and PDF/UA check needs Docker on Linux CI")
        pytest.skip("veraPDF runs through Docker")


def verapdf_failures(path, flavour):
    """The rules a PDF fails, as {(clause, test number): failed checks}, from veraPDF's machine-readable report."""
    import os
    import subprocess
    import xml.etree.ElementTree as ElementTree

    # The container's user reads the file through the mounted folder.
    os.chmod(path.parent, 0o755)
    os.chmod(path, 0o644)
    run = subprocess.run(
        ["docker", "run", "--rm", "-v", f"{path.parent}:/data", VERAPDF, "--flavour", flavour, "--format", "mrr",
         f"/data/{path.name}"],
        capture_output=True, encoding="utf-8", timeout=900, check=False,
    )
    try:
        report = ElementTree.fromstring(run.stdout).find("./jobs/job/validationReport")
    except ElementTree.ParseError:
        report = None
    if report is None or report.get("jobEndStatus") != "normal":
        pytest.fail(f"veraPDF gave no readable report for {path.name}: {run.stdout[-2000:]}{run.stderr[-2000:]}")
    return {
        (rule.get("clause"), rule.get("testNumber")): int(rule.get("failedChecks", "0"))
        for rule in report.iter("rule")
        if rule.get("status") == "failed"
    }


@pytest.mark.parametrize("flavour", ["2b", "ua1"])
def test_fields_keep_pdfa_and_pdfua_conformance(form, tmp_path, flavour):
    require_verapdf()
    if flavour == "2b":
        with_fields = form.to_pdfa_deterministic("pdfa-2b", fillable_forms=True)
        without = form.to_pdfa_deterministic("pdfa-2b")
    else:
        with_fields = form.to_pdf(fillable_forms=True)
        without = form.to_pdf()
    (tmp_path / "with.pdf").write_bytes(with_fields)
    (tmp_path / "without.pdf").write_bytes(without)
    with_failures = verapdf_failures(tmp_path / "with.pdf", flavour)
    without_failures = verapdf_failures(tmp_path / "without.pdf", flavour)
    in_form_clauses = {
        rule: checks
        for rule, checks in with_failures.items()
        if any(rule[0] == clause or rule[0].startswith(f"{clause}.") for clause in FORM_CLAUSES[flavour])
    }
    assert not in_form_clauses, f"fields fail {flavour} annotation and form rules: {in_form_clauses}"
    worse = {
        rule: (checks, without_failures.get(rule, 0))
        for rule, checks in with_failures.items()
        if checks > without_failures.get(rule, 0)
    }
    assert not worse, f"rules failing more {flavour} checks with fields than without (with, without): {worse}"

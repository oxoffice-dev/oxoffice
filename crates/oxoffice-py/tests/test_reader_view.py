"""What a user reads shows what a reader sees: Paragraph.runs lists the runs of a simple field typed with tracked
changes on, as Paragraph.text reads its result, as Rust and the CLI do."""

import io
import zipfile
from pathlib import Path

import oxoffice.docx as oxdocx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"

# "B", then a tracked insertion of a QUOTE field reading 12 and of "C".
INSERTED_FIELD = (
    '<w:p><w:r><w:t>B</w:t></w:r><w:ins w:id="901" w:author="a" w:date="2026-01-01T00:00:00Z">'
    '<w:fldSimple w:instr=" QUOTE a "><w:r><w:t>12</w:t></w:r></w:fldSimple>'
    '<w:r><w:t>C</w:t></w:r></w:ins></w:p>'
)


def report_with(extra):
    """The Word report of the corpus with `extra` before the last section properties of its body."""
    data = (CORPUS / "word" / "report.docx").read_bytes()
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(data)) as source, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            part = source.read(item.filename)
            if item.filename == "word/document.xml":
                xml = part.decode("utf-8")
                at = xml.rfind("<w:sectPr")
                part = (xml[:at] + extra + xml[at:]).encode("utf-8")
            target.writestr(item, part)
    return oxdocx.Document.from_bytes(out.getvalue())


def test_a_tracked_insertion_lists_the_runs_of_its_simple_field():
    doc = report_with(INSERTED_FIELD)
    paragraph = next(p for p in doc.paragraphs if p.text == "B12C")
    assert [run.text for run in paragraph.runs] == ["B", "12", "C"]

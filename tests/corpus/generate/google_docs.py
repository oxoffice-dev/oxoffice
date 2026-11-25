"""Documents written in Google Docs itself, exported as .docx, into files/google-docs/.

    OXOFFICE_GOOGLE_TOKEN=<access token> python3 tests/corpus/generate/google_docs.py [family ...]
    python3 tests/corpus/generate/google_docs.py --plan <family> <step> [document.json]

The google/ copies are sources python-docx wrote, converted by Google: they
hold what Google keeps of a Word file. These families are written in Google
Docs, through the Docs API (documents.batchUpdate), as an editor writes them:

- review: a title, headings, bullet, numbered and checkbox lists, links to
  headings and to a site, a person and a date chip, a footnote, a header and a
  footer, a table and a page break; then suggestions, as a reviewer leaves
  them: a word replaced, a paragraph added, two paragraphs joined, a style and
  a bold change.
- comments: comments cannot be written through the Docs API, so a small
  source with comments in every anchor shape (in a run, from a paragraph's
  start, over a whole paragraph or cell, across paragraphs, cells, rows, a
  table's edge, a heading, a link's edge and list items, on a tracked
  insertion and deletion, overlapping, of no length) is uploaded with
  conversion; then suggestions are left inside and around commented text.

Each family is a list of steps. A step is computed from the document as the
Docs API reads it (documents.get), so that indices and the ids Google gives
(headings, footnotes, header and footer) come from the document itself:
`--plan` prints the requests of a step for a document read by hand, to replay
them with another client. The run creates a temporary folder named
oxoffice-corpus-<id> in the Drive of the token, writes there, exports, and
deletes the folder. The access token needs the Drive and Docs scopes, for
example `gcloud auth print-access-token --scopes=https://www.googleapis.com/auth/drive,https://www.googleapis.com/auth/documents`.

What Google writes from them, and what the API cannot write, is in
tests/corpus/README.md.

Standard library only, Python 3.11 or later.
"""

import io
import json
import os
import sys
import time
import urllib.parse
import urllib.request
import uuid
import zipfile
from pathlib import Path

import manifest

OUT = Path(__file__).resolve().parent.parent / "files" / "google-docs"
DOCX = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
PRODUCER = "Google Docs (written through the Docs API)"

# ---- The review: written in Google Docs ----

# (text, named style, list preset or None)
REVIEW = [
    ("Warehouse Review 2026", "TITLE", None),
    ("Summary", "HEADING_1", None),
    ("The northern depot shipped more pallets than planned, and the southern depot cut its waiting times.", "NORMAL_TEXT", None),
    ("Inventory", "HEADING_1", None),
    ("Each site keeps a safety stock sized from the forecast of the coming weeks.", "NORMAL_TEXT", None),
    ("Counts", "HEADING_2", None),
    ("Forecast accuracy improved after the weekly review moved to Monday.", "NORMAL_TEXT", "BULLET_DISC_CIRCLE_SQUARE"),
    ("Stock counts are reconciled every Friday afternoon.", "NORMAL_TEXT", "BULLET_DISC_CIRCLE_SQUARE"),
    ("Checks", "HEADING_2", None),
    ("Check the loading bays.", "NORMAL_TEXT", "BULLET_CHECKBOX"),
    ("Check the fire exits.", "NORMAL_TEXT", "BULLET_CHECKBOX"),
    ("Plan", "HEADING_1", None),
    ("Order the new racks.", "NORMAL_TEXT", "NUMBERED_DECIMAL_ALPHA_ROMAN"),
    ("Train the night shift.", "NORMAL_TEXT", "NUMBERED_DECIMAL_ALPHA_ROMAN"),
    ("Owner: ", "NORMAL_TEXT", None),
    ("Due: ", "NORMAL_TEXT", None),
    ("See the Summary section and the Counts section for the figures.", "NORMAL_TEXT", None),
    ("The schedule lives on the planning site.", "NORMAL_TEXT", None),
    ("Suggested style: this line may become a heading.", "NORMAL_TEXT", None),
    ("Suggested bold: the forklift needs a service.", "NORMAL_TEXT", None),
    ("Suggested join: this paragraph", "NORMAL_TEXT", None),
    ("ends in the next one.", "NORMAL_TEXT", None),
    ("Appendix follows.", "NORMAL_TEXT", None),
    ("Appendix", "HEADING_1", None),
    ("Figures by site.", "NORMAL_TEXT", None),
]
REVIEW_TABLE = [["Site", "Pallets"], ["North", "1,240"], ["South", "980"]]
HEADER = "Warehouse review, internal draft"
FOOTER = "Prepared by the planning team"
FOOTNOTE = "Measured over the last twelve weeks."


def _starts(paragraphs: list[tuple]) -> dict[str, int]:
    """The body index where each paragraph of a fresh document starts."""
    starts, index = {}, 1
    for text, *_ in paragraphs:
        starts[text] = index
        index += len(text) + 1
    return starts


def _at(starts: dict[str, int], paragraph: str, text: str = "") -> int:
    """The index of `text` in `paragraph` (its end when `text` is empty)."""
    return starts[paragraph] + (paragraph.index(text) if text else len(paragraph))


def review_write(_: dict | None) -> list[dict]:
    """Step 1: the text, its styles and lists, a link, the chips, the footnote,
    the header and the footer, the page break and the table. Insertions at an
    index come last, from the end of the body back, so each index holds."""
    starts = _starts(REVIEW)
    requests = [{"insertText": {"location": {"index": 1}, "text": "".join(t + "\n" for t, *_ in REVIEW)}}]
    for text, style, preset in REVIEW:
        span = {"startIndex": starts[text], "endIndex": starts[text] + len(text) + 1}
        requests.append({"updateParagraphStyle": {"range": span, "paragraphStyle": {"namedStyleType": style}, "fields": "namedStyleType"}})
    for preset in ("BULLET_DISC_CIRCLE_SQUARE", "BULLET_CHECKBOX", "NUMBERED_DECIMAL_ALPHA_ROMAN"):
        items = [t for t, _, p in REVIEW if p == preset]
        span = {"startIndex": starts[items[0]], "endIndex": starts[items[-1]] + len(items[-1]) + 1}
        requests.append({"createParagraphBullets": {"range": span, "bulletPreset": preset}})
    site = "The schedule lives on the planning site."
    link = _at(starts, site, "planning site")
    requests.append({"updateTextStyle": {"range": {"startIndex": link, "endIndex": link + len("planning site")},
                                         "textStyle": {"link": {"url": "https://example.com/planning"}}, "fields": "link"}})
    requests.append({"insertTable": {"rows": len(REVIEW_TABLE), "columns": 2, "endOfSegmentLocation": {}}})
    requests.append({"createHeader": {"type": "DEFAULT"}})
    requests.append({"createFooter": {"type": "DEFAULT"}})
    at_index = [
        (_at(starts, "Appendix follows."), {"insertPageBreak": {"location": {"index": _at(starts, "Appendix follows.")}}}),
        (_at(starts, "Due: "), {"insertDate": {"location": {"index": _at(starts, "Due: ")},
                                                "dateElementProperties": {"timestamp": "2026-12-01T09:00:00Z"}}}),
        (_at(starts, "Owner: "), {"insertPerson": {"location": {"index": _at(starts, "Owner: ")},
                                                    "personProperties": {"email": "planner@example.com"}}}),
        (_at(starts, REVIEW[4][0], "."), {"createFootnote": {"location": {"index": _at(starts, REVIEW[4][0], ".")}}}),
    ]
    requests += [request for _, request in sorted(at_index, key=lambda item: -item[0])]
    return requests


def _segments(document: dict) -> dict:
    tab = document["tabs"][0]["documentTab"] if "tabs" in document else document
    return tab


def _paragraphs(content: list[dict]):
    """(start index, text, paragraph) of every paragraph, table cells included."""
    for element in content:
        if "paragraph" in element:
            runs = element["paragraph"]["elements"]
            text = "".join(run.get("textRun", {}).get("content", "\0") for run in runs)
            yield element["startIndex"], text, element["paragraph"]
        elif "table" in element:
            for row in element["table"]["tableRows"]:
                for cell in row["tableCells"]:
                    yield from _paragraphs(cell["content"])


def _find(document: dict, text: str, within: str | None = None) -> int:
    """The body index of `text`, in the paragraph that holds `within` (or it)."""
    for start, paragraph, _ in _paragraphs(_segments(document)["body"]["content"]):
        if (within or text) in paragraph:
            return start + paragraph.index(text)
    raise LookupError(f"no paragraph holds {within or text!r}")


def _heading_id(document: dict, text: str) -> str:
    for _, paragraph, properties in _paragraphs(_segments(document)["body"]["content"]):
        if paragraph.rstrip("\n") == text and "headingId" in properties["paragraphStyle"]:
            return properties["paragraphStyle"]["headingId"]
    raise LookupError(f"no heading reads {text!r}")


def review_fill(document: dict) -> list[dict]:
    """Step 2: the footnote, header and footer text, the table cells, and the
    links to two headings, from the ids and indices Google gave."""
    tab = _segments(document)
    requests = [
        {"insertText": {"endOfSegmentLocation": {"segmentId": next(iter(tab["footnotes"]))}, "text": FOOTNOTE}},
        {"insertText": {"endOfSegmentLocation": {"segmentId": tab["documentStyle"]["defaultHeaderId"]}, "text": HEADER}},
        {"insertText": {"endOfSegmentLocation": {"segmentId": tab["documentStyle"]["defaultFooterId"]}, "text": FOOTER}},
    ]
    for heading in ("Summary", "Counts"):
        start = _find(document, heading, "See the Summary section")
        requests.append({"updateTextStyle": {"range": {"startIndex": start, "endIndex": start + len(heading)},
                                             "textStyle": {"link": {"headingId": _heading_id(document, heading)}},
                                             "fields": "link"}})
    table = next(element["table"] for element in tab["body"]["content"] if "table" in element)
    cells = [cell["content"][0]["startIndex"] for row in table["tableRows"] for cell in row["tableCells"]]
    texts = [text for row in REVIEW_TABLE for text in row]
    for start, text in sorted(zip(cells, texts), reverse=True):
        requests.append({"insertText": {"location": {"index": start}, "text": text}})
    return requests


def review_suggest(document: dict) -> list[dict]:
    """Step 3, in suggestion mode: a word replaced, a paragraph added, two
    paragraphs joined, a style and a bold change, from the end of the body back."""
    friday = _find(document, "Friday")
    join = _find(document, "\n", "Suggested join: this paragraph")
    added = _find(document, "Appendix follows.")
    bold = _find(document, "the forklift needs a service")
    style = _find(document, "Suggested style: this line may become a heading.")
    return [
        {"insertText": {"location": {"index": added}, "text": "A suggested new paragraph.\n"}},
        {"deleteContentRange": {"range": {"startIndex": join, "endIndex": join + 1}}},
        {"updateTextStyle": {"range": {"startIndex": bold, "endIndex": bold + len("the forklift needs a service")},
                             "textStyle": {"bold": True}, "fields": "bold"}},
        {"updateParagraphStyle": {"range": {"startIndex": style, "endIndex": style + 1},
                                  "paragraphStyle": {"namedStyleType": "HEADING_2"}, "fields": "namedStyleType"}},
        {"deleteContentRange": {"range": {"startIndex": friday, "endIndex": friday + len("Friday")}}},
        {"insertText": {"location": {"index": friday}, "text": "Thursday"}},
    ]


# ---- The comments: a converted source, then suggestions in Google Docs ----

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
DATE = "2026-11-21T09:00:00Z"


def _run(text: str) -> str:
    return f'<w:r><w:t xml:space="preserve">{text}</w:t></w:r>'


def _start(cid: int) -> str:
    return f'<w:commentRangeStart w:id="{cid}"/>'


def _end(cid: int) -> str:
    return f'<w:commentRangeEnd w:id="{cid}"/><w:r><w:commentReference w:id="{cid}"/></w:r>'


def _p(content: str, style: str = "") -> str:
    properties = f'<w:pPr><w:pStyle w:val="{style}"/></w:pPr>' if style else ""
    return f"<w:p>{properties}{content}</w:p>"


def _table(rows: list[list[str]]) -> str:
    cell = '<w:tc><w:tcPr><w:tcW w:w="4000" w:type="dxa"/></w:tcPr>{}</w:tc>'
    return ('<w:tbl><w:tblPr><w:tblW w:w="8000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="4000"/>'
            '<w:gridCol w:w="4000"/></w:tblGrid>'
            + "".join("<w:tr>" + "".join(cell.format(c) for c in row) + "</w:tr>" for row in rows) + "</w:tbl>")


def _revision(kind: str, rid: int, content: str) -> str:
    return f'<w:{kind} w:id="{rid}" w:author="Bob Chen" w:date="{DATE}">{content}</w:{kind}>'


COMMENTS_BODY = "".join([
    _p(_run("The depot ") + _start(1) + _run("opens early") + _end(1) + _run(" on Mondays.")),
    _p(_start(2) + _run("Trucks leave") + _end(2) + _run(" at noon.")),
    _p(_start(3) + _run("Every pallet is scanned.") + _end(3)),
    _p(_run("Stock is ") + _start(4) + _run("counted on Friday.")),
    _p(_run("The count") + _end(4) + _run(" is sent to the planners.")),
    _p(_run("Forecasts ") + _start(5) + _run("come from ") + _start(6) + _run("the sales") + _end(5) + _run(" team")
       + _end(6) + _run(" each week.")),
    _p(_run("A point comment") + _start(7) + '<w:commentRangeEnd w:id="7"/><w:r><w:commentReference w:id="7"/></w:r>'
       + _run(" sits here.")),
    _p(_run("Shifts start ") + _revision("ins", 101, _start(8) + _run("an hour earlier") + '<w:commentRangeEnd w:id="8"/>')
       + '<w:r><w:commentReference w:id="8"/></w:r>' + _run(" in summer.")),
    _p(_run("Breaks last ") + _start(9) + _revision("del", 102, '<w:r><w:delText>twenty</w:delText></w:r>')
       + _revision("ins", 103, _run("thirty")) + _end(9) + _run(" minutes.")),
    _p(_run("The first table follows.")),
    _table([[_p(_start(10) + _run("Preconditions and test data")), _p(_run("the second cell") + _end(10))],
            [_p(_run("Alpha ") + _start(11) + _run("cell")), _p(_run("Beta cell"))],
            [_p(_run("Gamma cell")), _p(_run("Delta") + _end(11) + _run(" cell"))],
            [_p(_start(12) + _run("Whole cell") + _end(12)), _p(_run("Other cell"))]]),
    _p(_run("Around the ") + _start(13) + _run("second table.")),
    _table([[_p(_run("Epsilon cell")), _p(_run("Zeta cell"))]]),
    _p(_run("After") + _end(13) + _run(" the second table.")),
    _p(_start(14) + _run("A heading"), "Heading1"),
    _p(_run("Its first") + _end(14) + _run(" paragraph.")),
    _p(_run("Read the ") + '<w:hyperlink r:id="rIdLink">' + _run("planning ") + _start(15) + _run("guide")
       + "</w:hyperlink>" + _run(" before") + _end(15) + _run(" Monday.")),
    _p(_run("First ") + _start(16) + _run("item"), "ListParagraph"),
    _p(_run("Second") + _end(16) + _run(" item"), "ListParagraph"),
    _p(_run("Closing line.")),
])
COMMENT_AUTHORS = {cid: ("Alice Martin", "AM") if cid % 2 else ("Bob Chen", "BC") for cid in range(1, 17)}


def comments_source() -> bytes:
    """The source the comments family uploads, the same bytes on every run."""
    comments = "".join(
        f'<w:comment w:id="{cid}" w:author="{author}" w:date="{DATE}" w:initials="{initials}">'
        f"<w:p><w:r><w:t>Remark {cid}.</w:t></w:r></w:p></w:comment>"
        for cid, (author, initials) in COMMENT_AUTHORS.items()
    )
    styles = ('<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>'
              '<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/>'
              '<w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>'
              '<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/>'
              '<w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720"/></w:pPr></w:style>')
    declaration = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    relationships = "http://schemas.openxmlformats.org/package/2006/relationships"
    office = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
    parts = {
        "[Content_Types].xml": declaration + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
        '<Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/>'
        "</Types>",
        "_rels/.rels": declaration + f'<Relationships xmlns="{relationships}">'
        f'<Relationship Id="rId1" Type="{office}/officeDocument" Target="word/document.xml"/></Relationships>',
        "word/_rels/document.xml.rels": declaration + f'<Relationships xmlns="{relationships}">'
        f'<Relationship Id="rId1" Type="{office}/styles" Target="styles.xml"/>'
        f'<Relationship Id="rId2" Type="{office}/comments" Target="comments.xml"/>'
        f'<Relationship Id="rIdLink" Type="{office}/hyperlink" Target="https://example.com/plan" TargetMode="External"/>'
        "</Relationships>",
        "word/document.xml": declaration + f'<w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{COMMENTS_BODY}'
        '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/>'
        "</w:sectPr></w:body></w:document>",
        "word/styles.xml": declaration + f'<w:styles xmlns:w="{W}">{styles}</w:styles>',
        "word/comments.xml": declaration + f'<w:comments xmlns:w="{W}">{comments}</w:comments>',
    }
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            info = zipfile.ZipInfo(name, (2026, 11, 21, 9, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            package.writestr(info, data)
    return out.getvalue()


def comments_suggest(document: dict) -> list[dict]:
    """In suggestion mode: a word replaced inside a commented range, a word
    added where a comment starts, the first word of a commented paragraph
    deleted, a word replaced where a comment over two paragraphs starts, from
    the end of the body back."""
    edits = [
        (_find(document, "Friday"), "Friday", "Thursday"),
        (_find(document, "Every "), "Every ", ""),
        (_find(document, "Trucks"), "", "All "),
        (_find(document, "early"), "early", "late"),
    ]
    requests = []
    for start, old, new in sorted(edits, key=lambda edit: -edit[0]):
        if old:
            requests.append({"deleteContentRange": {"range": {"startIndex": start, "endIndex": start + len(old)}}})
        if new:
            requests.append({"insertText": {"location": {"index": start}, "text": new}})
    return requests


# (step name, plan from the document read before it, suggestion mode)
FAMILIES = {
    "review": [("write", review_write, False), ("fill", review_fill, False), ("suggest", review_suggest, True)],
    "comments": [("suggest", comments_suggest, True)],
}
UPLOADED = {"comments": comments_source}


# ---- The Drive and Docs APIs ----

def _call(method: str, url: str, token: str, body: bytes | None = None, content_type: str = "application/json") -> bytes:
    request = urllib.request.Request(url, data=body, method=method, headers={"Authorization": f"Bearer {token}"})
    if body is not None:
        request.add_header("Content-Type", content_type)
    with urllib.request.urlopen(request, timeout=120) as response:
        return response.read()


def _json(method: str, url: str, token: str, body: dict | None = None) -> dict:
    data = _call(method, url, token, None if body is None else json.dumps(body).encode())
    return json.loads(data) if data else {}


def _create(token: str, name: str, folder: str, source: bytes | None) -> str:
    google_doc = "application/vnd.google-apps.document"
    if source is None:
        return _json("POST", "https://www.googleapis.com/drive/v3/files", token,
                     {"name": name, "mimeType": google_doc, "parents": [folder]})["id"]
    boundary = uuid.uuid4().hex
    metadata = json.dumps({"name": name, "mimeType": google_doc, "parents": [folder]}).encode()
    body = (f"--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n".encode() + metadata
            + f"\r\n--{boundary}\r\nContent-Type: {DOCX}\r\n\r\n".encode() + source + f"\r\n--{boundary}--".encode())
    data = _call("POST", "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart", token, body,
                 f"multipart/related; boundary={boundary}")
    return json.loads(data)["id"]


def write(family: str, token: str, folder: str) -> Path:
    upload = UPLOADED.get(family)
    document_id = _create(token, family, folder, upload() if upload else None)
    docs = f"https://docs.googleapis.com/v1/documents/{document_id}"
    for name, plan, suggest in FAMILIES[family]:
        document = _json("GET", docs, token)
        body = {"requests": plan(document)}
        if suggest:
            body["writeControl"] = {"writeMode": "SUGGEST"}
        _json("POST", f"{docs}:batchUpdate", token, body)
        print(f"google-docs: {family} {name}")
    exported = _call("GET", f"https://www.googleapis.com/drive/v3/files/{document_id}/export?"
                     + urllib.parse.urlencode({"mimeType": DOCX}), token)
    OUT.mkdir(exist_ok=True)
    path = OUT / f"{family}.docx"
    path.write_bytes(exported)
    return path


def main() -> None:
    args = sys.argv[1:]
    if args[:1] == ["--plan"]:
        family, step = args[1], args[2]
        document = json.loads(Path(args[3]).read_text()) if len(args) > 3 else None
        plan = next(plan for name, plan, _ in FAMILIES[family] if name == step)
        print(json.dumps(plan(document)))
        return
    if args[:1] == ["--source"]:
        sys.stdout.buffer.write(UPLOADED[args[1]]())
        return
    token = os.environ.get("OXOFFICE_GOOGLE_TOKEN") or sys.exit("set OXOFFICE_GOOGLE_TOKEN (see the docstring)")
    started = time.time()
    folder = _json("POST", "https://www.googleapis.com/drive/v3/files", token,
                   {"name": f"oxoffice-corpus-{uuid.uuid4().hex[:8]}", "mimeType": "application/vnd.google-apps.folder"})["id"]
    try:
        for family in args or FAMILIES:
            write(family, token, folder)
    finally:
        _call("DELETE", f"https://www.googleapis.com/drive/v3/files/{folder}", token)
    manifest.update({("google-docs", "docx"): PRODUCER}, since=started)


if __name__ == "__main__":
    main()

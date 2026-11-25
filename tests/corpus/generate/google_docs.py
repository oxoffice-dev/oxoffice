"""Documents written in Google Docs itself, exported as .docx, into files/google-docs/.

    OXOFFICE_GOOGLE_TOKEN=<access token> python3 tests/corpus/generate/google_docs.py [family ...]

The google/ copies are sources python-docx wrote, converted by Google. These
are written in Google Docs, through the Docs API (documents.batchUpdate),
replaying the requests of google_docs.json:

- review: a title, headings, bullet, numbered and checkbox lists, links to
  headings and to a site, a person and a date chip, a footnote, a header and
  a footer, a table and a page break; then suggestions: a word replaced, a
  paragraph added, two paragraphs joined, a style and a bold change.
- comments: the Docs API writes no comment, so a source with comments in
  every anchor shape (its parts are in the JSON file) is uploaded with
  conversion; then suggestions inside and around commented text.

A step is a list of requests, sent in suggestion mode when it says so. The
indices are those of a document the steps before wrote; the ids Google gives
are read from the document (documents.get) in place of `@footnote`,
`@header`, `@footer` and `@heading:<text>`. The run works in a temporary
folder oxoffice-corpus-<id> of the token's Drive, which it deletes. The token
needs the Drive and Docs scopes (`gcloud auth print-access-token`). Google
writes the account's name as the author of every suggestion: NEUTRAL_AUTHOR
replaces it after the export.

Standard library only, Python 3.11 or later.
"""

import io
import json
import os
import re
import sys
import time
import urllib.parse
import urllib.request
import uuid
import zipfile
from pathlib import Path

import manifest

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "files" / "google-docs"
DOCX = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
PRODUCER = "Google Docs (written through the Docs API, author names neutral)"
NEUTRAL_AUTHOR = "Corpus Author"
DRIVE, DOCS = "https://www.googleapis.com/drive/v3/files", "https://docs.googleapis.com/v1/documents"


def call(method: str, url: str, token: str, body: bytes | None = None, kind: str = "application/json") -> bytes:
    request = urllib.request.Request(url, data=body, method=method, headers={"Authorization": f"Bearer {token}"})
    if body is not None:
        request.add_header("Content-Type", kind)
    with urllib.request.urlopen(request, timeout=120) as response:
        return response.read()


def call_json(method: str, url: str, token: str, body: dict | None = None) -> dict:
    data = call(method, url, token, None if body is None else json.dumps(body).encode())
    return json.loads(data) if data else {}


def source_docx(parts: dict[str, str]) -> bytes:
    """A package of these parts, the same bytes on every run."""
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in parts.items():
            package.writestr(zipfile.ZipInfo(name, (2026, 11, 21, 9, 0, 0)), data, zipfile.ZIP_DEFLATED)
    return out.getvalue()


def ids(document: dict) -> dict[str, str]:
    """The values of the placeholders, from the document as the API reads it."""
    tab = document["tabs"][0]["documentTab"]
    found = {"@footnote": next(iter(tab.get("footnotes", {})), ""),
             "@header": tab["documentStyle"].get("defaultHeaderId", ""),
             "@footer": tab["documentStyle"].get("defaultFooterId", "")}
    for element in tab["body"]["content"]:
        paragraph = element.get("paragraph", {})
        if "headingId" in paragraph.get("paragraphStyle", {}):
            text = "".join(run.get("textRun", {}).get("content", "") for run in paragraph["elements"]).strip()
            found[f"@heading:{text}"] = paragraph["paragraphStyle"]["headingId"]
    return found


def placed(value, found: dict[str, str]):
    """`value` with every placeholder string replaced by what it names."""
    if isinstance(value, dict):
        return {key: placed(item, found) for key, item in value.items()}
    if isinstance(value, list):
        return [placed(item, found) for item in value]
    return found[value] if isinstance(value, str) and value.startswith("@") else value


def neutral_authors(path: Path, account: str) -> int:
    """Write NEUTRAL_AUTHOR in place of the account's name in every `w:author`
    of the package at `path`, "CA" in the `w:initials` of the same element,
    and return how many tags changed. Every other byte of each part is kept."""
    escaped = account.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")
    tag = re.compile(rb'<[^<>]*\bw:author="' + re.escape(escaped.encode()) + rb'"[^<>]*>')

    def neutral(match: re.Match) -> bytes:
        written = match.group(0).replace(f'w:author="{escaped}"'.encode(), f'w:author="{NEUTRAL_AUTHOR}"'.encode())
        return re.sub(rb'\bw:initials="[^"]*"', b'w:initials="CA"', written)

    with zipfile.ZipFile(path) as package:
        infos = package.infolist()
        parts = {info.filename: package.read(info) for info in infos}
    changed = 0
    for name, data in parts.items():
        if name.endswith(".xml"):
            parts[name], count = tag.subn(neutral, data)
            changed += count
    if changed:
        with zipfile.ZipFile(path, "w") as package:
            for info in infos:
                package.writestr(info, parts[info.filename])
    return changed


def write(family: str, spec: dict, token: str, folder: str) -> Path:
    metadata = {"name": family, "mimeType": "application/vnd.google-apps.document", "parents": [folder]}
    if "source" in spec:
        boundary = uuid.uuid4().hex
        body = (f"--{boundary}\r\nContent-Type: application/json\r\n\r\n{json.dumps(metadata)}\r\n"
                f"--{boundary}\r\nContent-Type: {DOCX}\r\n\r\n").encode() + source_docx(spec["source"]) \
            + f"\r\n--{boundary}--".encode()
        upload = "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart"
        document_id = json.loads(call("POST", upload, token, body, f"multipart/related; boundary={boundary}"))["id"]
    else:
        document_id = call_json("POST", DRIVE, token, metadata)["id"]
    for name, suggest, requests in spec["steps"]:
        body = {"requests": placed(requests, ids(call_json("GET", f"{DOCS}/{document_id}", token)))}
        if suggest:
            body["writeControl"] = {"writeMode": "SUGGEST"}
        call_json("POST", f"{DOCS}/{document_id}:batchUpdate", token, body)
        print(f"google-docs: {family} {name}")
    OUT.mkdir(exist_ok=True)
    path = OUT / f"{family}.docx"
    path.write_bytes(call("GET", f"{DRIVE}/{document_id}/export?" + urllib.parse.urlencode({"mimeType": DOCX}), token))
    account = call_json("GET", "https://www.googleapis.com/drive/v3/about?fields=user", token)["user"]["displayName"]
    neutral_authors(path, account)
    return path


def main() -> None:
    token = os.environ.get("OXOFFICE_GOOGLE_TOKEN") or sys.exit("set OXOFFICE_GOOGLE_TOKEN (see the docstring)")
    families = json.loads((HERE / "google_docs.json").read_text())
    started = time.time()
    folder = call_json("POST", DRIVE, token, {"name": f"oxoffice-corpus-{uuid.uuid4().hex[:8]}",
                                              "mimeType": "application/vnd.google-apps.folder"})["id"]
    try:
        for family in sys.argv[1:] or families:
            write(family, families[family], token, folder)
    finally:
        call("DELETE", f"{DRIVE}/{folder}", token)
    manifest.update({("google-docs", "docx"): PRODUCER}, since=started)


if __name__ == "__main__":
    main()

"""Write manifest.toml from the corpus files themselves.

    python3 tests/corpus/generate/manifest.py            # rewrite manifest.toml
    python3 tests/corpus/generate/manifest.py --check    # fail if it is out of date or over budget

One entry per file under files/: the producer that wrote it, its size, and the
features found in its XML (so an entry says what the file carries, not what
its source meant to carry: a producer that drops content controls shows it).
Producer versions are kept from the current manifest unless a producer run
passes new ones (author.py, produce.py) for the files it wrote. The
`known_failure = <issue>` and `rewritten = [...]` lines are kept: the round-trip
tests then expect a save to rewrite exactly those parts of that file, and
nothing worse, until the issue is fixed.

Standard library only, Python 3.11 or later.
"""

import re
import sys
import tomllib
import zipfile
from pathlib import Path

CORPUS = Path(__file__).resolve().parent.parent
FILES = CORPUS / "files"
MANIFEST = CORPUS / "manifest.toml"
MAX_FILE = 1024 * 1024
MAX_TOTAL = 20 * 1024 * 1024
EXTENSIONS = ("docx", "pptx")

HEADER = """\
# Written by generate/manifest.py from the files under files/: do not edit by
# hand, except known_failure and rewritten. Every file is ours: authored by
# generate/author.py, then opened and saved by the producer named here
# (generate/produce.py), or written in Google Docs or by hand in Word (see
# the README). They are released under the repository's licence.
#
# features: what the file's XML carries, so a failure points at an area.
# known_failure: the issue that makes a save without an edit rewrite the parts
# listed in `rewritten` today; the file must still open, save, reopen and keep
# every part, and no other part may change.
"""

CJK = re.compile(r"[぀-ヿ一-鿿가-힯]")


def _xml(package: zipfile.ZipFile, prefix: str) -> str:
    return "\n".join(
        package.read(name).decode("utf-8", "replace")
        for name in package.namelist()
        if name.startswith(prefix) and name.endswith(".xml")
    )


def _max_depth(xml: str, tag: str) -> int:
    depth = deepest = 0
    for match in re.finditer(rf"<(/?){tag}[ >]", xml):
        depth += -1 if match.group(1) else 1
        deepest = max(deepest, depth)
    return deepest


def _list_levels(xml: str) -> set[tuple[str, int]]:
    """(numId, ilvl) of every numbering reference in this XML, level 0 when none is given."""
    return {
        (num, int(ilvl or 0))
        for ilvl, num in re.findall(r'<w:numPr>(?:<w:ilvl w:val="(\d)"/>)?<w:numId w:val="(\d+)"/>', xml)
        if num != "0"
    }


def _number_format(numbering: str, num: str, ilvl: int) -> str | None:
    """The number format of level `ilvl` of list `num`, through its abstract definition."""
    abstract = re.search(rf'<w:num w:numId="{num}"[^>]*>.*?<w:abstractNumId w:val="(\d+)"/>', numbering, re.S)
    if not abstract:
        return None
    definition = re.search(rf'<w:abstractNum [^>]*w:abstractNumId="{abstract.group(1)}".*?</w:abstractNum>', numbering, re.S)
    level = definition and re.search(rf'<w:lvl w:ilvl="{ilvl}".*?<w:numFmt w:val="([^"]*)"', definition.group(0), re.S)
    return level.group(1) if level else None


def _signature_control(stories: str) -> bool:
    """A control PDF form export turns into a signature field: a plain text,
    rich text or picture control tagged `signature` or `signature:<name>`,
    whatever the ASCII case (a placeholder's `w:docPart` is not a kind)."""
    for properties in re.findall(r"<w:sdtPr>(.*?)</w:sdtPr>", stories, re.S):
        tagged = re.search(r'<w:tag w:val="(?ai:signature)(:[^"]*)?"', properties)
        other_kind = re.search(r"<(w:date|w:dropDownList|w:comboBox|w14:checkbox|w:docPart(?:Obj|List)|w:group|w:equation"
                               r"|w:citation|w:bibliography|w15:repeatingSection\w*)\b", properties)
        if tagged and not other_kind:
            return True
    return False


def docx_features(package: zipfile.ZipFile) -> list[str]:
    names = package.namelist()
    body = package.read("word/document.xml").decode("utf-8", "replace")
    stories = _xml(package, "word/")
    numbering = _xml(package, "word/numbering.xml")
    styles = _xml(package, "word/styles.xml")
    settings = _xml(package, "word/settings.xml")
    notes = _xml(package, "word/footnotes.xml") + _xml(package, "word/endnotes.xml")
    text = "".join(re.findall(r"<w:t(?: [^>]*)?>([^<]*)", body))
    # Style ids are localised (Word in French writes Titre1), their names are not.
    style_names, style_levels = {}, {}
    for style_id, definition in re.findall(r'<w:style [^>]*w:styleId="([^"]*)"(.*?)</w:style>', styles, re.S):
        name = re.search(r'<w:name w:val="([^"]*)"', definition)
        style_names[style_id] = name.group(1).lower() if name else ""
        style_levels[style_id] = _list_levels(definition)
    used_ids = set(re.findall(r'<w:pStyle w:val="([^"]*)"', body))
    used = {style_names.get(i, i.lower()) for i in used_ids}
    # The list levels the body uses, directly or through a paragraph style, and their number formats.
    levels = _list_levels(body) | {level for i in used_ids for level in style_levels.get(i, set())}
    formats = {_number_format(numbering, num, ilvl) for num, ilvl in levels}
    instructions = " ".join(
        re.findall(r"<w:instrText[^>]*>([^<]*)", stories) + re.findall(r'w:instr="([^"]*)"', stories)
    ).upper()

    found = {
        "headings": any(re.fullmatch(r"heading \d", name) for name in used),
        "title": "title" in used,
        "bullet-list": "bullet" in formats,
        "numbered-list": any(f and f not in ("bullet", "none") for f in formats),
        "multi-level-list": any(ilvl > 0 for _, ilvl in levels),
        "table": "<w:tbl>" in body,
        "merged-cells": "<w:gridSpan " in body or "<w:vMerge" in body,
        "nested-table": _max_depth(body, "w:tbl") > 1,
        "picture": "<pic:pic" in stories,
        "floating-picture": "<wp:anchor" in stories and "<pic:pic" in stories,
        "text-box": "<wps:txbx" in stories or "<v:textbox" in stories,
        "shape": "<wps:wsp" in stories,
        "chart": any(name.startswith("word/charts/") for name in names),
        "comment": "<w:comment " in _xml(package, "word/comments.xml"),
        "comment-reply": "w15:paraIdParent" in _xml(package, "word/commentsExtended.xml"),
        "footnote": re.search(r"<w:footnote (?![^>]*w:type)", notes),
        "endnote": re.search(r"<w:endnote (?![^>]*w:type)", notes),
        "tracked-changes": re.search(r"<w:(ins|del) ", stories),
        "format-change": re.search(r"<w:(rPrChange|pPrChange) ", stories),
        "header-footer": any(re.match(r"word/(header|footer)\d*\.xml", name) for name in names),
        "sections": body.count("<w:sectPr") > 1,
        "landscape": 'w:orient="landscape"' in body,
        "columns": re.search(r'<w:cols [^>]*w:num="[2-9]"', body),
        "title-page": "<w:titlePg" in body,
        "page-number-format": re.search(r'<w:pgNumType [^>]*w:fmt="(?!decimal")', body),
        "even-odd-headers": "<w:evenAndOddHeaders" in settings,
        "toc": re.search(r"\bTOC\b", instructions),
        "page-fields": re.search(r"\b(PAGE|NUMPAGES|SECTIONPAGES)\b", instructions),
        "ref-fields": re.search(r"\b(REF|PAGEREF|NOTEREF)\b", instructions),
        "seq-fields": re.search(r"\bSEQ\b", instructions),
        "date-field": re.search(r"\b(DATE|TIME|CREATEDATE|SAVEDATE)\b", instructions),
        "merge-fields": re.search(r"\bMERGEFIELD\b", instructions),
        "form-fields": re.search(r"\bFORM(TEXT|CHECKBOX|DROPDOWN)\b", instructions),
        "bookmark": re.search(r'<w:bookmarkStart [^>]*w:name="(?!_GoBack)', stories),
        "hyperlink": "<w:hyperlink " in stories,
        "content-control": "<w:sdt>" in stories,
        "checkbox-control": "<w14:checkbox" in stories,
        "date-control": "<w:date " in stories or "<w:date>" in stories,
        "list-control": "<w:dropDownList" in stories or "<w:comboBox" in stories,
        # The tag PDF form export reads as a signature field, and a locked control.
        "signature-control": _signature_control(stories),
        "locked-control": re.search(r'<w:lock w:val="(contentLocked|sdtContentLocked)"', stories),
        "equation": "<m:oMath" in stories,
        "rtl": "<w:bidi/>" in stories or "<w:rtl/>" in stories or '<w:bidi w:val="1"' in stories,
        "cjk": CJK.search(text),
        "protection": "<w:documentProtection " in settings,
        "rsid": "w:rsidR=" in body,
        "paraId": "w14:paraId=" in body,
        "embedded-fonts": any(name.startswith("word/fonts/") for name in names),
        "long": body.count("<w:p>") + body.count("<w:p ") > 2000,
    }
    return [name for name, present in found.items() if present]


def _sixteen_by_nine(presentation: str) -> bool:
    """A 16:9 slide, within 0.1 %: producers round it (12192000 x 6858000, the
    12191695 of 13.333 inches, python-pptx's 13.33 inches)."""
    size = re.search(r"<p:sldSz\s[^>]*>", presentation)
    cx = size and re.search(r'\bcx="(\d+)"', size.group(0))
    cy = size and re.search(r'\bcy="(\d+)"', size.group(0))
    if not (cx and cy):
        return False
    width, height = int(cx.group(1)), int(cy.group(1))
    return abs(width * 9 - height * 16) <= height * 16 // 1000


def pptx_features(package: zipfile.ZipFile) -> list[str]:
    names = package.namelist()
    slides = _xml(package, "ppt/slides/")
    presentation = _xml(package, "ppt/presentation.xml")
    slide_count = sum(1 for name in names if re.match(r"ppt/slides/slide\d+\.xml$", name))

    found = {
        "layouts": sum(1 for name in names if re.match(r"ppt/slideLayouts/slideLayout\d+\.xml$", name)) > 1,
        "masters": sum(1 for name in names if re.match(r"ppt/slideMasters/slideMaster\d+\.xml$", name)) > 1,
        "placeholders": "<p:ph" in slides,
        "bullet-levels": re.search(r'<a:pPr [^>]*lvl="[1-8]"', slides),
        "notes": any(name.startswith("ppt/notesSlides/") for name in names),
        "table": "<a:tbl>" in slides,
        "merged-cells": re.search(r'(gridSpan|rowSpan|hMerge|vMerge)="', slides),
        "picture": "<p:pic>" in slides,
        "chart": any(name.startswith("ppt/charts/") for name in names),
        "group": "<p:grpSp>" in slides,
        "connector": "<p:cxnSp>" in slides,
        "auto-shape": re.search(r'<a:prstGeom prst="(?!rect")', slides),
        "text-box": 'txBox="1"' in slides,
        "hyperlink": re.search(r'<a:hlinkClick [^>]*r:id="rId', slides),
        # An effect, not LibreOffice's empty <p:transition spd="slow"> on every slide.
        "transition": re.search(r"<p:transition\b[^>]*(?<!/)>\s*<(?!p:sndAc\b|p:extLst\b)\w+:", slides),
        "hidden-slide": re.search(r'<p:sld [^>]*show="0"', slides),
        "sections": "sectionLst" in presentation,
        "comments": any(name.startswith("ppt/comments/") for name in names),
        "slide-number": 'type="slidenum"' in slides or 'type="sldNum"' in slides,
        "rtl": 'rtl="1"' in slides,
        "cjk": CJK.search("".join(re.findall(r"<a:t>([^<]*)", slides))),
        "widescreen": _sixteen_by_nine(presentation),
        "embedded-fonts": any(name.startswith("ppt/fonts/") for name in names),
        "long": slide_count >= 40,
    }
    return [name for name, present in found.items() if present]


def features(path: Path) -> list[str]:
    with zipfile.ZipFile(path) as package:
        return docx_features(package) if path.suffix == ".docx" else pptx_features(package)


def current() -> dict[str, dict]:
    if not MANIFEST.exists():
        return {}
    return {entry["path"]: entry for entry in tomllib.loads(MANIFEST.read_text())["file"]}


def entries(versions: dict[tuple[str, str], str] | None = None, since: float = 0.0) -> list[dict]:
    """One entry per corpus file. `versions` maps (producer folder, extension)
    to the version string of a producer that has just run; it is given to the
    files written since `since` (a timestamp), the others keep theirs."""
    versions = versions or {}
    old = current()
    out = []
    for path in sorted(p for p in FILES.rglob("*") if p.suffix[1:] in EXTENSIONS):
        relative = path.relative_to(FILES).as_posix()
        folder = relative.split("/")[0]
        previous = old.get(relative, {})
        fresh = path.stat().st_mtime >= since
        producer = (versions.get((folder, path.suffix[1:])) if fresh else None) or previous.get("producer")
        if not producer:
            sys.exit(f"{relative}: no producer version, run the producer that wrote it")
        entry = {"path": relative, "producer": producer, "size": path.stat().st_size, "features": features(path)}
        for key in ("known_failure", "rewritten"):
            if key in previous:
                entry[key] = previous[key]
        out.append(entry)
    return out


def render(items: list[dict]) -> str:
    def value(v):
        if isinstance(v, list):
            return "[" + ", ".join(f'"{x}"' for x in v) + "]"
        return str(v) if isinstance(v, int) else f'"{v}"'

    blocks = [HEADER]
    for entry in items:
        blocks.append("[[file]]\n" + "".join(f"{key} = {value(v)}\n" for key, v in entry.items()))
    return "\n".join(blocks)


def budget_problems(items: list[dict]) -> list[str]:
    problems = [f"{e['path']}: {e['size']} bytes, over {MAX_FILE}" for e in items if e["size"] > MAX_FILE]
    total = sum(e["size"] for e in items)
    if total > MAX_TOTAL:
        problems.append(f"corpus is {total} bytes, over {MAX_TOTAL}")
    return problems


def update(versions: dict[tuple[str, str], str] | None = None, since: float = 0.0) -> None:
    """Rewrite manifest.toml, after a producer run (with the time it started) or by hand."""
    items = entries(versions, since)
    MANIFEST.write_text(render(items))
    total = sum(e["size"] for e in items)
    print(f"manifest: {len(items)} files, {total / 1024 / 1024:.1f} MiB")
    for problem in budget_problems(items):
        print(f"over budget: {problem}")


def main() -> None:
    if sys.argv[1:] == ["--check"]:
        items = entries()
        problems = budget_problems(items)
        if MANIFEST.read_text() != render(items):
            problems.append("manifest.toml is out of date: run python3 tests/corpus/generate/manifest.py")
        if problems:
            sys.exit("\n".join(problems))
        print(f"manifest up to date, {len(items)} files within budget")
    elif not sys.argv[1:]:
        update()
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()

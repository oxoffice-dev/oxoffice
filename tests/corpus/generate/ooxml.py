"""Raw WordprocessingML for the Word families: what python-docx 1.2.0 does not write.

Builders of XML strings (runs, fields, bookmarks, links, revisions, content
controls, form fields, equations, drawings) are parsed into the python-docx
tree with `append` or `body_append`. Other helpers add parts (notes, comment
threads, charts) or edit numbering, styles, settings and section properties in
place. Element order follows ECMA-376 as Word writes it: Word repairs, or
refuses, a file whose elements are out of order.
"""

import io
from datetime import datetime
from xml.sax.saxutils import escape

from docx.opc.constants import CONTENT_TYPE as CT
from docx.opc.constants import RELATIONSHIP_TYPE as RT
from docx.opc.packuri import PackURI
from docx.opc.part import Part, XmlPart
from docx.oxml import parse_xml

NS = {
    "w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "m": "http://schemas.openxmlformats.org/officeDocument/2006/math",
    "mc": "http://schemas.openxmlformats.org/markup-compatibility/2006",
    "w14": "http://schemas.microsoft.com/office/word/2010/wordml",
    "w15": "http://schemas.microsoft.com/office/word/2012/wordml",
    "wp": "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "wp14": "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing",
    "wps": "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "pic": "http://schemas.openxmlformats.org/drawingml/2006/picture",
    "c": "http://schemas.openxmlformats.org/drawingml/2006/chart",
    "v": "urn:schemas-microsoft-com:vml",
    "o": "urn:schemas-microsoft-com:office:office",
    "w10": "urn:schemas-microsoft-com:office:word",
}
EMU_PER_CM = 360000
HYPERLINK_RPR = '<w:rStyle w:val="Hyperlink"/>'
COMMENTS_EXTENDED_RT = "http://schemas.microsoft.com/office/2011/relationships/commentsExtended"
COMMENTS_EXTENDED_CT = "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml"


def q(name: str) -> str:
    """Clark name of a prefixed name: q("w:p") is "{...main}p"."""
    prefix, local = name.split(":")
    return f"{{{NS[prefix]}}}{local}"


def nsdecls(*prefixes: str) -> str:
    """Namespace declarations for a part root, every known prefix by default."""
    return " ".join(f'xmlns:{p}="{NS[p]}"' for p in prefixes or NS)


def esc(text: str) -> str:
    """Text escaped for an element or an attribute value."""
    return escape(text, {'"': "&quot;"})


def parse(fragment: str) -> list:
    """The elements of an XML fragment that may use any prefix of NS."""
    return list(parse_xml(f"<root {nsdecls()}>{fragment}</root>"))


def append(parent, fragment: str) -> list:
    """Append the elements of `fragment` to an lxml element or a python-docx object."""
    parent = getattr(parent, "_element", parent)
    elements = parse(fragment)
    for element in elements:
        parent.append(element)
    return elements


def body_append(doc, fragment: str) -> list:
    """Append block content (paragraphs, tables, content controls) to the end of the body."""
    sect_pr = doc.element.body.sectPr
    elements = parse(fragment)
    for element in elements:
        sect_pr.addprevious(element)
    return elements


def set_child(parent, child, order: list[str]):
    """Put `child` into `parent` at its schema position, replacing a child of the same name.
    `order` lists the prefixed names of the parent's sequence; names not listed sort last."""
    rank = {q(name): i for i, name in enumerate(order)}
    for old in parent.findall(child.tag):
        parent.remove(old)
    mine = rank.get(child.tag, len(order))
    for sibling in parent:
        if rank.get(sibling.tag, len(order)) > mine:
            sibling.addprevious(child)
            return child
    parent.append(child)
    return child


# Text and paragraphs


def run(text: str, rpr: str = "") -> str:
    """A run of text; `rpr` holds rPr children already in schema order."""
    props = f"<w:rPr>{rpr}</w:rPr>" if rpr else ""
    return f'<w:r>{props}<w:t xml:space="preserve">{esc(text)}</w:t></w:r>'


def tab(rpr: str = "") -> str:
    """A run holding one tab character."""
    props = f"<w:rPr>{rpr}</w:rPr>" if rpr else ""
    return f"<w:r>{props}<w:tab/></w:r>"


def para(content: str = "", ppr: str = "", style: str = "", para_id: str = "") -> str:
    """A paragraph; `ppr` holds pPr children after pStyle, in schema order."""
    props = (f'<w:pStyle w:val="{style}"/>' if style else "") + ppr
    attrs = f' w14:paraId="{para_id}" w14:textId="77777777"' if para_id else ""
    return f"<w:p{attrs}>" + (f"<w:pPr>{props}</w:pPr>" if props else "") + content + "</w:p>"


def para_id(n: int) -> str:
    """A w14:paraId value: eight hex digits below 0x80000000, unique per `n`."""
    return f"{0x1A000000 + n:08X}"


# Fields, bookmarks, links


def field(instr: str, result: str | None = "", rpr: str = "", ff_data: str = "", result_xml: str = "") -> str:
    """A complex field as runs: begin, instruction, separate, cached result, end.
    `result=None` writes neither separate nor result (FORMCHECKBOX, FORMDROPDOWN);
    `ff_data` is the w:ffData of a legacy form field, `result_xml` raw result runs."""
    out = field_start(instr, rpr, ff_data, separate=result is not None)
    if result is not None:
        out += result_xml or run(result, rpr)
    return out + field_end(rpr)


def field_start(instr: str, rpr: str = "", ff_data: str = "", separate: bool = True) -> str:
    """The runs that open a complex field, up to its separator: for a result that spans
    paragraphs (TOC), close it later with `field_end`."""
    props = f"<w:rPr>{rpr}</w:rPr>" if rpr else ""
    out = f'<w:r>{props}<w:fldChar w:fldCharType="begin">{ff_data}</w:fldChar></w:r>'
    out += f'<w:r>{props}<w:instrText xml:space="preserve"> {esc(instr)} </w:instrText></w:r>'
    return out + (f'<w:r>{props}<w:fldChar w:fldCharType="separate"/></w:r>' if separate else "")


def field_end(rpr: str = "") -> str:
    """The run that closes a complex field."""
    props = f"<w:rPr>{rpr}</w:rPr>" if rpr else ""
    return f'<w:r>{props}<w:fldChar w:fldCharType="end"/></w:r>'


def simple_field(instr: str, result: str, rpr: str = "") -> str:
    """A w:fldSimple with its cached result."""
    return f'<w:fldSimple w:instr=" {esc(instr)} ">{run(result, rpr)}</w:fldSimple>'


def bookmark(bid: int, name: str, inner: str = "") -> str:
    """A bookmark around `inner` (runs); an empty `inner` marks a point."""
    return f'<w:bookmarkStart w:id="{bid}" w:name="{name}"/>{inner}<w:bookmarkEnd w:id="{bid}"/>'


def hyperlink(part, url: str, text: str, rpr: str = HYPERLINK_RPR) -> str:
    """A link to an external URL, with its relationship added to `part`."""
    rid = part.relate_to(url, RT.HYPERLINK, is_external=True)
    return f'<w:hyperlink r:id="{rid}" w:history="1">{run(text, rpr)}</w:hyperlink>'


def anchor_link(anchor: str, text: str = "", rpr: str = HYPERLINK_RPR, inner: str = "") -> str:
    """A link to a bookmark of the same document."""
    return f'<w:hyperlink w:anchor="{anchor}" w:history="1">{inner or run(text, rpr)}</w:hyperlink>'


# Styles

_STYLES = {
    "Hyperlink": '<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/>'
    '<w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/>'
    '<w:rPr><w:color w:val="0563C1" w:themeColor="hyperlink"/><w:u w:val="single"/></w:rPr></w:style>',
    "FootnoteText": '<w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/>'
    '<w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/>'
    '<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr>'
    '<w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>',
    "FootnoteReference": '<w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/>'
    '<w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/>'
    '<w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>',
    "EndnoteText": '<w:style w:type="paragraph" w:styleId="EndnoteText"><w:name w:val="endnote text"/>'
    '<w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/>'
    '<w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr>'
    '<w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style>',
    "EndnoteReference": '<w:style w:type="character" w:styleId="EndnoteReference"><w:name w:val="endnote reference"/>'
    '<w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/>'
    '<w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>',
    "TOC1": '<w:style w:type="paragraph" w:styleId="TOC1"><w:name w:val="toc 1"/><w:basedOn w:val="Normal"/>'
    '<w:next w:val="Normal"/><w:autoRedefine/><w:uiPriority w:val="39"/><w:unhideWhenUsed/>'
    '<w:pPr><w:spacing w:after="100"/></w:pPr></w:style>',
    "TOC2": '<w:style w:type="paragraph" w:styleId="TOC2"><w:name w:val="toc 2"/><w:basedOn w:val="Normal"/>'
    '<w:next w:val="Normal"/><w:autoRedefine/><w:uiPriority w:val="39"/><w:unhideWhenUsed/>'
    '<w:pPr><w:spacing w:after="100"/><w:ind w:left="220"/></w:pPr></w:style>',
    "TOC3": '<w:style w:type="paragraph" w:styleId="TOC3"><w:name w:val="toc 3"/><w:basedOn w:val="Normal"/>'
    '<w:next w:val="Normal"/><w:autoRedefine/><w:uiPriority w:val="39"/><w:unhideWhenUsed/>'
    '<w:pPr><w:spacing w:after="100"/><w:ind w:left="440"/></w:pPr></w:style>',
    "TableofFigures": '<w:style w:type="paragraph" w:styleId="TableofFigures"><w:name w:val="table of figures"/>'
    '<w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/>'
    '<w:pPr><w:spacing w:after="0"/></w:pPr></w:style>',
}


def add_styles(doc, *style_ids: str) -> None:
    """Add Word's built-in definitions of these styles when the template lacks them
    (Hyperlink, FootnoteText, FootnoteReference, EndnoteText, EndnoteReference, TOC1-3, TableofFigures)."""
    styles = doc.styles.element
    present = {s.get(q("w:styleId")) for s in styles.findall(q("w:style"))}
    for style_id in style_ids:
        if style_id not in present:
            append(styles, _STYLES[style_id])


# Settings

SETTINGS_ORDER = [
    "w:writeProtection", "w:view", "w:zoom", "w:removePersonalInformation", "w:removeDateAndTime",
    "w:doNotDisplayPageBoundaries", "w:displayBackgroundShape", "w:printPostScriptOverText",
    "w:printFractionalCharacterWidth", "w:printFormsData", "w:embedTrueTypeFonts", "w:embedSystemFonts",
    "w:saveSubsetFonts", "w:saveFormsData", "w:mirrorMargins", "w:alignBordersAndEdges",
    "w:bordersDoNotSurroundHeader", "w:bordersDoNotSurroundFooter", "w:gutterAtTop", "w:hideSpellingErrors",
    "w:hideGrammaticalErrors", "w:activeWritingStyle", "w:proofState", "w:formsDesign", "w:attachedTemplate",
    "w:linkStyles", "w:stylePaneFormatFilter", "w:stylePaneSortMethod", "w:documentType", "w:mailMerge",
    "w:revisionView", "w:trackRevisions", "w:doNotTrackMoves", "w:doNotTrackFormatting", "w:documentProtection",
    "w:autoFormatOverride", "w:styleLockTheme", "w:styleLockQFSet", "w:defaultTabStop", "w:autoHyphenation",
    "w:consecutiveHyphenLimit", "w:hyphenationZone", "w:doNotHyphenateCaps", "w:showEnvelope", "w:summaryLength",
    "w:clickAndTypeStyle", "w:defaultTableStyle", "w:evenAndOddHeaders", "w:bookFoldRevPrinting",
    "w:bookFoldPrinting", "w:bookFoldPrintingSheets", "w:drawingGridHorizontalSpacing",
    "w:drawingGridVerticalSpacing", "w:displayHorizontalDrawingGridEvery", "w:displayVerticalDrawingGridEvery",
    "w:doNotUseMarginsForDrawingGridOrigin", "w:drawingGridHorizontalOrigin", "w:drawingGridVerticalOrigin",
    "w:doNotShadeFormData", "w:noPunctuationKerning", "w:characterSpacingControl", "w:printTwoOnOne",
    "w:strictFirstAndLastChars", "w:noLineBreaksAfter", "w:noLineBreaksBefore", "w:savePreviewPicture",
    "w:doNotValidateAgainstSchema", "w:saveInvalidXml", "w:ignoreMixedContent", "w:alwaysShowPlaceholderText",
    "w:doNotDemarcateInvalidXml", "w:saveXmlDataOnly", "w:useXSLTWhenSaving", "w:saveThroughXslt",
    "w:showXMLTags", "w:alwaysMergeEmptyNamespace", "w:updateFields", "w:hdrShapeDefaults", "w:footnotePr",
    "w:endnotePr", "w:compat", "w:docVars", "w:rsids", "m:mathPr", "w:attachedSchema", "w:themeFontLang",
    "w:clrSchemeMapping", "w:doNotIncludeSubdocsInStats", "w:doNotAutoCompressPictures", "w:forceUpgrade",
    "w:captions", "w:readModeInkLockDown", "w:smartTagType", "w:schemaLibrary", "w:shapeDefaults",
    "w:doNotEmbedSmartTags", "w:decimalSymbol", "w:listSeparator",
]


def set_setting(doc, fragment: str):
    """Put one settings element (raw XML) at its schema position, replacing an older one."""
    return set_child(doc.settings.element, parse(fragment)[0], SETTINGS_ORDER)


def protect_forms(doc) -> None:
    """Restrict editing to form fields and content controls, enforced, without a password."""
    set_setting(doc, '<w:documentProtection w:edit="forms" w:enforcement="1"/>')


# Notes

_NOTES = {
    "footnote": (RT.FOOTNOTES, CT.WML_FOOTNOTES, "/word/footnotes.xml"),
    "endnote": (RT.ENDNOTES, CT.WML_ENDNOTES, "/word/endnotes.xml"),
}


def _related(part, reltype):
    for rel in part.rels.values():
        if rel.reltype == reltype and not rel.is_external:
            return rel.target_part
    return None


def _notes_root(doc, kind: str):
    """The w:footnotes or w:endnotes root, created on first use with its two separator
    notes, the settings entries that name them, and the note styles."""
    reltype, content_type, partname = _NOTES[kind]
    part = _related(doc.part, reltype)
    if part is not None:
        return part.element
    separators = "".join(
        f'<w:{kind} w:type="{kind_type}" w:id="{nid}"><w:p><w:pPr><w:spacing w:after="0" w:line="240" '
        f'w:lineRule="auto"/></w:pPr><w:r><w:{mark}/></w:r></w:p></w:{kind}>'
        for kind_type, nid, mark in (("separator", -1, "separator"), ("continuationSeparator", 0, "continuationSeparator"))
    )
    root = parse_xml(f'<w:{kind}s {nsdecls("w", "r", "w14", "mc")} mc:Ignorable="w14">{separators}</w:{kind}s>')
    part = XmlPart(PackURI(partname), content_type, root, doc.part.package)
    doc.part.relate_to(part, reltype)
    set_setting(doc, f'<w:{kind}Pr><w:{kind} w:id="-1"/><w:{kind} w:id="0"/></w:{kind}Pr>')
    style = kind.capitalize()
    add_styles(doc, f"{style}Text", f"{style}Reference")
    return root


def add_note(doc, paragraph, kind: str, text: str) -> int:
    """Add a footnote or endnote with `text` and its reference mark at the end of `paragraph`
    (a python-docx paragraph or a w:p); return the note id."""
    root = _notes_root(doc, kind)
    nid = max(int(n.get(q("w:id"))) for n in root.findall(q(f"w:{kind}"))) + 1
    style = kind.capitalize()
    append(
        root,
        f'<w:{kind} w:id="{nid}">'
        + para(f'<w:r><w:rPr><w:rStyle w:val="{style}Reference"/></w:rPr><w:{kind}Ref/></w:r>' + run(" " + text),
               style=f"{style}Text")
        + f"</w:{kind}>",
    )
    target = getattr(paragraph, "_p", paragraph)
    append(target, f'<w:r><w:rPr><w:rStyle w:val="{style}Reference"/></w:rPr><w:{kind}Reference w:id="{nid}"/></w:r>')
    return nid


# Revisions


def inserted(rev_id: int, author: str, date: str, content: str) -> str:
    """Runs marked as inserted by `author`."""
    return f'<w:ins w:id="{rev_id}" w:author="{esc(author)}" w:date="{date}">{content}</w:ins>'


def deleted(rev_id: int, author: str, date: str, text: str, rpr: str = "") -> str:
    """Text marked as deleted by `author` (w:delText inside w:del)."""
    props = f"<w:rPr>{rpr}</w:rPr>" if rpr else ""
    return (
        f'<w:del w:id="{rev_id}" w:author="{esc(author)}" w:date="{date}">'
        f'<w:r>{props}<w:delText xml:space="preserve">{esc(text)}</w:delText></w:r></w:del>'
    )


def format_change(rev_id: int, author: str, date: str, old_rpr: str = "") -> str:
    """A w:rPrChange holding the run properties before the change; it closes the new rPr."""
    return f'<w:rPrChange w:id="{rev_id}" w:author="{esc(author)}" w:date="{date}"><w:rPr>{old_rpr}</w:rPr></w:rPrChange>'


def inserted_mark(rev_id: int, author: str, date: str) -> str:
    """pPr content marking the paragraph mark as inserted (a whole new paragraph)."""
    return f'<w:rPr><w:ins w:id="{rev_id}" w:author="{esc(author)}" w:date="{date}"/></w:rPr>'


# Comment threads


def link_comments(doc, parents: dict[int, int], done: frozenset = frozenset()) -> None:
    """Write commentsExtended: give the last paragraph of every comment a w14:paraId, link each
    reply (comment id -> parent comment id) through w15:paraIdParent, mark `done` threads resolved."""
    comments = doc.part._comments_part.element
    comments.set(q("mc:Ignorable"), "w14")
    para_ids = {}
    for n, comment in enumerate(comments.findall(q("w:comment"))):
        last = comment.findall(q("w:p"))[-1]
        last.set(q("w14:paraId"), para_id(0x400 + n))
        last.set(q("w14:textId"), "77777777")
        para_ids[int(comment.get(q("w:id")))] = para_id(0x400 + n)
    entries = []
    for cid, pid in para_ids.items():
        parent = f' w15:paraIdParent="{para_ids[parents[cid]]}"' if cid in parents else ""
        state = "1" if cid in done or parents.get(cid) in done else "0"
        entries.append(f'<w15:commentEx w15:paraId="{pid}"{parent} w15:done="{state}"/>')
    _ends_parent_first(doc, parents)
    root = parse_xml(f'<w15:commentsEx {nsdecls("w15", "mc")} mc:Ignorable="w15">{"".join(entries)}</w15:commentsEx>')
    part = XmlPart(PackURI("/word/commentsExtended.xml"), COMMENTS_EXTENDED_CT, root, doc.part.package)
    doc.part.relate_to(part, COMMENTS_EXTENDED_RT)


def _ends_parent_first(doc, parents: dict[int, int]) -> None:
    """Write the range end and reference of a thread's parent before its replies', as Word does. python-docx
    nests a reply's range inside its parent's, which puts the reply's end first; Word for Mac then saves the
    replies as top-level comments."""
    reply_ids = {str(cid) for cid in parents}
    body = doc.element.body
    for end in list(body.iter(q("w:commentRangeEnd"))):
        cid = end.get(q("w:id"))
        if cid not in reply_ids:
            continue
        parent = str(parents[int(cid)])
        reference = end.getnext()
        parent_end = next(e for e in body.iter(q("w:commentRangeEnd")) if e.get(q("w:id")) == parent)
        parent_reference = parent_end.getnext()
        # A reply anchored on its parent's text: both ends in one paragraph, each followed by its reference run.
        for mark, ref, ident in ((end, reference, cid), (parent_end, parent_reference, parent)):
            assert ref is not None and ref.find(f'{q("w:commentReference")}[@{q("w:id")}="{ident}"]') is not None, ident
        assert end.getparent() is parent_end.getparent(), f"reply {cid} ends in its parent's paragraph"
        # The reply's end and reference go right after its parent's reference (and after the replies placed
        # there before it), so the order of a thread is parent, then replies in id order.
        anchor = parent_reference
        while (anchor.getnext() is not None and anchor.getnext().tag == q("w:commentRangeEnd")
               and anchor.getnext().get(q("w:id")) in reply_ids and int(anchor.getnext().get(q("w:id"))) < int(cid)):
            anchor = anchor.getnext().getnext()
        anchor.addnext(end)
        end.addnext(reference)


# Content controls


def sdt(content: str, sid: int, alias: str = "", kind: str = "", rpr: str = "", lock: str = "") -> str:
    """A content control around `content` (runs, paragraphs or a cell, as its parent allows).
    `kind` is the type element (w:text, w:date, w:dropDownList, w:comboBox, w14:checkbox, a
    w:docPartObj...); none makes a rich text control, as Word writes it."""
    props = f"<w:rPr>{rpr}</w:rPr>" if rpr else ""
    if alias:
        props += f'<w:alias w:val="{esc(alias)}"/><w:tag w:val="{esc(alias.lower().replace(" ", "_"))}"/>'
    props += f'<w:id w:val="{sid}"/>'
    if lock:
        props += f'<w:lock w:val="{lock}"/>'
    return f"<w:sdt><w:sdtPr>{props}{kind}</w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"


def sdt_text(sid: int, alias: str, value: str, rpr: str = "") -> str:
    """An inline plain text control holding `value`."""
    return sdt(run(value, rpr), sid, alias, "<w:text/>")


def sdt_date(sid: int, alias: str, iso_date: str, shown: str, fmt: str = "d MMMM yyyy", lang: str = "en-GB") -> str:
    """An inline date picker set to `iso_date` (YYYY-MM-DD), showing `shown`."""
    kind = (
        f'<w:date w:fullDate="{iso_date}T00:00:00Z"><w:dateFormat w:val="{fmt}"/><w:lid w:val="{lang}"/>'
        '<w:storeMappedDataAs w:val="dateTime"/><w:calendar w:val="gregorian"/></w:date>'
    )
    return sdt(run(shown), sid, alias, kind)


def sdt_list(sid: int, alias: str, items: list[str], selected: int, combo: bool = False) -> str:
    """An inline drop-down list (or combo box) showing `items[selected]`."""
    tag = "w:comboBox" if combo else "w:dropDownList"
    entries = "".join(f'<w:listItem w:displayText="{esc(i)}" w:value="{esc(i)}"/>' for i in items)
    kind = f'<{tag} w:lastValue="{esc(items[selected])}">{entries}</{tag}>'
    return sdt(run(items[selected]), sid, alias, kind)


def sdt_checkbox(sid: int, alias: str, checked: bool) -> str:
    """An inline Word 2010 check box (w14:checkbox) drawn with MS Gothic ballot boxes."""
    kind = (
        f'<w14:checkbox><w14:checked w14:val="{int(checked)}"/>'
        '<w14:checkedState w14:val="2612" w14:font="MS Gothic"/>'
        '<w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/></w14:checkbox>'
    )
    box = run("☒" if checked else "☐", '<w:rFonts w:ascii="MS Gothic" w:eastAsia="MS Gothic" w:hAnsi="MS Gothic" w:hint="eastAsia"/>')
    return sdt(box, sid, alias, kind)


# Legacy form fields


def _ff_data(name: str, body: str) -> str:
    return f'<w:ffData><w:name w:val="{name}"/><w:enabled/><w:calcOnExit w:val="0"/>{body}</w:ffData>'


def form_text(bid: int, name: str, value: str, max_length: int = 0) -> str:
    """A legacy text form field (FORMTEXT) holding `value`, inside its bookmark."""
    limit = f'<w:maxLength w:val="{max_length}"/>' if max_length else ""
    data = _ff_data(name, f"<w:textInput>{limit}</w:textInput>")
    return bookmark(bid, name, field("FORMTEXT", value, ff_data=data, result_xml=run(value, "<w:noProof/>")))


def form_checkbox(bid: int, name: str, checked: bool) -> str:
    """A legacy check box form field (FORMCHECKBOX), inside its bookmark."""
    state = '<w:checked/>' if checked else ""
    data = _ff_data(name, f'<w:checkBox><w:sizeAuto/><w:default w:val="0"/>{state}</w:checkBox>')
    return bookmark(bid, name, field("FORMCHECKBOX", None, ff_data=data))


def form_dropdown(bid: int, name: str, entries: list[str], selected: int = 0) -> str:
    """A legacy drop-down form field (FORMDROPDOWN) showing `entries[selected]`, inside its bookmark."""
    result = f'<w:result w:val="{selected}"/>' if selected else ""
    items = "".join(f'<w:listEntry w:val="{esc(e)}"/>' for e in entries)
    data = _ff_data(name, f"<w:ddList>{result}{items}</w:ddList>")
    return bookmark(bid, name, field("FORMDROPDOWN", None, ff_data=data))


# Equations (OMML)

_MATH_RPR = '<w:rPr><w:rFonts w:ascii="Cambria Math" w:hAnsi="Cambria Math"/></w:rPr>'


def m_r(text: str, plain: bool = False) -> str:
    """A math run; `plain` sets upright style (function names, units)."""
    style = '<m:rPr><m:sty m:val="p"/></m:rPr>' if plain else ""
    return f"<m:r>{style}{_MATH_RPR}<m:t>{esc(text)}</m:t></m:r>"


def m_frac(num: str, den: str) -> str:
    """A fraction."""
    return f"<m:f><m:num>{num}</m:num><m:den>{den}</m:den></m:f>"


def m_sup(base: str, sup: str) -> str:
    """A superscript."""
    return f"<m:sSup><m:e>{base}</m:e><m:sup>{sup}</m:sup></m:sSup>"


def m_sub(base: str, sub: str) -> str:
    """A subscript."""
    return f"<m:sSub><m:e>{base}</m:e><m:sub>{sub}</m:sub></m:sSub>"


def m_subsup(base: str, sub: str, sup: str) -> str:
    """A subscript and a superscript on the same base."""
    return f"<m:sSubSup><m:e>{base}</m:e><m:sub>{sub}</m:sub><m:sup>{sup}</m:sup></m:sSubSup>"


def m_nary(char: str, sub: str, sup: str, body: str) -> str:
    """An n-ary operator (sum, product, integral) with limits under and over it."""
    return (
        f'<m:nary><m:naryPr><m:chr m:val="{char}"/><m:limLoc m:val="undOvr"/></m:naryPr>'
        f"<m:sub>{sub}</m:sub><m:sup>{sup}</m:sup><m:e>{body}</m:e></m:nary>"
    )


def m_rad(body: str, degree: str = "") -> str:
    """A square root, or a root of `degree`."""
    hide = "" if degree else '<m:radPr><m:degHide m:val="1"/></m:radPr>'
    return f"<m:rad>{hide}<m:deg>{degree}</m:deg><m:e>{body}</m:e></m:rad>"


def m_delim(body: str, begin: str = "(", end: str = ")") -> str:
    """Delimiters that grow with their content."""
    return f'<m:d><m:dPr><m:begChr m:val="{begin}"/><m:endChr m:val="{end}"/></m:dPr><m:e>{body}</m:e></m:d>'


def m_matrix(rows: list[list[str]], begin: str = "[", end: str = "]") -> str:
    """A matrix in brackets; each cell is OMML."""
    count = len(rows[0])
    props = (
        f'<m:mPr><m:mcs><m:mc><m:mcPr><m:count m:val="{count}"/><m:mcJc m:val="center"/></m:mcPr></m:mc>'
        "</m:mcs></m:mPr>"
    )
    body = "".join("<m:mr>" + "".join(f"<m:e>{cell}</m:e>" for cell in row) + "</m:mr>" for row in rows)
    return m_delim(f"<m:m>{props}{body}</m:m>", begin, end)


def m_acc(base: str, char: str = "\u0302") -> str:
    """An accent over its base (a hat by default)."""
    return f'<m:acc><m:accPr><m:chr m:val="{char}"/></m:accPr><m:e>{base}</m:e></m:acc>'


def m_bar(base: str) -> str:
    """A bar over its base (a mean)."""
    return f'<m:bar><m:barPr><m:pos m:val="top"/></m:barPr><m:e>{base}</m:e></m:bar>'


def m_func(name: str, arg: str) -> str:
    """A function application such as log or sin."""
    return f"<m:func><m:fName>{m_r(name, plain=True)}</m:fName><m:e>{arg}</m:e></m:func>"


def omath(*parts: str) -> str:
    """An inline equation, to place among the runs of a paragraph."""
    return f"<m:oMath>{''.join(parts)}</m:oMath>"


def omath_para(*parts: str) -> str:
    """A display equation, alone in its paragraph."""
    return f'<m:oMathPara><m:oMathParaPr><m:jc m:val="center"/></m:oMathParaPr>{omath(*parts)}</m:oMathPara>'


# Drawings: floating pictures, text boxes, shapes


def _position(axis: str, frame: str, where) -> str:
    inner = f"<wp:align>{where}</wp:align>" if isinstance(where, str) else f"<wp:posOffset>{int(where)}</wp:posOffset>"
    return f'<wp:position{axis} relativeFrom="{frame}">{inner}</wp:position{axis}>'


def _wrap(kind: str) -> str:
    if kind == "square":
        return '<wp:wrapSquare wrapText="bothSides"/>'
    if kind == "tight":
        return (
            '<wp:wrapTight wrapText="bothSides"><wp:wrapPolygon edited="0"><wp:start x="0" y="0"/>'
            '<wp:lineTo x="0" y="21600"/><wp:lineTo x="21600" y="21600"/><wp:lineTo x="21600" y="0"/>'
            '<wp:lineTo x="0" y="0"/></wp:wrapPolygon></wp:wrapTight>'
        )
    if kind == "topAndBottom":
        return "<wp:wrapTopAndBottom/>"
    return "<wp:wrapNone/>"


def anchor(graphic: str, docpr_id: int, name: str, cx: int, cy: int, wrap: str = "square",
           h: tuple = ("column", 0), v: tuple = ("paragraph", 0), behind: bool = False, locks: str = "") -> str:
    """A floating drawing (wp:anchor) around an a:graphic. `h` and `v` are (relativeFrom,
    offset in EMU or an alignment such as "right"); `wrap` is square, tight, topAndBottom or none."""
    dist = "0" if wrap == "none" else "114300"
    return (
        f'<wp:anchor distT="0" distB="0" distL="{dist}" distR="{dist}" simplePos="0" '
        f'relativeHeight="{251658240 + docpr_id}" behindDoc="{int(behind)}" locked="0" layoutInCell="1" '
        f'allowOverlap="1"><wp:simplePos x="0" y="0"/>{_position("H", *h)}{_position("V", *v)}'
        f'<wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>{_wrap(wrap)}'
        f'<wp:docPr id="{docpr_id}" name="{esc(name)}"/><wp:cNvGraphicFramePr>{locks}</wp:cNvGraphicFramePr>'
        f"{graphic}</wp:anchor>"
    )


def picture_graphic(rid: str, name: str, cx: int, cy: int) -> str:
    """An a:graphic holding a picture whose image part is `rid`."""
    return (
        '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
        f'<pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="{esc(name)}"/><pic:cNvPicPr/></pic:nvPicPr>'
        f'<pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
        f'<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>'
    )


def floating_picture(part, image, docpr_id: int, cx: int, wrap: str = "square",
                     h: tuple = ("column", "right"), v: tuple = ("paragraph", 0)) -> str:
    """A run holding a floating picture `cx` EMU wide (height from the image ratio)."""
    rid, img = part.get_or_add_image(image)
    cy = int(cx * img.px_height / img.px_width)
    name = f"Picture {docpr_id}"
    locks = '<a:graphicFrameLocks noChangeAspect="1"/>'
    graphic = picture_graphic(rid, name, cx, cy)
    return f"<w:r><w:drawing>{anchor(graphic, docpr_id, name, cx, cy, wrap, h, v, locks=locks)}</w:drawing></w:r>"


def _vml_wrap(kind: str) -> str:
    return kind if kind in ("square", "tight", "topAndBottom") else "none"


def _pt(emu: int) -> str:
    return f"{emu / 12700:g}pt"


def _vml_style(cx: int, cy: int, h: tuple, v: tuple, z: int) -> str:
    left = h[1] if isinstance(h[1], int) else 0
    top = v[1] if isinstance(v[1], int) else 0
    style = f"position:absolute;margin-left:{_pt(left)};margin-top:{_pt(top)};width:{_pt(cx)};height:{_pt(cy)};z-index:{z};"
    if isinstance(h[1], str):
        style += f"mso-position-horizontal:{h[1]};"
    style += f"mso-position-horizontal-relative:{'text' if h[0] == 'column' else h[0]};"
    return style + f"mso-position-vertical-relative:{'text' if v[0] == 'paragraph' else v[0]}"


def text_box(docpr_id: int, cx: int, cy: int, content: str, h: tuple = ("column", "right"),
             v: tuple = ("paragraph", 0), wrap: str = "square", fill: str = "F1F5F9", line: str = "0F766E") -> str:
    """A run holding a floating text box as Word writes it: a wps shape in mc:AlternateContent
    with a VML fallback; `content` is the paragraphs, written in both branches."""
    name = f"Text Box {docpr_id}"
    wsp = (
        '<wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr>'
        f'<a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>'
        f'<a:solidFill><a:srgbClr val="{fill}"/></a:solidFill>'
        f'<a:ln w="12700"><a:solidFill><a:srgbClr val="{line}"/></a:solidFill></a:ln></wps:spPr>'
        f"<wps:txbx><w:txbxContent>{content}</w:txbxContent></wps:txbx>"
        '<wps:bodyPr rot="0" vert="horz" wrap="square" lIns="91440" tIns="45720" rIns="91440" bIns="45720" '
        'anchor="t" anchorCtr="0"><a:noAutofit/></wps:bodyPr></wps:wsp>'
    )
    graphic = f'<a:graphic><a:graphicData uri="{NS["wps"]}">{wsp}</a:graphicData></a:graphic>'
    choice = f"<w:drawing>{anchor(graphic, docpr_id, name, cx, cy, wrap, h, v)}</w:drawing>"
    fallback = (
        '<w:pict><v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe">'
        '<v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype>'
        f'<v:shape id="{name}" o:spid="_x0000_s{1024 + docpr_id}" type="#_x0000_t202" '
        f'style="{_vml_style(cx, cy, h, v, docpr_id)}" fillcolor="#{fill}" strokecolor="#{line}" strokeweight="1pt">'
        f'<v:textbox><w:txbxContent>{content}</w:txbxContent></v:textbox>'
        f'<w10:wrap type="{_vml_wrap(wrap)}"/></v:shape></w:pict>'
    )
    return (
        f'<w:r><mc:AlternateContent><mc:Choice Requires="wps">{choice}</mc:Choice>'
        f"<mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent></w:r>"
    )


_VML_SHAPES = {"rect": "v:rect", "roundRect": "v:roundrect", "ellipse": "v:oval"}


def shape(docpr_id: int, prst: str, cx: int, cy: int, fill: str, h: tuple, v: tuple,
          wrap: str = "none", behind: bool = False, text: str = "") -> str:
    """A run holding a floating DrawingML shape (rect, roundRect or ellipse) with optional
    centred white text, in mc:AlternateContent with its VML fallback."""
    name = f"Shape {docpr_id}"
    body = para(run(text, '<w:b/><w:color w:val="FFFFFF"/>'), '<w:jc w:val="center"/>') if text else ""
    txbx = f"<wps:txbx><w:txbxContent>{body}</w:txbxContent></wps:txbx>" if text else ""
    wsp = (
        f'<wps:wsp><wps:cNvSpPr/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        f'<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill>'
        f'<a:ln><a:noFill/></a:ln></wps:spPr>{txbx}'
        '<wps:bodyPr rot="0" vert="horz" wrap="square" lIns="91440" tIns="45720" rIns="91440" bIns="45720" '
        'anchor="ctr" anchorCtr="0"><a:noAutofit/></wps:bodyPr></wps:wsp>'
    )
    graphic = f'<a:graphic><a:graphicData uri="{NS["wps"]}">{wsp}</a:graphicData></a:graphic>'
    choice = f"<w:drawing>{anchor(graphic, docpr_id, name, cx, cy, wrap, h, v, behind)}</w:drawing>"
    tag = _VML_SHAPES[prst]
    arc = ' arcsize="10923f"' if prst == "roundRect" else ""
    inner = f"<v:textbox><w:txbxContent>{body}</w:txbxContent></v:textbox>" if text else ""
    fallback = (
        f'<w:pict><{tag} id="{name}" o:spid="_x0000_s{1024 + docpr_id}" style="{_vml_style(cx, cy, h, v, docpr_id)}"'
        f'{arc} fillcolor="#{fill}" stroked="f">{inner}<w10:wrap type="{_vml_wrap(wrap)}"/>'
        f"</{tag}></w:pict>"
    )
    return (
        f'<w:r><mc:AlternateContent><mc:Choice Requires="wps">{choice}</mc:Choice>'
        f"<mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent></w:r>"
    )


def renumber_drawings(doc) -> None:
    """Give every wp:docPr of the document and its headers and footers a unique id, in order."""
    parts = [doc.part] + [rel.target_part for rel in doc.part.rels.values()
                          if not rel.is_external and rel.reltype in (RT.HEADER, RT.FOOTER)]
    n = 0
    for part in parts:
        for doc_pr in part.element.iter(q("wp:docPr")):
            n += 1
            doc_pr.set("id", str(n))


# Charts


def _xlsx_blob(chart_data) -> bytes:
    """The chart's embedded workbook, as python-pptx writes it but with a fixed creation date."""
    from xlsxwriter import Workbook

    stream = io.BytesIO()
    workbook = Workbook(stream, {"in_memory": True})
    workbook.set_properties({"created": datetime(2026, 11, 21, 9, 0, 0)})
    worksheet = workbook.add_worksheet()
    chart_data._workbook_writer._populate_worksheet(workbook, worksheet)
    workbook.close()
    return stream.getvalue()


def add_chart(doc, chart_type, chart_data, title: str, docpr_id: int, cx: int, cy: int) -> str:
    """Add a chart part (python-pptx's chart XML) with its embedded workbook; return the run
    holding the inline graphic frame that shows it."""
    from pptx.chart.xmlwriter import ChartXmlWriter

    package = doc.part.package
    n = 1 + sum(1 for p in package.iter_parts() if str(p.partname).startswith("/word/charts/"))
    space = parse_xml(ChartXmlWriter(chart_type, chart_data).xml.encode())
    chart = space.find(q("c:chart"))
    set_child(space, parse('<c:roundedCorners val="0"/>')[0], ["c:date1904", "c:lang", "c:roundedCorners", "c:chart"])
    chart.insert(0, parse(
        f'<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1200" b="1"/></a:pPr>'
        f'<a:r><a:rPr lang="en-GB" sz="1200" b="1"/><a:t>{esc(title)}</a:t></a:r></a:p></c:rich></c:tx>'
        '<c:overlay val="0"/></c:title>')[0])
    # One legend, at the bottom: python-pptx's XML may already hold one.
    for legend in chart.findall(q("c:legend")):
        chart.remove(legend)
    chart.find(q("c:plotArea")).addnext(parse('<c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend>')[0])
    for size in space.iter(q("a:defRPr")):
        if size.get("sz") == "1800":
            size.set("sz", "1000")
    workbook = Part(
        PackURI(f"/word/embeddings/Microsoft_Excel_Worksheet{n}.xlsx"), CT.SML_SHEET, _xlsx_blob(chart_data), package
    )
    part = XmlPart(PackURI(f"/word/charts/chart{n}.xml"), CT.DML_CHART, space, package)
    wb_rid = part.relate_to(workbook, RT.PACKAGE)
    space.append(parse(f'<c:externalData r:id="{wb_rid}"><c:autoUpdate val="0"/></c:externalData>')[0])
    rid = doc.part.relate_to(part, RT.CHART)
    return (
        f'<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/>'
        f'<wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{docpr_id}" name="Chart {docpr_id}"/>'
        f'<wp:cNvGraphicFramePr/><a:graphic><a:graphicData uri="{NS["c"]}"><c:chart r:id="{rid}"/>'
        "</a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
    )


# Numbering


def add_numbering(doc, levels: list[tuple], nsid: str, styles: tuple = ()) -> int:
    """Add an abstract numbering definition and a w:num using it; return the numId.
    `levels` holds (numFmt, lvlText, left indent, hanging indent) per level, in twips;
    `styles` optionally links levels to paragraph style ids (w:pStyle)."""
    numbering = doc.part.numbering_part.element
    abstract_id = 1 + max((int(a.get(q("w:abstractNumId"))) for a in numbering.findall(q("w:abstractNum"))), default=-1)
    lvls = ""
    for ilvl, (fmt, text, left, hanging) in enumerate(levels):
        style = f'<w:pStyle w:val="{styles[ilvl]}"/>' if ilvl < len(styles) else ""
        lvls += (
            f'<w:lvl w:ilvl="{ilvl}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/>{style}'
            f'<w:lvlText w:val="{esc(text)}"/><w:lvlJc w:val="left"/>'
            f'<w:pPr><w:ind w:left="{left}" w:hanging="{hanging}"/></w:pPr></w:lvl>'
        )
    abstract = parse(
        f'<w:abstractNum w:abstractNumId="{abstract_id}"><w:nsid w:val="{nsid}"/>'
        f'<w:multiLevelType w:val="{"multilevel" if len(levels) > 1 else "singleLevel"}"/>{lvls}</w:abstractNum>'
    )[0]
    first_num = numbering.find(q("w:num"))
    if first_num is not None:
        first_num.addprevious(abstract)
    else:
        numbering.append(abstract)
    return _add_num(numbering, abstract_id)


def _add_num(numbering, abstract_id: int, override: str = "") -> int:
    num_id = 1 + max((int(n.get(q("w:numId"))) for n in numbering.findall(q("w:num"))), default=0)
    num = parse(f'<w:num w:numId="{num_id}"><w:abstractNumId w:val="{abstract_id}"/>{override}</w:num>')[0]
    last = numbering.findall(q("w:num"))
    (last[-1].addnext(num) if last else numbering.append(num))
    return num_id


def restart_numbering(doc, num_id: int) -> int:
    """A new w:num on the same abstract numbering as `num_id`, restarting level 0 at 1."""
    numbering = doc.part.numbering_part.element
    num = next(n for n in numbering.findall(q("w:num")) if n.get(q("w:numId")) == str(num_id))
    abstract_id = int(num.find(q("w:abstractNumId")).get(q("w:val")))
    return _add_num(numbering, abstract_id, '<w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride>')


def numbered(num_id: int, level: int = 0) -> str:
    """pPr content (w:numPr) putting a paragraph in list `num_id` at `level`."""
    return f'<w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{num_id}"/></w:numPr>'


def style_numbering(doc, style_id: str, num_id: int, level: int) -> None:
    """Link a paragraph style to a list level, as Word does for numbered headings."""
    style = doc.styles.element.get_by_id(style_id)
    style.get_or_add_pPr().get_or_add_numPr()
    num_pr = style.pPr.numPr
    for child in list(num_pr):
        num_pr.remove(child)
    append(num_pr, f'<w:ilvl w:val="{level}"/><w:numId w:val="{num_id}"/>')


# Section properties

SECTPR_ORDER = [
    "w:headerReference", "w:footerReference", "w:footnotePr", "w:endnotePr", "w:type", "w:pgSz", "w:pgMar",
    "w:paperSrc", "w:pgBorders", "w:lnNumType", "w:pgNumType", "w:cols", "w:formProt", "w:vAlign",
    "w:noEndnote", "w:titlePg", "w:textDirection", "w:bidi", "w:rtlGutter", "w:docGrid", "w:printerSettings",
    "w:sectPrChange",
]


def page_numbers(section, fmt: str = "", start: int | None = None) -> None:
    """Set the page number format (lowerRoman, decimal...) and restart value of a section."""
    attrs = (f' w:fmt="{fmt}"' if fmt else "") + (f' w:start="{start}"' if start is not None else "")
    set_child(section._sectPr, parse(f"<w:pgNumType{attrs}/>")[0], SECTPR_ORDER)


def columns(section, num: int, space: int = 567, separator: bool = False) -> None:
    """Lay a section out in `num` equal columns `space` twips apart."""
    sep = ' w:sep="1"' if separator else ""
    set_child(section._sectPr, parse(f'<w:cols w:num="{num}" w:space="{space}"{sep}/>')[0], SECTPR_ORDER)


# Tables

TBLPR_ORDER = [
    "w:tblStyle", "w:tblpPr", "w:tblOverlap", "w:bidiVisual", "w:tblStyleRowBandSize", "w:tblStyleColBandSize",
    "w:tblW", "w:jc", "w:tblCellSpacing", "w:tblInd", "w:tblBorders", "w:shd", "w:tblLayout", "w:tblCellMar",
    "w:tblLook", "w:tblCaption", "w:tblDescription",
]
TCPR_ORDER = [
    "w:cnfStyle", "w:tcW", "w:gridSpan", "w:hMerge", "w:vMerge", "w:tcBorders", "w:shd", "w:noWrap", "w:tcMar",
    "w:textDirection", "w:tcFitText", "w:vAlign", "w:hideMark",
]
TRPR_ORDER = [
    "w:cnfStyle", "w:divId", "w:gridBefore", "w:gridAfter", "w:wBefore", "w:wAfter", "w:cantSplit", "w:trHeight",
    "w:tblHeader", "w:tblCellSpacing", "w:jc", "w:hidden",
]


def table_borders(table, val: str = "none", size: int = 0, color: str = "auto") -> None:
    """Set every border of a table (outer and inside) to `val` (none for a layout table, single...)."""
    sides = "".join(
        f'<w:{side} w:val="{val}" w:sz="{size}" w:space="0" w:color="{color}"/>'
        for side in ("top", "left", "bottom", "right", "insideH", "insideV")
    )
    set_child(table._tbl.tblPr, parse(f"<w:tblBorders>{sides}</w:tblBorders>")[0], TBLPR_ORDER)


def cell_shading(cell, fill: str) -> None:
    """A solid background colour (hex RGB) for a table cell."""
    set_child(cell._tc.get_or_add_tcPr(), parse(f'<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/>')[0], TCPR_ORDER)


def repeat_header(row) -> None:
    """Repeat a table row at the top of every page the table spans (w:tblHeader)."""
    set_child(row._tr.get_or_add_trPr(), parse("<w:tblHeader/>")[0], TRPR_ORDER)

"""The PowerPoint families of the corpus, written with python-pptx.

Each builder writes one source deck; see common.py for the registry. What
python-pptx cannot write (transitions, sections, legacy comments, hidden
slides, footer placeholders, right-to-left and East Asian run properties) is
added as raw PresentationML by the helpers below, in the order and form
PowerPoint itself writes.
"""

import io
import random
import re
import uuid
import zipfile
from datetime import datetime
from pathlib import Path

from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.dml.color import RGBColor
from pptx.enum.chart import XL_CHART_TYPE, XL_LABEL_POSITION, XL_LEGEND_POSITION
from pptx.enum.shapes import MSO_CONNECTOR, MSO_SHAPE, PP_PLACEHOLDER
from pptx.enum.text import PP_ALIGN
from pptx.opc.constants import CONTENT_TYPE as CT
from pptx.opc.constants import RELATIONSHIP_TYPE as RT
from pptx.opc.package import Part
from pptx.opc.packuri import PackURI
from pptx.oxml import parse_xml
from pptx.oxml.ns import qn
from pptx.util import Emu, Inches
from pptx.util import Pt as PPt

from common import DATE, LATER, chart_png, family, logo_png, sentences

NS_A = "http://schemas.openxmlformats.org/drawingml/2006/main"
NS_P = "http://schemas.openxmlformats.org/presentationml/2006/main"
NS_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
NS_P14 = "http://schemas.microsoft.com/office/powerpoint/2010/main"
NS_P15 = "http://schemas.microsoft.com/office/powerpoint/2012/main"
DECLS = f'xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"'

# PowerPoint's 16:9 slide, exactly: Inches(13.333) is 305 EMU short of it.
WIDE, HIGH = 12192000, 6858000

INK = RGBColor(0x1F, 0x29, 0x37)
TEAL = RGBColor(0x0F, 0x76, 0x6E)
INDIGO = RGBColor(0x4F, 0x46, 0xE5)
AMBER = RGBColor(0xD9, 0x77, 0x06)
RED = RGBColor(0xB9, 0x1C, 0x1C)
GREY = RGBColor(0x6B, 0x72, 0x80)


def _when(stamp: str) -> datetime:
    return datetime.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ")


def _xml_text(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


def _guid(*names: str) -> str:
    """A stable {GUID} for ids PowerPoint writes as GUIDs (sections)."""
    return "{" + str(uuid.uuid5(uuid.NAMESPACE_URL, "oxoffice-corpus/" + "/".join(names))).upper() + "}"


# ---------------------------------------------------------------- presentation


def _widescreen(prs) -> None:
    """16:9 slides, with the default master and layouts stretched to match."""
    size = prs.part._element.find(qn("p:sldSz"))
    size.set("cx", str(WIDE))
    size.set("cy", str(HIGH))
    size.attrib.pop("type", None)
    parts = [prs.slide_master.part] + [layout.part for layout in prs.slide_layouts]
    for part in parts:
        for xfrm in part._element.iter(qn("a:xfrm")):
            off, ext = xfrm.find(qn("a:off")), xfrm.find(qn("a:ext"))
            if off is not None and ext is not None:
                off.set("x", str(int(off.get("x")) * 4 // 3))
                ext.set("cx", str(int(ext.get("cx")) * 4 // 3))


def _notes_master_list(prs) -> None:
    """List the notes master in presentation.xml, as PowerPoint does
    (python-pptx only relates the part)."""
    root = prs.part._element
    if root.find(qn("p:notesMasterIdLst")) is not None:
        return
    for rel in prs.part.rels.values():
        if rel.reltype == RT.NOTES_MASTER:
            ids = parse_xml(f'<p:notesMasterIdLst {DECLS}><p:notesMasterId r:id="{rel.rId}"/></p:notesMasterIdLst>')
            root.find(qn("p:sldMasterIdLst")).addnext(ids)


def _save(prs, path: Path, title: str, author: str) -> None:
    """Fixed core properties, then save."""
    _notes_master_list(prs)
    props = prs.core_properties
    props.title = title
    props.author = author
    props.last_modified_by = author
    props.comments = ""
    props.revision = 2
    props.created = _when(DATE)
    props.modified = _when(LATER)
    prs.save(path)


def _pin_workbook(chart) -> None:
    """Give the chart's embedded workbook fixed dates: XlsxWriter stamps now()."""
    part = chart.part.chart_workbook.xlsx_part
    with zipfile.ZipFile(io.BytesIO(part.blob)) as workbook:
        items = [(info.filename, workbook.read(info)) for info in workbook.infolist()]
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as workbook:
        for name, data in items:
            if name == "docProps/core.xml":
                data = re.sub(rb"(<dcterms:(?:created|modified)[^>]*>)[^<]*", rb"\g<1>" + DATE.encode(), data)
            workbook.writestr(zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0)), data, zipfile.ZIP_DEFLATED)
    part.blob = out.getvalue()


def _sections(prs, sections: list[tuple[str, list]]) -> None:
    """Slide sections: p14:sectionLst in the presentation extLst."""
    root = prs.part._element
    body = "".join(
        f'<p14:section name="{_xml_text(name)}" id="{_guid(prs.core_properties.title or "deck", name)}"><p14:sldIdLst>'
        + "".join(f'<p14:sldId id="{slide.slide_id}"/>' for slide in slides)
        + "</p14:sldIdLst></p14:section>"
        for name, slides in sections
    )
    ext = parse_xml(
        f'<p:ext {DECLS} uri="{{521415D9-36F7-43E2-AB2F-B90AF26B5E84}}">'
        f'<p14:sectionLst xmlns:p14="{NS_P14}">{body}</p14:sectionLst></p:ext>'
    )
    ext_list = root.find(qn("p:extLst"))
    if ext_list is None:
        ext_list = parse_xml(f"<p:extLst {DECLS}/>")
        root.append(ext_list)
    ext_list.insert(0, ext)


# ---------------------------------------------------------------------- slides


def _transition(slide, effect: str, speed: str = "med") -> None:
    """A slide transition, e.g. effect='<p:push dir="u"/>'; after p:clrMapOvr."""
    transition = parse_xml(f'<p:transition {DECLS} spd="{speed}">{effect}</p:transition>')
    slide._element.find(qn("p:clrMapOvr")).addnext(transition)


def _hide(slide) -> None:
    """Hide the slide in the slide show."""
    slide._element.set("show", "0")


def _next_id(slide) -> int:
    return max(int(i) for i in slide._element.xpath("//p:cNvPr/@id")) + 1


def _footer(slide, text: str, number: int) -> None:
    """Footer and slide number placeholders, matching the layout's (type, idx, sz, field id)."""
    tree = slide.shapes._spTree
    for ph in slide.slide_layout.placeholders:
        kind = ph.placeholder_format.type
        if kind not in (PP_PLACEHOLDER.FOOTER, PP_PLACEHOLDER.SLIDE_NUMBER):
            continue
        source = ph._element.ph
        shape_id = _next_id(slide)
        if kind == PP_PLACEHOLDER.FOOTER:
            name, ph_type = f"Footer Placeholder {shape_id - 1}", "ftr"
            para = f'<a:r><a:rPr lang="en-US" dirty="0"/><a:t>{_xml_text(text)}</a:t></a:r>'
        else:
            name, ph_type = f"Slide Number Placeholder {shape_id - 1}", "sldNum"
            field_id = ph._element.xpath(".//a:fld/@id")[0]
            para = f'<a:fld id="{field_id}" type="slidenum"><a:rPr lang="en-US"/><a:t>{number}</a:t></a:fld>'
        tree.append(
            parse_xml(
                f'<p:sp {DECLS}><p:nvSpPr><p:cNvPr id="{shape_id}" name="{name}"/>'
                f'<p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
                f'<p:nvPr><p:ph type="{ph_type}" sz="{source.get("sz", "quarter")}" idx="{source.get("idx")}"/></p:nvPr>'
                f"</p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p>{para}"
                f'<a:endParaRPr lang="en-US" dirty="0"/></a:p></p:txBody></p:sp>'
            )
        )


def _comments(prs, authors: list[tuple[str, str]], notes: list[tuple]) -> None:
    """Legacy comments as PowerPoint 2016 writes them: ppt/commentAuthors.xml
    and one ppt/comments/commentN.xml per slide.

    `authors` is [(name, initials)]; `notes` is [(slide, author index, stamp,
    (x, y), text, parent)] where parent is the (author index, idx) of the
    comment answered, or None.
    """
    package = prs.part.package
    counters = [0] * len(authors)
    per_slide: dict = {}
    for slide, who, stamp, (x, y), text, parent in notes:
        counters[who] += 1
        threading = f'<p15:threadingInfo xmlns:p15="{NS_P15}" timeZoneBias="0"/>'
        if parent:
            threading = threading[:-2] + f'><p15:parentCm authorId="{parent[0] + 1}" idx="{parent[1]}"/></p15:threadingInfo>'
        per_slide.setdefault(slide.slide_id, (slide, []))[1].append(
            f'<p:cm authorId="{who + 1}" dt="{stamp[:-1]}.000" idx="{counters[who]}">'
            f'<p:pos x="{x}" y="{y}"/><p:text>{_xml_text(text)}</p:text>'
            f'<p:extLst><p:ext uri="{{C676402C-5697-4E1C-873F-D02D1690AC5C}}">{threading}</p:ext></p:extLst></p:cm>'
        )
    author_list = "".join(
        f'<p:cmAuthor id="{i + 1}" name="{_xml_text(name)}" initials="{initials}" lastIdx="{counters[i]}" clrIdx="{i}">'
        f'<p:extLst><p:ext uri="{{19B8F6BF-5375-455C-9EA6-DF929625EA0E}}">'
        f'<p15:presenceInfo xmlns:p15="{NS_P15}" userId="{_xml_text(name)}" providerId="None"/>'
        f"</p:ext></p:extLst></p:cmAuthor>"
        for i, (name, initials) in enumerate(authors)
    )
    head = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    part = Part(
        PackURI("/ppt/commentAuthors.xml"),
        CT.PML_COMMENT_AUTHORS,
        package,
        (head + f"<p:cmAuthorLst {DECLS}>{author_list}</p:cmAuthorLst>").encode(),
    )
    prs.part.relate_to(part, RT.COMMENT_AUTHORS)
    for n, (slide, items) in enumerate(per_slide.values(), start=1):
        part = Part(
            PackURI(f"/ppt/comments/comment{n}.xml"),
            CT.PML_COMMENTS,
            package,
            (head + f"<p:cmLst {DECLS}>{''.join(items)}</p:cmLst>").encode(),
        )
        slide.part.relate_to(part, RT.COMMENTS)


def _bullets(frame, items: list[tuple[str, int]]) -> None:
    """Fill a text frame with (text, level) paragraphs."""
    frame.text = items[0][0]
    frame.paragraphs[0].level = items[0][1]
    for text, level in items[1:]:
        para = frame.add_paragraph()
        para.text = text
        para.level = level


def _content_slide(prs, title: str, items: list[tuple[str, int]], notes: str | None = None):
    slide = prs.slides.add_slide(prs.slide_layouts[1])
    slide.shapes.title.text = title
    _bullets(slide.placeholders[1].text_frame, items)
    if notes:
        slide.notes_slide.notes_text_frame.text = notes
    return slide


def _title_only(prs, title: str):
    slide = prs.slides.add_slide(prs.slide_layouts[5])
    slide.shapes.title.text = title
    return slide


def _box(slide_or_group, shape, x, y, w, h, text, fill, size=14, color=RGBColor(0xFF, 0xFF, 0xFF)):
    """An auto shape with centred text."""
    item = slide_or_group.shapes.add_shape(shape, x, y, w, h)
    item.fill.solid()
    item.fill.fore_color.rgb = fill
    item.line.color.rgb = fill
    frame = item.text_frame
    frame.word_wrap = True
    frame.text = text
    for para in frame.paragraphs:
        para.alignment = PP_ALIGN.CENTER
        for run in para.runs:
            run.font.size = PPt(size)
            run.font.color.rgb = color
    return item


def _arrow(slide, start, start_site, end, end_site, kind=MSO_CONNECTOR.STRAIGHT):
    """A connector glued to two shapes, with an arrow head at the end."""
    line = slide.shapes.add_connector(kind, 0, 0, 0, 0)
    line.begin_connect(start, start_site)
    line.end_connect(end, end_site)
    line.line.width = PPt(1.75)
    line.line.color.rgb = INK
    line.line._get_or_add_ln().append(parse_xml(f'<a:tailEnd {DECLS} type="triangle"/>'))
    return line


# ----------------------------------------------------------------------- pitch


def _chart(slide, kind, data, x, y, w, h):
    frame = slide.shapes.add_chart(kind, x, y, w, h, data)
    _pin_workbook(frame.chart)
    return frame.chart


@family("pitch", "pptx")
def pitch(path: Path) -> None:
    """A seed-round pitch deck: native charts, a merged table, transitions, sections."""
    prs = Presentation()
    _widescreen(prs)
    prs.core_properties.title = "Tidewater Storage, seed round"

    slide = prs.slides.add_slide(prs.slide_layouts[0])
    slide.shapes.title.text = "Tidewater Storage"
    slide.placeholders[1].text = "Batteries for small solar farms. Seed round, November 2026"
    slide.shapes.add_picture(logo_png("TW", color=(15, 118, 110)), Emu(WIDE // 2 - Inches(0.6)), Inches(0.5), Inches(1.2))
    slide.notes_slide.notes_text_frame.text = (
        "Introduce the three founders in one sentence each. Say where we come from: grid operations, "
        "battery chemistry and energy trading. Then go straight to the problem."
    )
    title = slide

    problem = _content_slide(
        prs,
        "The problem",
        [
            ("Small solar farms curtail up to 9 % of their output at midday", 0),
            ("The grid cannot take it, and prices turn negative", 1),
            ("Grid connection queues in Europe now run three to five years", 0),
            ("Utility-scale batteries are sized and priced for 100 MW sites", 0),
            ("Minimum orders of 20 MWh, twelve months of engineering", 1),
        ],
        "Use the Beauce example: a 4 MW farm that lost 310 MWh last summer, about EUR 28,000.",
    )
    solution = _content_slide(
        prs,
        "Our solution",
        [
            ("A containerised 2 MWh battery, delivered and commissioned in ten days", 0),
            ("Software that stores midday energy and sells it at the evening peak", 0),
            ("Price forecasts trained on 40,000 site-days of data", 1),
            ("Bids placed automatically on the day-ahead and intraday markets", 1),
            ("Leased per month: no upfront cost for the farm owner", 0),
        ],
        "Stress the lease: owners do not want another capital decision.",
    )

    market = _title_only(prs, "A market of 61,000 small sites")
    data = CategoryChartData()
    data.categories = ["Under 1 MW", "1 to 5 MW", "5 to 20 MW", "Community solar"]
    data.add_series("Sites in Europe", (0.38, 0.33, 0.18, 0.11), number_format="0%")
    chart = _chart(market, XL_CHART_TYPE.PIE, data, Inches(0.8), Inches(1.5), Inches(7.4), Inches(5.4))
    chart.has_legend = True
    chart.legend.position = XL_LEGEND_POSITION.BOTTOM
    chart.legend.include_in_layout = False
    chart.plots[0].has_data_labels = True
    chart.plots[0].data_labels.number_format = "0%"
    chart.plots[0].data_labels.number_format_is_linked = False
    chart.plots[0].data_labels.position = XL_LABEL_POSITION.OUTSIDE_END
    note = market.shapes.add_textbox(Inches(8.4), Inches(2.4), Inches(4.2), Inches(2.5))
    _bullets(
        note.text_frame,
        [("Sites under 20 MW in the EU and the UK, 2026", 0), ("Source: national grid registers, our analysis", 0)],
    )
    note.text_frame.word_wrap = True
    for para in note.text_frame.paragraphs:
        para.runs[0].font.size = PPt(16)
    market.notes_slide.notes_text_frame.text = "We start with sites from 1 to 20 MW: 51 % of the market."

    traction = _title_only(prs, "Traction: contracted capacity")
    data = CategoryChartData()
    data.categories = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct"]
    data.add_series("Contracted (MWh)", (4, 6, 9, 12, 18, 22, 29, 35, 44, 52))
    data.add_series("Commissioned (MWh)", (0, 2, 4, 6, 8, 12, 16, 20, 26, 32))
    chart = _chart(traction, XL_CHART_TYPE.LINE_MARKERS, data, Inches(1), Inches(1.5), Inches(11.3), Inches(5.2))
    chart.has_legend = True
    chart.legend.position = XL_LEGEND_POSITION.BOTTOM
    chart.legend.include_in_layout = False
    chart.value_axis.has_major_gridlines = True
    traction.notes_slide.notes_text_frame.text = "Sixteen sites signed, nine running. No churn so far."

    revenue = _title_only(prs, "Revenue plan (EUR million)")
    data = CategoryChartData()
    data.categories = ["2026", "2027", "2028", "2029"]
    data.add_series("Battery leases", (0.6, 2.4, 6.1, 11.8), number_format="0.0")
    data.add_series("Energy trading", (0.2, 1.1, 3.9, 8.6), number_format="0.0")
    chart = _chart(revenue, XL_CHART_TYPE.COLUMN_CLUSTERED, data, Inches(1), Inches(1.5), Inches(11.3), Inches(5.2))
    chart.has_legend = True
    chart.legend.position = XL_LEGEND_POSITION.TOP
    chart.legend.include_in_layout = False
    chart.plots[0].has_data_labels = True
    chart.plots[0].data_labels.position = XL_LABEL_POSITION.OUTSIDE_END
    chart.plots[0].gap_width = 80

    economics = _title_only(prs, "Unit economics of one site")
    rows = [
        ("Category", "Item", "Cash flow per 2 MWh site, EUR", ""),
        ("", "", "Year 1", "Year 3"),
        ("Hardware", "Battery system", "-412,000", "-"),
        ("", "Installation and grid works", "-58,000", "-"),
        ("Revenue", "Lease from the owner", "96,000", "96,000"),
        ("", "Trading margin", "41,000", "78,000"),
        ("Payback", "", "3.4 years", ""),
    ]
    table = economics.shapes.add_table(len(rows), 4, Inches(1.2), Inches(1.6), Inches(10.9), Inches(4.4)).table
    for r, row in enumerate(rows):
        for c, value in enumerate(row):
            if value:
                table.cell(r, c).text = value
    table.cell(0, 0).merge(table.cell(1, 0))
    table.cell(0, 1).merge(table.cell(1, 1))
    table.cell(0, 2).merge(table.cell(0, 3))
    table.cell(2, 0).merge(table.cell(3, 0))
    table.cell(4, 0).merge(table.cell(5, 0))
    table.cell(6, 0).merge(table.cell(6, 1))
    table.cell(6, 2).merge(table.cell(6, 3))
    table.columns[0].width = Inches(2.2)
    table.columns[1].width = Inches(4.3)
    table.columns[2].width = Inches(2.2)
    table.columns[3].width = Inches(2.2)
    economics.notes_slide.notes_text_frame.text = "Payback assumes today's spread; the battery lasts fifteen years."

    competition = _content_slide(
        prs,
        "Competition",
        [
            ("Utility-scale integrators", 0),
            ("Minimum order of 20 MWh, no offer below 10 MW", 1),
            ("Residential battery makers", 0),
            ("Hardware only, no market access, no trading software", 1),
            ("Tidewater", 0),
            ("Sized for 1 to 20 MW, revenue shared with the owner", 1),
        ],
    )

    team = _content_slide(
        prs,
        "Team",
        [
            ("Claire Morand, chief executive", 0),
            ("Ten years in grid operations at a distribution network operator", 1),
            ("Daniel Weiss, chief technology officer", 0),
            ("Battery chemist, previously led cell testing at a carmaker", 1),
            ("Ana Ribeiro, head of trading", 0),
            ("Built the intraday desk of a regional utility", 1),
        ],
    )
    ask = _content_slide(
        prs,
        "The ask: EUR 4 million",
        [
            ("Eighteen months of runway to 60 commissioned sites", 0),
            ("Use of funds", 0),
            ("Deployment team in France and Germany: 45 %", 1),
            ("Trading software and forecasting: 30 %", 1),
            ("Working capital for battery leases: 25 %", 1),
        ],
        "Pause here for questions. Leave the data room link in the chat.",
    )

    effects = [
        (title, "<p:fade/>", "slow"),
        (problem, '<p:push dir="u"/>', "med"),
        (solution, '<p:push dir="u"/>', "med"),
        (market, '<p:wipe dir="r"/>', "med"),
        (traction, '<p:wipe dir="r"/>', "med"),
        (revenue, '<p:split orient="vert" dir="out"/>', "med"),
        (economics, '<p:cover dir="l"/>', "fast"),
        (competition, '<p:randomBar dir="horz"/>', "med"),
        (team, '<p:zoom dir="in"/>', "med"),
        (ask, '<p:fade thruBlk="1"/>', "slow"),
    ]
    for slide, effect, speed in effects:
        _transition(slide, effect, speed)
    _sections(
        prs,
        [
            ("Opening", [title, problem, solution]),
            ("Market and traction", [market, traction]),
            ("Business model", [revenue, economics, competition]),
            ("Team and ask", [team, ask]),
        ],
    )
    _save(prs, path, "Tidewater Storage, seed round", "Claire Morand")


# -------------------------------------------------------------------- training

FOOTER = "Support onboarding, module 3"


def _button(slide, text, target, x, y):
    """A rounded button that jumps to another slide when clicked."""
    button = _box(slide, MSO_SHAPE.ROUNDED_RECTANGLE, x, y, Inches(2.8), Inches(0.6), text, INDIGO, size=14)
    button.click_action.target_slide = target
    return button


def _person(group, initials, name, x, y, fill):
    """A person card in a group: initials in a circle and the name beside it."""
    _box(group, MSO_SHAPE.OVAL, x, y, Inches(0.7), Inches(0.7), initials, fill, size=12)
    label = group.shapes.add_textbox(x + Inches(0.8), y + Inches(0.12), Inches(2.2), Inches(0.5))
    label.text_frame.text = name
    label.text_frame.paragraphs[0].runs[0].font.size = PPt(14)


@family("training", "pptx")
def training(path: Path) -> None:
    """A training module: groups, auto shapes, connectors, links, hidden slide, footers, comments."""
    prs = Presentation()
    _widescreen(prs)
    prs.core_properties.title = "Incident Response Essentials"

    slide = prs.slides.add_slide(prs.slide_layouts[0])
    slide.shapes.title.text = "Incident Response Essentials"
    slide.placeholders[1].text = "Support engineering onboarding, module 3"
    slide.notes_slide.notes_text_frame.text = "Welcome the group, check that everyone has access to the runbook site."

    agenda = _content_slide(
        prs,
        "Agenda",
        [
            ("Why incidents need a shared process", 0),
            ("Four severity levels", 0),
            ("From SEV1 to SEV4, with an example for each", 1),
            ("The incident lifecycle and the escalation path", 0),
            ("Who decides, who informs, who fixes", 1),
            ("Who is on call this week", 1),
            ("Tools and runbooks", 0),
            ("Check your understanding", 0),
        ],
        "Forty-five minutes in total. The quiz at the end is not graded.",
    )

    levels = _title_only(prs, "Four severity levels")
    cards = [
        ("SEV1\nService down for every customer", RED),
        ("SEV2\nA major feature broken, no workaround", AMBER),
        ("SEV3\nDegraded, a workaround exists", INDIGO),
        ("SEV4\nCosmetic, or one customer affected", GREY),
    ]
    for i, (text, fill) in enumerate(cards):
        x = Inches(0.8 + i * 3.0)
        _box(levels, MSO_SHAPE.ROUNDED_RECTANGLE, x, Inches(2.0), Inches(2.7), Inches(2.2), text, fill, size=16)
    urgency = "Increasing urgency"
    _box(levels, MSO_SHAPE.LEFT_ARROW, Inches(0.8), Inches(4.6), Inches(11.7), Inches(0.7), urgency, INK, size=14)
    note = levels.shapes.add_textbox(Inches(0.8), Inches(5.6), Inches(11.7), Inches(0.8))
    note.text_frame.word_wrap = True
    note.text_frame.text = "When in doubt, pick the higher level: lowering a severity later costs nothing."
    levels.notes_slide.notes_text_frame.text = "Ask the group for a recent incident and classify it together."

    lifecycle = _title_only(prs, "The incident lifecycle")
    steps = ["Detect", "Triage", "Mitigate", "Resolve", "Review"]
    for i, step in enumerate(steps):
        shape = MSO_SHAPE.PENTAGON if i == 0 else MSO_SHAPE.CHEVRON
        _box(lifecycle, shape, Inches(0.7 + i * 2.4), Inches(2.4), Inches(2.6), Inches(1.2), step, TEAL, size=18)
    _bullets(
        lifecycle.shapes.add_textbox(Inches(0.9), Inches(4.2), Inches(11.5), Inches(2.0)).text_frame,
        [
            ("Mitigate before you resolve: restore the service first, find the cause after", 0),
            ("Every SEV1 and SEV2 ends with a written review within five working days", 0),
        ],
    )

    escalation = _title_only(prs, "The escalation path")
    alert, triage, impact, commander, queue, review = (
        _box(escalation, shape, Inches(x), Inches(y), Inches(w), Inches(h), text, fill, size=size)
        for shape, (x, y, w, h), text, fill, size in (
            (MSO_SHAPE.FLOWCHART_TERMINATOR, (0.6, 3.3, 2.0, 0.9), "Alert received", TEAL, 14),
            (MSO_SHAPE.FLOWCHART_PROCESS, (3.2, 3.2, 2.3, 1.1), "On-call engineer triages", INDIGO, 14),
            (MSO_SHAPE.FLOWCHART_DECISION, (6.0, 2.95, 2.4, 1.6), "Customer impact?", AMBER, 14),
            (MSO_SHAPE.FLOWCHART_PROCESS, (9.2, 1.6, 2.6, 1.1), "Page the incident commander", RED, 14),
            (MSO_SHAPE.FLOWCHART_PROCESS, (9.2, 4.8, 2.6, 1.1), "Fix within the ticket SLA", GREY, 14),
            (MSO_SHAPE.FLOWCHART_TERMINATOR, (9.5, 3.3, 2.0, 0.9), "Write the review", TEAL, 13),
        )
    )
    _arrow(escalation, alert, 3, triage, 1)
    _arrow(escalation, triage, 3, impact, 1)
    _arrow(escalation, impact, 0, commander, 1, MSO_CONNECTOR.ELBOW)
    _arrow(escalation, impact, 2, queue, 1, MSO_CONNECTOR.ELBOW)
    _arrow(escalation, commander, 2, review, 0)
    _arrow(escalation, queue, 0, review, 2)
    for text, x, y in (("Yes", 6.6, 2.2), ("No", 6.6, 4.7)):
        label = escalation.shapes.add_textbox(Inches(x), Inches(y), Inches(0.8), Inches(0.4))
        label.text_frame.text = text
    escalation.notes_slide.notes_text_frame.text = "Walk through the last SEV2 on this diagram, box by box."

    rota = _title_only(prs, "Who is on call this week")
    outer = rota.shapes.add_group_shape()
    outer.name = "On-call rota"
    week = "Week 47, Monday 08:00 to Monday 08:00"
    _box(outer, MSO_SHAPE.RECTANGLE, Inches(0.8), Inches(1.7), Inches(11.7), Inches(0.6), week, INK, size=16)
    for t, (tier, people, fill) in enumerate(
        (
            ("Tier 1: first response", (("SL", "Sara Lindqvist"), ("JP", "Jonas Petit"), ("AM", "Amira Mansour")), TEAL),
            ("Tier 2: platform experts", (("KO", "Kwame Owusu"), ("LB", "Lea Brunner")), INDIGO),
        )
    ):
        inner = outer.shapes.add_group_shape()
        inner.name = tier.split(":")[0]
        x = Inches(0.8 + t * 6.0)
        _box(inner, MSO_SHAPE.ROUNDED_RECTANGLE, x, Inches(2.6), Inches(5.7), Inches(0.6), tier, fill, size=16)
        for p, (initials, name) in enumerate(people):
            _person(inner, initials, name, x + Inches(0.2), Inches(3.4 + p * 0.9), fill)
    rota.notes_slide.notes_text_frame.text = "Swaps go through the rota channel, never by private message."

    tools = _content_slide(prs, "Tools and runbooks", [("Status page for customers: ", 0)])
    frame = tools.placeholders[1].text_frame
    links = [
        ("status.northwind.example", "https://status.northwind.example/"),
        ("the incident runbook", "https://runbooks.northwind.example/incident-response"),
        ("Managing Incidents, in the Google SRE book", "https://sre.google/sre-book/managing-incidents/"),
    ]
    run = frame.paragraphs[0].add_run()
    run.text, run.hyperlink.address = links[0]
    for prefix, (label, url), level in (
        ("Start every incident from ", links[1], 0),
        ("Further reading: ", links[2], 1),
    ):
        para = frame.add_paragraph()
        para.level = level
        para.text = prefix
        run = para.add_run()
        run.text, run.hyperlink.address = label, url
    for text, level in (
        ("Paging: the on-call app, never phone calls first", 0),
        ("Chat: one channel per incident, named after its ticket", 0),
    ):
        para = frame.add_paragraph()
        para.text, para.level = text, level

    trainer = _content_slide(
        prs,
        "Trainer notes: timing and exercises",
        [
            ("Severity levels: 10 minutes, classify three real tickets", 0),
            ("Escalation path: 15 minutes, role play with two volunteers", 0),
            ("Rota: 5 minutes, show where swaps are recorded", 0),
            ("Quiz: 10 minutes, answers on the next page of the handbook", 0),
        ],
        "Hidden slide: for the trainer only, skipped in the slide show.",
    )
    _hide(trainer)

    quiz = _content_slide(
        prs,
        "Check your understanding",
        [
            ("A customer reports slow exports, other customers are fine. Which severity?", 0),
            ("Hint: one customer, a workaround exists", 1),
            ("Who pages the incident commander?", 0),
            ("When is the written review due after a SEV2?", 0),
        ],
        "Give two minutes per question, then reveal the answers on the escalation slide.",
    )
    _button(quiz, "Back to the escalation path", escalation, Inches(9.6), Inches(6.0))
    _button(agenda, "Jump to the quiz", quiz, Inches(9.6), Inches(6.0))

    summary = _content_slide(
        prs,
        "Summary",
        [
            ("Classify first, and pick the higher level when unsure", 0),
            ("Customer impact decides whether the incident commander is paged", 0),
            ("Restore the service, then find the cause", 0),
            ("Every SEV1 and SEV2 gets a review", 0),
        ],
        "Hand out the laminated escalation card and collect feedback forms.",
    )

    for number, slide in enumerate(prs.slides, start=1):
        if number > 1:
            _footer(slide, FOOTER, number)
    _comments(
        prs,
        [("Maya Chen", "MC"), ("Tom Okafor", "TO")],
        [
            (levels, 0, DATE, (5480, 1120), "Add an example incident for each level: trainees asked for one last time.", None),
            (escalation, 1, DATE, (3920, 1810), "Should the decision box mention security incidents? They skip the queue.", None),
            (escalation, 0, LATER, (3920, 1810), "Yes, I will add a separate branch for security in the next revision.", (1, 1)),
        ],
    )
    _save(prs, path, "Incident Response Essentials", "Maya Chen")


# ---------------------------------------------------------------- multilingual

ARABIC = ("ar-SA", "cs", "Arial")
HEBREW = ("he-IL", "cs", "Arial")
CHINESE = ("zh-CN", "ea", "Microsoft YaHei")
JAPANESE = ("ja-JP", "ea", "Yu Gothic")
ENGLISH = ("en-US", None, None)


def _run(para, text: str, script, size: int | None = None) -> None:
    """Append a run with its language and, for complex or East Asian scripts, its typeface."""
    lang, slot, face = script
    alt = ' altLang="en-US"' if lang != "en-US" else ""
    sz = f' sz="{size * 100}"' if size else ""
    font = f'<a:{slot} typeface="{face}"/>' if slot else ""
    para._p.append(parse_xml(f'<a:r {DECLS}><a:rPr lang="{lang}"{alt}{sz} dirty="0">{font}</a:rPr><a:t>{_xml_text(text)}</a:t></a:r>'))


def _para(para, text: str, script, rtl: bool = False, level: int = 0, size: int | None = None) -> None:
    """Fill an empty paragraph with one run; right-aligned right-to-left for Arabic and Hebrew."""
    attrs = (f' lvl="{level}"' if level else "") + (' algn="r" rtl="1"' if rtl else "")
    if attrs:
        para._p.insert(0, parse_xml(f"<a:pPr {DECLS}{attrs}/>"))
    _run(para, text, script, size)


def _script_slide(prs, title: str, items: list[tuple[str, int]], script, rtl: bool, notes: str):
    """A title and content slide written entirely in one script."""
    slide = prs.slides.add_slide(prs.slide_layouts[1])
    _para(slide.shapes.title.text_frame.paragraphs[0], title, script, rtl)
    frame = slide.placeholders[1].text_frame
    for i, (text, level) in enumerate(items):
        _para(frame.paragraphs[0] if i == 0 else frame.add_paragraph(), text, script, rtl, level)
    slide.notes_slide.notes_text_frame.text = notes
    return slide


@family("multilingual", "pptx")
def multilingual(path: Path) -> None:
    """Survey results in Arabic, Hebrew, Chinese and Japanese, a mixed-script slide and a bilingual table."""
    prs = Presentation()
    _widescreen(prs)

    slide = prs.slides.add_slide(prs.slide_layouts[0])
    slide.shapes.title.text = "Customer Survey 2026"
    slide.placeholders[1].text = "Regional results, presented in each region's language"
    slide.notes_slide.notes_text_frame.text = "Each regional lead presents their own slide."

    _script_slide(
        prs,
        "الشرق الأوسط: النتائج الرئيسية",
        [
            ("ارتفع رضا العملاء إلى 84% هذا العام", 0),
            ("يطلب العملاء دعماً باللغة العربية على مدار الساعة", 0),
            ("تحسّن وقت الاستجابة بنسبة 30% بعد افتتاح مكتب دبي", 0),
            ("الأولوية للعام القادم: تطبيق الهاتف المحمول", 0),
            ("الإطلاق التجريبي في الرياض خلال الربع الثاني", 1),
        ],
        ARABIC,
        True,
        "Middle East: satisfaction up to 84 %, Arabic support around the clock, mobile app next year.",
    )
    _script_slide(
        prs,
        "ישראל: ממצאים עיקריים",
        [
            ("שביעות הרצון של הלקוחות עלתה ל-79%", 0),
            ("הלקוחות מבקשים חשבוניות בעברית", 0),
            ("זמן הטיפול בפניות ירד בשליש", 0),
            ("בשנה הבאה: הרחבת צוות התמיכה בתל אביב", 0),
            ("שני עובדים חדשים ברבעון הראשון", 1),
        ],
        HEBREW,
        True,
        "Israel: satisfaction at 79 %, invoices in Hebrew requested, the Tel Aviv team grows.",
    )
    _script_slide(
        prs,
        "中国：主要调查结果",
        [
            ("客户满意度上升至81%", 0),
            ("客户希望增加微信支付选项", 0),
            ("上海仓库的交货时间缩短了两天", 0),
            ("明年的重点：本地化的培训材料", 0),
            ("第一批中文视频将于三月发布", 1),
        ],
        CHINESE,
        False,
        "China: satisfaction at 81 %, WeChat Pay requested, faster deliveries from Shanghai.",
    )
    _script_slide(
        prs,
        "日本：主な調査結果",
        [
            ("顧客満足度は前年から6ポイント上昇しました", 0),
            ("請求書の電子化を求める声が多く寄せられました", 0),
            ("大阪の新しいサポート窓口は好評です", 0),
            ("来年度の課題：導入研修の充実", 0),
            ("オンライン研修を毎月開催します", 1),
        ],
        JAPANESE,
        False,
        "Japan: satisfaction up six points, electronic invoices requested, the Osaka desk is well received.",
    )

    mixed = _title_only(prs, "One message, four scripts")
    frame = mixed.shapes.add_textbox(Inches(0.9), Inches(1.7), Inches(11.5), Inches(4.8)).text_frame
    frame.word_wrap = True
    first = frame.paragraphs[0]
    for text, script in (
        ("Thank you: ", ENGLISH),
        ("شكراً", ARABIC),
        (" / ", ENGLISH),
        ("תודה", HEBREW),
        (" / ", ENGLISH),
        ("谢谢", CHINESE),
        (" / ", ENGLISH),
        ("ありがとうございます", JAPANESE),
    ):
        _run(first, text, script, 28)
    para = frame.add_paragraph()
    for text, script in (
        ("The new product name, ", ENGLISH),
        ("نورث ويند", ARABIC),
        (", is written right to left inside an English sentence.", ENGLISH),
    ):
        _run(para, text, script, 22)
    para = frame.add_paragraph()
    para._p.append(parse_xml(f'<a:pPr {DECLS} algn="r" rtl="1"/>'))
    for text, script in (("فريق ", ARABIC), ("Northwind", ENGLISH), (" في دبي يرحب بكم", ARABIC)):
        _run(para, text, script, 22)
    para = frame.add_paragraph()
    for text, script in (("Tokyo office: ", ENGLISH), ("東京オフィス", JAPANESE), (", Shanghai office: ", ENGLISH), ("上海办公室", CHINESE)):
        _run(para, text, script, 22)
    mixed.notes_slide.notes_text_frame.text = "The same greeting in the four languages of the survey."

    glossary = _title_only(prs, "Survey terms in English and Arabic")
    rows = [
        ("English", "العربية"),
        ("Customer satisfaction", "رضا العملاء"),
        ("Response time", "وقت الاستجابة"),
        ("Support languages", "لغات الدعم"),
        ("Mobile app", "تطبيق الهاتف المحمول"),
        ("Electronic invoices", "الفواتير الإلكترونية"),
    ]
    table = glossary.shapes.add_table(len(rows), 2, Inches(1.5), Inches(1.7), Inches(10.3), Inches(4.2)).table
    for r, (english, arabic) in enumerate(rows):
        _para(table.cell(r, 0).text_frame.paragraphs[0], english, ENGLISH)
        _para(table.cell(r, 1).text_frame.paragraphs[0], arabic, ARABIC, rtl=True)

    _save(prs, path, "Customer Survey 2026", "Leila Haddad")


# ------------------------------------------------------------------- long-deck

REGIONS = (
    "Paris", "Lyon", "Brussels", "Amsterdam", "Frankfurt", "Munich", "Milan",
    "Madrid", "Barcelona", "Lisbon", "Dublin", "London", "Copenhagen", "Warsaw",
)


@family("long-deck", "pptx")
def long_deck(path: Path) -> None:
    """60 slides of the deck pattern: four slides per region, one picture shared by all."""
    prs = Presentation()
    _widescreen(prs)
    rng = random.Random(60)

    slide = prs.slides.add_slide(prs.slide_layouts[0])
    slide.shapes.title.text = "Regional Operations Review 2026"
    slide.placeholders[1].text = "Northwind Analytics, fourteen regions"
    slide.notes_slide.notes_text_frame.text = "One block of four slides per region, in the order of the agenda."

    for n, region in enumerate(REGIONS):
        lines = sentences(1000 + n, 4)
        _content_slide(
            prs,
            f"{region}: highlights",
            [(lines[0], 0), (lines[1], 1), (lines[2], 0), (lines[3], 1)],
            " ".join(sentences(2000 + n, 2)),
        )

        slide = _title_only(prs, f"{region}: key figures")
        revenue = rng.randint(2400, 6800)
        rows = [("Indicator", "2025", "2026", "Change")]
        for label in ("Revenue (kEUR)", "Costs (kEUR)", "Customers", "Tickets closed"):
            before = rng.randint(800, 6000) if label != "Revenue (kEUR)" else revenue
            after = before + rng.randint(-200, 900)
            rows.append((label, f"{before:,}", f"{after:,}", f"{(after - before) / before:+.1%}"))
        table = slide.shapes.add_table(len(rows), 4, Inches(1.2), Inches(1.8), Inches(10.9), Inches(3.6)).table
        for r, row in enumerate(rows):
            for c, value in enumerate(row):
                table.cell(r, c).text = value
        slide.notes_slide.notes_text_frame.text = " ".join(sentences(3000 + n, 2))

        slide = _title_only(prs, f"{region}: revenue trend")
        slide.shapes.add_picture(chart_png(), Inches(1.7), Inches(1.5), width=Inches(9.9))
        caption = slide.shapes.add_textbox(Inches(1.7), Inches(6.5), Inches(9.9), Inches(0.5))
        caption.text_frame.text = f"{region}, revenue indexed on the first quarter"
        caption.text_frame.paragraphs[0].runs[0].font.size = PPt(12)
        slide.notes_slide.notes_text_frame.text = " ".join(sentences(4000 + n, 2))

        lines = sentences(5000 + n, 3)
        _content_slide(
            prs,
            f"{region}: actions for 2027",
            [(lines[0], 0), (lines[1], 0), (lines[2], 1)],
            " ".join(sentences(6000 + n, 2)),
        )

    _content_slide(
        prs,
        "Summary",
        [
            ("Revenue grew in twelve of fourteen regions", 0),
            ("Costs held flat outside the two new warehouses", 0),
            ("Ticket volumes rose with the customer base, not faster", 0),
        ],
        "Keep this to two minutes.",
    )
    _content_slide(
        prs,
        "Next steps",
        [
            ("Regional plans for 2027 due on 15 January", 0),
            ("Shared reporting calendar from February", 0),
            ("Template and figures in the finance workspace", 1),
        ],
        "Remind the leads of the January deadline.",
    )
    _content_slide(prs, "Questions", [("Thank you", 0)], "Open the floor.")
    _save(prs, path, "Regional Operations Review 2026", "Northwind Analytics")


# ------------------------------------------------------------------------ deck


@family("deck", "pptx")
def deck(path: Path) -> None:
    prs = Presentation()
    # The pilot's size, 305 EMU short of 16:9: every producer's copy has it.
    prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)

    slide = prs.slides.add_slide(prs.slide_layouts[0])
    slide.shapes.title.text = "Quarterly Operations Review"
    slide.placeholders[1].text = "Northwind Analytics, Q3"

    slide = prs.slides.add_slide(prs.slide_layouts[1])
    slide.shapes.title.text = "Highlights"
    body = slide.placeholders[1].text_frame
    body.text = "Subscription revenue up 13 %"
    for text, level in (("New tier adopted by 18 % of customers", 1), ("Churn at 2.1 % per month", 0), ("Lyon warehouse open", 0)):
        para = body.add_paragraph()
        para.text = text
        para.level = level
    slide.notes_slide.notes_text_frame.text = "Start with the revenue story, then churn."

    slide = prs.slides.add_slide(prs.slide_layouts[5])
    slide.shapes.title.text = "Key figures"
    rows = [("Indicator", "Q2", "Q3"), ("Revenue", "4,817", "5,362"), ("Costs", "3,950", "4,240"), ("Margin", "867", "1,122")]
    shape = slide.shapes.add_table(len(rows), 3, Inches(1), Inches(1.8), Inches(8), Inches(3))
    for r, row in enumerate(rows):
        for c, value in enumerate(row):
            shape.table.cell(r, c).text = value

    slide = prs.slides.add_slide(prs.slide_layouts[5])
    slide.shapes.title.text = "Revenue by quarter"
    slide.shapes.add_picture(chart_png(), Inches(1.5), Inches(1.6), width=Inches(10))
    box = slide.shapes.add_textbox(Inches(1.5), Inches(6.6), Inches(10), Inches(0.5))
    box.text_frame.text = "Indexed, Q1 = 42"
    box.text_frame.paragraphs[0].runs[0].font.size = PPt(12)

    prs.core_properties.title = "Quarterly Operations Review"
    prs.save(path)

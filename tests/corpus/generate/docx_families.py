"""The Word families of the corpus, written with python-docx.

Each builder writes one source document; see common.py for the registry.
python-docx writes the structure, ooxml.py the parts and elements it lacks
(notes, fields, revisions, content controls, equations, drawings, charts).
"""

import io
from datetime import datetime
from itertools import count
from pathlib import Path

from docx import Document
from docx.enum.section import WD_ORIENT, WD_SECTION
from docx.enum.table import WD_TABLE_ALIGNMENT
from docx.enum.text import WD_ALIGN_PARAGRAPH, WD_BREAK, WD_TAB_ALIGNMENT, WD_TAB_LEADER
from docx.oxml.ns import qn
from docx.shared import Cm, Pt, RGBColor
from docx.text.paragraph import Paragraph

import ooxml as ox
from common import DATE, LATER, chart_png, family, logo_png, paragraph_text, sentences


def _properties(doc, title: str, author: str, subject: str = "") -> None:
    """Core properties with the fixed corpus dates."""
    props = doc.core_properties
    props.title, props.author, props.subject = title, author, subject
    props.last_modified_by, props.revision = author, 1
    props.created = datetime.fromisoformat(DATE).replace(tzinfo=None)
    props.modified = datetime.fromisoformat(LATER).replace(tzinfo=None)
    props.comments = ""


def _new(font: str = "Calibri", size: int = 11, margins_cm: float = 2.5):
    """A document with its Normal font and page margins set."""
    doc = Document()
    doc.styles["Normal"].font.name = font
    doc.styles["Normal"].font.size = Pt(size)
    section = doc.sections[0]
    section.left_margin = section.right_margin = Cm(margins_cm)
    return doc


def _paragraph(doc, fragment: str) -> Paragraph:
    """Append one raw paragraph to the body and return it as a python-docx paragraph."""
    return Paragraph(ox.body_append(doc, fragment)[0], doc._body)


def _page_x_of_y(paragraph, prefix: str = "") -> None:
    """Fill a footer paragraph with "prefix Page X of Y" (PAGE and NUMPAGES fields)."""
    ox.append(paragraph._p, ox.run(f"{prefix}Page ") + ox.field("PAGE", "1") + ox.run(" of ") + ox.field("NUMPAGES", "1"))


@family("report", "docx")
def report(path: Path) -> None:
    doc = Document()
    styles = doc.styles
    styles["Normal"].font.name = "Calibri"
    styles["Normal"].font.size = Pt(11)

    section = doc.sections[0]
    section.left_margin = section.right_margin = Cm(2.2)
    section.header.paragraphs[0].text = "Northwind Analytics: Quarterly Operations Review"
    section.footer.paragraphs[0].text = "Internal. Prepared by the operations team."

    doc.add_heading("Quarterly Operations Review", level=0)
    intro = doc.add_paragraph()
    intro.add_run("Summary. ").bold = True
    intro.add_run(
        "Revenue grew by 12 % over the quarter, driven by the new subscription tier and a lower churn rate. "
        "Support response times improved, while warehouse costs rose with the expansion of the Lyon site."
    )

    doc.add_heading("1. Highlights", level=1)
    for item in (
        "Subscription revenue reached 4.2 million euros, up from 3.7 million.",
        "Churn fell to 2.1 % per month, its lowest level in two years.",
        "The Lyon warehouse opened on schedule and serves the southern region.",
    ):
        doc.add_paragraph(item, style="List Bullet")

    doc.add_heading("2. Financial overview", level=1)
    doc.add_paragraph(
        "The table below compares the main indicators with the previous quarter. Figures are in thousands of euros "
        "unless stated otherwise."
    )
    rows = [
        ("Indicator", "Q2", "Q3", "Change"),
        ("Subscription revenue", "3,712", "4,198", "+13.1 %"),
        ("Services revenue", "1,105", "1,164", "+5.3 %"),
        ("Operating costs", "3,950", "4,240", "+7.3 %"),
        ("Operating margin", "867", "1,122", "+29.4 %"),
    ]
    table = doc.add_table(rows=len(rows) + 1, cols=4)
    table.style = "Light Grid Accent 1"
    table.alignment = WD_TABLE_ALIGNMENT.CENTER
    for r, row in enumerate(rows):
        for c, value in enumerate(row):
            table.cell(r, c).text = value
    note = table.cell(len(rows), 0).merge(table.cell(len(rows), 3))
    note.text = "Operating margin excludes one-off costs of the Lyon opening (212)."

    doc.add_paragraph()
    doc.add_picture(chart_png(), width=Cm(15))
    caption = doc.add_paragraph("Figure 1. Subscription revenue by quarter, indexed.")
    caption.runs[0].italic = True

    doc.add_heading("3. Operations", level=1)
    doc.add_heading("3.1 Support", level=2)
    p = doc.add_paragraph("Median first response time went from ")
    p.add_run("6 h 40").bold = True
    p.add_run(" to ")
    p.add_run("3 h 15").bold = True
    p.add_run(". The team handled 18,400 tickets, 9 % more than in Q2.")
    doc.add_heading("3.2 Next steps", level=2)
    for step in (
        "Hire two support engineers for the evening shift.",
        "Move the remaining customers to the new billing system.",
        "Review warehouse contracts before the January renewal.",
    ):
        doc.add_paragraph(step, style="List Number")
    comment_target = doc.add_paragraph("The budget for the billing migration is still to be confirmed.")
    comment = doc.add_comment(comment_target.runs, text="Confirm with finance before sending.", author="Reviewer", initials="RV")
    # The pilot source's date, so that the source matches the pilot's copies
    # (Word keeps it to the minute).
    comment._comment_elm.set(qn("w:date"), "2026-11-20T21:51:59Z")

    doc.add_paragraph().add_run().add_break(WD_BREAK.PAGE)
    landscape = doc.add_section()
    landscape.orientation = WD_ORIENT.LANDSCAPE
    landscape.page_width, landscape.page_height = landscape.page_height, landscape.page_width
    doc.add_heading("Appendix A. Regional detail", level=1)
    regional = doc.add_table(rows=1, cols=5)
    regional.style = "Table Grid"
    for c, value in enumerate(("Region", "Customers", "Revenue", "Churn", "Tickets")):
        cell = regional.rows[0].cells[c]
        cell.text = value
        cell.paragraphs[0].runs[0].bold = True
    for region in (
        ("North", "1,240", "1,402", "1.9 %", "5,120"),
        ("South", "980", "1,118", "2.4 %", "4,870"),
        ("East", "860", "931", "2.0 %", "3,990"),
        ("West", "1,010", "1,128", "2.2 %", "4,420"),
    ):
        cells = regional.add_row().cells
        for c, value in enumerate(region):
            cells[c].text = value
    disclaimer = doc.add_paragraph("All figures are unaudited.")
    disclaimer.runs[0].font.color.rgb = RGBColor(0x64, 0x74, 0x8B)

    doc.core_properties.title = "Quarterly Operations Review"
    doc.core_properties.author = "Operations team"
    doc.save(path)


# Contract


@family("contract", "docx")
def contract(path: Path) -> None:
    """A services agreement under negotiation: legal numbering, cross references,
    tracked changes by two authors, comment threads, footnotes, Page X of Y."""
    doc = _new("Times New Roman", 11)
    ox.add_styles(doc, "Hyperlink")
    supplier, customer = "Claire Dubois", "James Patel"
    legal = ox.add_numbering(
        doc, [("decimal", "%1.", 567, 567), ("decimal", "%1.%2", 1134, 567), ("decimal", "%1.%2.%3", 1871, 737)], "4C1E6A01"
    )
    bookmarks, revisions = count(1), count(100)

    section = doc.sections[0]
    section.header.paragraphs[0].text = "Master Services Agreement - Execution version"
    section.header.paragraphs[0].alignment = WD_ALIGN_PARAGRAPH.RIGHT
    footer = section.footer.paragraphs[0]
    footer.alignment = WD_ALIGN_PARAGRAPH.CENTER
    _page_x_of_y(footer, "Confidential - ")

    doc.add_heading("Master Services Agreement", level=0)
    doc.add_paragraph("between Northwind Analytics SAS and Contoso Logistics Ltd").runs[0].italic = True
    doc.add_paragraph(
        'This Agreement is made on 21 November 2026 (the "Effective Date") between the parties named below.'
    )
    doc.add_heading("Parties", level=2)
    for text in (
        ("(1) Northwind Analytics SAS, a company incorporated in France under number 812 345 678, whose registered "
        'office is at 14 rue des Halles, 69002 Lyon, France (the "Supplier"); and'),
        ("(2) Contoso Logistics Ltd, a company incorporated in England and Wales under number 09876543, whose "
        'registered office is at 2 Canal Wharf, Leeds LS11 5PS, United Kingdom (the "Customer").'),
    ):
        doc.add_paragraph(text)
    doc.add_heading("Background", level=2)
    for text in (
        "(A) The Supplier provides data analytics and reporting services to logistics companies.",
        ("(B) The Customer wishes to appoint the Supplier to provide those services for its European warehouses, "
        "and the Supplier agrees to do so on the terms of this Agreement."),
    ):
        doc.add_paragraph(text)

    def clause(level: int, content: str, ppr: str = "") -> Paragraph:
        style = "Heading1" if level == 0 else ""
        return _paragraph(doc, ox.para(content, ox.numbered(legal, level) + ppr, style=style))

    def ref(name: str, shown: str) -> str:
        return ox.field(f"REF {name} \\r \\h", shown)

    clause(0, ox.run("Definitions and interpretation"))
    clause(1, ox.run("In this Agreement the following definitions apply:"))
    for term, meaning in (
        ("Business Day", "a day other than a Saturday, Sunday or public holiday in England or France."),
        ("Confidential Information", ("all information of a confidential nature disclosed by one party to the "
         "other, in any form, including the terms of this Agreement.")),
        ("Services", "the analytics, reporting and support services described in Schedule 1."),
        ("Service Levels", "the availability and response commitments set out in Schedule 3."),
    ):
        clause(2, ox.run(f'"{term}"', "<w:b/>") + ox.run(f" means {meaning}"))
    clause(1, ox.run("Clause headings do not affect the interpretation of this Agreement. A reference to a clause "
                     "is to a clause of this Agreement, and words in the singular include the plural."))

    clause(0, ox.run("Term"))
    clause(1, ox.run('This Agreement starts on the Effective Date and continues for an initial term of thirty-six '
                     '(36) months (the "Initial Term").'))
    renewal = clause(
        1,
        ox.run("It renews automatically for successive periods of twelve (12) months unless either party gives at "
               "least ")
        + ox.deleted(next(revisions), customer, LATER, "ninety (90)")
        + ox.inserted(next(revisions), customer, LATER, ox.run("one hundred and twenty (120)"))
        + ox.run(" days' written notice before the end of the current term."),
    )

    clause(0, ox.run("Services"))
    clause(1, ox.run("The Supplier shall provide the Services in accordance with Schedule 1 and shall meet or exceed "
                     "the Service Levels."))
    clause(1, ox.run("The Supplier shall use appropriately qualified staff and shall comply with the Customer's site "
                     "rules when working at the Customer's premises."))
    clause(
        1,
        ox.inserted(next(revisions), supplier, DATE,
                    ox.run("The Supplier shall appoint a named account manager, who shall attend a monthly service "
                           "review with the Customer.")),
        ox.inserted_mark(next(revisions), supplier, DATE),
    )

    clause(0, ox.run("Fees and payment"))
    clause(1, ox.run("The Customer shall pay the fees set out in Schedule 2. Fees are exclusive of VAT, which the "
                     "Customer shall pay at the applicable rate."))
    payment_ref = "_Ref217004201"
    clause(
        1,
        ox.bookmark(
            next(bookmarks), payment_ref,
            ox.run("The Supplier shall invoice the Customer monthly in arrears. Invoices are payable within ")
            + ox.deleted(next(revisions), supplier, DATE, "forty-five (45)")
            + ox.inserted(next(revisions), supplier, DATE, ox.run("thirty (30)"))
            + ox.run(" days of receipt of a valid invoice."),
        ),
    )
    late = clause(1, ox.run("If the Customer fails to pay an undisputed amount when due, the Supplier may charge "
                            "interest on that amount at the statutory rate"))
    ox.add_note(doc, late, "footnote", "In France, the rate set by Article L.441-10 of the Commercial Code; in England, "
                                       "the rate under the Late Payment of Commercial Debts (Interest) Act 1998.")
    ox.append(late._p, ox.run(", from the due date until the date of payment."))
    clause(1, ox.run("The Customer may withhold a disputed amount, provided that it notifies the Supplier within ten "
                     "(10) Business Days and pays the undisputed part in accordance with clause ")
           + ref(payment_ref, "4.2") + ox.run("."))

    confidentiality_ref = "_Ref217004501"
    clause(0, ox.bookmark(next(bookmarks), confidentiality_ref, ox.run("Confidentiality")))
    secret = clause(1, ox.run("Each party shall keep the other party's ") + ox.run("Confidential Information")
                    + ox.run(" confidential for the term of this Agreement and for three (3) years after its end."))
    clause(1, ox.run("A party may disclose Confidential Information to its employees, advisers and auditors who need "
                     "to know it, provided that they are bound by obligations of confidentiality no less strict than "
                     "those in this clause."))
    clause(1, ox.run("This clause does not apply to information that is or becomes public other than through a breach "
                     "of this Agreement, or that a party must disclose by law."))

    liability_ref = "_Ref217004601"
    cap_ref = "_Ref217004611"
    clause(0, ox.bookmark(next(bookmarks), liability_ref, ox.run("Liability")))
    clause(1, ox.bookmark(next(bookmarks), cap_ref,
                          ox.run("Nothing in this Agreement limits either party's liability for death or personal "
                                 "injury caused by its negligence, or for fraud.")))
    cap = clause(
        1,
        ox.run("Subject to clause ") + ref(cap_ref, "6.1") + ox.run(", each party's total liability under this "
                                                                    "Agreement in each contract year ")
        + ox.run("shall not exceed", "<w:b/>" + ox.format_change(next(revisions), customer, LATER))
        + ox.run(" the fees paid or payable in the twelve (12) months before the event giving rise to the claim."),
    )
    clause(1, ox.run("Neither party is liable for loss of profit, loss of data or any indirect or consequential "
                     "loss, even if that loss was foreseeable."))

    clause(0, ox.run("Termination"))
    clause(
        1,
        ox.run("Either party may terminate this Agreement with immediate effect by written notice if the other party "
               "commits a material breach which it fails to remedy within thirty (30) days of being asked to do so.")
        + ox.deleted(next(revisions), customer, LATER, " The Supplier may also terminate on six months' notice for "
                                                       "convenience."),
    )
    clause(1, ox.run("Clauses ") + ref(confidentiality_ref, "5") + ox.run(" and ") + ref(liability_ref, "6")
           + ox.run(" survive termination or expiry of this Agreement, together with any other clause which by its "
                    "nature is intended to survive."))
    clause(1, ox.run("On termination, the Supplier shall return or destroy the Customer's data within thirty (30) "
                     "days, as the Customer directs, and confirm in writing that it has done so."))

    clause(0, ox.run("General"))
    clause(1, ox.run("Notices under this Agreement shall be in writing and sent to the addresses set out above, or by "
                     "email to ") + ox.hyperlink(doc.part, "mailto:legal@northwind.example", "legal@northwind.example")
           + ox.run(" for the Supplier."))
    clause(1, ox.run("This Agreement constitutes the entire agreement between the parties and supersedes any previous "
                     "arrangement relating to its subject matter."))
    law = clause(1, ox.run("This Agreement and any dispute arising from it are governed by the laws of England and "
                           "Wales, and the courts of England have exclusive jurisdiction."))
    ox.add_note(doc, law, "footnote", "The parties considered French law and chose English law for consistency with "
                                      "the Customer's other supplier agreements.")

    doc.add_paragraph()
    doc.add_paragraph("Signed by the authorised representatives of the parties:")
    table = doc.add_table(rows=4, cols=2)
    table.style = "Table Grid"
    for r, (left, right) in enumerate((
        ("For Northwind Analytics SAS", "For Contoso Logistics Ltd"),
        ("Name: Marie Laurent", "Name: Oliver Grant"),
        ("Title: Managing Director", "Title: Chief Operating Officer"),
        ("Signature:", "Signature:"),
    )):
        table.cell(r, 0).text, table.cell(r, 1).text = left, right

    parents = {}

    def comment(runs, text: str, author: str, date: str):
        initials = "".join(part[0] for part in author.split())
        made = doc.add_comment(runs, text=text, author=author, initials=initials)
        made._comment_elm.set(qn("w:date"), date)
        return made.comment_id

    target = [secret.runs[1]]
    first = comment(target, "Should trade secrets stay confidential for longer than three years?", customer, DATE)
    parents[comment(target, "Agreed for trade secrets: five years. Three years for everything else.", supplier,
                    LATER)] = first
    parents[comment(target, "Fine with that split, please update the draft.", customer, LATER)] = first
    notice = comment(renewal.runs[1:2], "120 days is longer than our standard. Can we meet at 90?", supplier, LATER)
    parents[comment(renewal.runs[1:2], "We need the extra month for procurement approvals.", customer, LATER)] = notice
    # Stretch that thread over the tracked change it discusses.
    first_change = renewal._p.find(qn("w:del"))
    for start in renewal._p.findall(qn("w:commentRangeStart")):
        first_change.addprevious(start)
    resolved = comment(law.runs[:1], "Check with French counsel whether a French-law version is needed.", supplier,
                       DATE)
    bold = [r for r in cap.runs if r.text == "shall not exceed"]
    comment(bold, "Bold for emphasis, as agreed on the call.", customer, LATER)
    ox.link_comments(doc, parents, done=frozenset({resolved}))

    _properties(doc, "Master Services Agreement", supplier, "Northwind / Contoso services agreement")
    doc.save(path)


# Thesis

_THESIS_TITLE = "Adaptive Inventory Control for Multi-Echelon Warehouse Networks"
_UNNUMBERED = '<w:numPr><w:ilvl w:val="0"/><w:numId w:val="0"/></w:numPr>'


@family("thesis", "docx")
def thesis(path: Path) -> None:
    """A master's thesis: title page, roman then arabic page numbers, TOC and list of figures,
    numbered headings, SEQ captions, notes, equations, odd and even headers, a landscape section."""
    doc = _new("Cambria", 11, 2.5)
    first = doc.sections[0]
    first.page_width, first.page_height = Cm(21.0), Cm(29.7)
    first.top_margin = first.bottom_margin = Cm(2.5)
    width = 9070  # text width in twips, for the right-aligned tab stops
    ox.add_styles(doc, "TOC1", "TOC2", "TOC3", "TableofFigures", "Hyperlink")
    doc.styles["Normal"].paragraph_format.space_after = Pt(6)
    doc.styles["Normal"].paragraph_format.line_spacing = 1.15
    doc.styles["Heading 1"].paragraph_format.page_break_before = True
    doc.settings.odd_and_even_pages_header_footer = True
    headings = ox.add_numbering(
        doc, [("decimal", "%1", 432, 432), ("decimal", "%1.%2", 576, 576), ("decimal", "%1.%2.%3", 720, 720)],
        "5D2B7C01", styles=("Heading1", "Heading2", "Heading3"),
    )
    for level in range(3):
        ox.style_numbering(doc, f"Heading{level + 1}", headings, level)
    bookmarks, seeds = count(1), count(300)
    outline, figures = [], []
    counters = [0, 0, 0]
    body_paragraphs = [0]

    def heading(level: int, title: str) -> None:
        counters[level - 1] += 1
        counters[level:] = [0] * (3 - level)
        number = ".".join(str(c) for c in counters[:level])
        name = f"_Toc2170{len(outline) + 1:05d}"
        _paragraph(doc, ox.para(ox.bookmark(next(bookmarks), name, ox.run(title)), style=f"Heading{level}"))
        outline.append((level, number, title, name, 1 + body_paragraphs[0] // 5))

    def unnumbered(title: str) -> None:
        _paragraph(doc, ox.para(ox.run(title), _UNNUMBERED, style="Heading1"))

    def text(n: int = 7, lead: str = "") -> Paragraph:
        body_paragraphs[0] += 1
        content = (lead + " " if lead else "") + paragraph_text(next(seeds), n)
        p = doc.add_paragraph(content)
        p.alignment = WD_ALIGN_PARAGRAPH.JUSTIFY
        return p

    def caption(label: str, title: str) -> str:
        number = 1 + sum(1 for f in figures if f[0] == label)
        toc_name, ref_name = f"_Toc2171{len(figures) + 1:05d}", f"_Ref2171{len(figures) + 1:05d}"
        label_xml = ox.bookmark(next(bookmarks), ref_name, ox.run(f"{label} ") + ox.field(f"SEQ {label} \\* ARABIC", str(number)))
        _paragraph(doc, ox.para(ox.bookmark(next(bookmarks), toc_name, label_xml + ox.run(f": {title}")), style="Caption"))
        figures.append((label, number, title, toc_name, 1 + body_paragraphs[0] // 5))
        return ref_name

    def equation(*parts: str) -> None:
        _paragraph(doc, ox.para(ox.omath_para(*parts)))

    # Front matter: title page, abstract, acknowledgements, contents, list of figures.
    for text_line, size, bold in (
        ("University of Westmere", 14, True),
        ("Department of Industrial Engineering", 12, False),
        ("", 12, False),
        (_THESIS_TITLE, 24, True),
        ("Policies that learn from demand signals across regional and central stock points", 14, False),
        ("", 12, False),
        ("A thesis submitted in partial fulfilment of the requirements for the degree of", 12, False),
        ("Master of Science in Operations Research", 12, True),
        ("", 12, False),
        ("Sophie Laurent", 14, True),
        ("Supervised by Prof. Daniel Okafor and Dr. Hannah Weiss", 12, False),
        ("November 2026", 12, False),
    ):
        p = doc.add_paragraph()
        p.alignment = WD_ALIGN_PARAGRAPH.CENTER
        p.paragraph_format.space_after = Pt(18)
        if text_line:
            r = p.add_run(text_line)
            r.font.size, r.bold = Pt(size), bold

    unnumbered("Abstract")
    text(7, "Warehouse networks hold stock at several levels, and the safety stock of each level depends on the "
            "replenishment decisions of the others.")
    text(6, "This thesis proposes an adaptive base-stock policy that updates its targets from observed demand and "
            "lead times, and evaluates it on a simulated network of one central and six regional warehouses.")
    text(5, "Across twelve scenarios, the adaptive policy reduces the mean daily cost by 9 to 17 % against a static "
            "policy tuned on historical data, at an equal fill rate.")
    unnumbered("Acknowledgements")
    text(4, "I thank my supervisors, Prof. Daniel Okafor and Dr. Hannah Weiss, for their patience and their careful "
            "reading of every draft.")
    text(4, "The data of Chapter 4 were provided by the logistics team of a regional distributor, who asked to remain "
            "anonymous.")
    toc_slot = _paragraph(doc, ox.para())
    lof_slot = _paragraph(doc, ox.para())

    # Main matter, arabic page numbers from 1, chapters on odd pages.
    main = doc.add_section(WD_SECTION.ODD_PAGE)
    main.different_first_page_header_footer = False
    ox.page_numbers(main, "decimal", 1)
    body_paragraphs[0] = 0

    heading(1, "Introduction")
    heading(2, "Motivation")
    text(6, "Regional distributors keep stock close to their customers to deliver within a day, and replenish that "
            "stock from a central warehouse.")
    note = text(5)
    ox.add_note(doc, note, "footnote", "Same-day and next-day delivery represented 64 % of the orders of the "
                                       "distributor studied in Chapter 4.")
    for _ in range(3):
        text()
    heading(2, "Research questions")
    text(4, "The thesis answers three questions:")
    for question in (
        "How much safety stock does a static base-stock policy hold in excess when demand drifts?",
        "Can a policy that re-estimates demand and lead time every week keep the target fill rate?",
        "What does the adaptive policy cost to run, in data and in computation?",
    ):
        doc.add_paragraph(question, style="List Number")
    heading(2, "Contributions and structure")
    for item in (
        "an adaptive base-stock policy for two-echelon networks with a closed-form update rule;",
        "a simulation study on twelve demand scenarios built from real order data;",
        "an open implementation of the simulator and of the policies.",
    ):
        doc.add_paragraph(item, style="List Bullet")
    for _ in range(3):
        text()

    heading(1, "Literature review")
    text(6, "Inventory theory distinguishes single-location models from multi-echelon models, where the stock of "
            "one location is the supply of another.")
    heading(2, "Inventory models")
    heading(3, "Single-echelon models")
    text(5, "The economic order quantity balances the fixed cost of ordering against the cost of holding stock:")
    equation(ox.m_sup(ox.m_r("Q"), ox.m_r("*")), ox.m_r("="), ox.m_rad(ox.m_frac(ox.m_r("2DK"), ox.m_r("h"))))
    p = text(4, "Here D is the annual demand, K the fixed cost of an order and h the holding cost per unit and year.")
    ox.add_note(doc, p, "endnote", "Harris, F. W. (1913). How many parts to make at once. Factory, The Magazine of "
                                   "Management, 10(2), 135-136.")
    for _ in range(3):
        text()
    heading(3, "Multi-echelon models")
    text(6, "Clark and Scarf showed that the echelon stock of a serial system can be managed with one base-stock "
            "level per stage.")
    p = text(5)
    ox.add_note(doc, p, "endnote", "Clark, A. J. and Scarf, H. (1960). Optimal policies for a multi-echelon inventory "
                                   "problem. Management Science, 6(4), 475-490.")
    for _ in range(4):
        text()
    heading(2, "Demand forecasting")
    for _ in range(4):
        text()
    p = text(4, "Exponential smoothing remains the default in practice, because it needs little data and little "
                "tuning.")
    ox.add_note(doc, p, "footnote", "Of the 31 companies surveyed by the European Logistics Association in 2024, 22 "
                                    "used a form of exponential smoothing.")
    heading(2, "Research gap")
    for _ in range(4):
        text()
    heading(2, "Summary")
    for _ in range(3):
        text()

    heading(1, "Model and policies")
    heading(2, "Notation")
    text(4, "Table 1 lists the notation used in the rest of the thesis.")
    caption("Table", "Notation")
    notation = doc.add_table(rows=1, cols=2)
    notation.style = "Light List Accent 1"
    notation.rows[0].cells[0].text, notation.rows[0].cells[1].text = "Symbol", "Meaning"
    for symbol, meaning in (
        ("N", "number of regional warehouses"), ("L_i", "replenishment lead time of warehouse i, in days"),
        ("mu_i, sigma_i", "mean and standard deviation of the daily demand at warehouse i"),
        ("h_i, b_i", "holding and backorder cost per unit and day at warehouse i"),
        ("S_i", "base-stock level of warehouse i"), ("z", "safety factor for the target fill rate"),
    ):
        cells = notation.add_row().cells
        cells[0].text, cells[1].text = symbol, meaning
    heading(2, "Cost function")
    text(4, "The expected cost per day of the network sums the holding and backorder costs of every warehouse:")
    equation(
        ox.m_r("C"), ox.m_r("="),
        ox.m_nary("∑", ox.m_r("i=1"), ox.m_r("N"), ox.m_delim(
            ox.m_sub(ox.m_r("h"), ox.m_r("i")) + ox.m_sub(ox.m_r("I"), ox.m_r("i")) + ox.m_r("+")
            + ox.m_sub(ox.m_r("b"), ox.m_r("i")) + ox.m_sub(ox.m_r("B"), ox.m_r("i")))),
    )
    p = doc.add_paragraph("where ")
    ox.append(p._p, ox.omath(ox.m_sub(ox.m_r("I"), ox.m_r("i"))) + ox.run(" is the expected on-hand stock and ")
              + ox.omath(ox.m_sub(ox.m_r("B"), ox.m_r("i"))) + ox.run(" the expected backorders of warehouse ")
              + ox.omath(ox.m_r("i")) + ox.run("."))
    for _ in range(3):
        text()
    heading(2, "Adaptive base-stock policy")
    text(5, "Every week, the policy re-estimates the mean and the variance of demand and of lead time, and sets the "
            "base-stock level to")
    equation(
        ox.m_sub(ox.m_r("S"), ox.m_r("i")), ox.m_r("="),
        ox.m_sub(ox.m_r("μ"), ox.m_r("i")), ox.m_sub(ox.m_r("L"), ox.m_r("i")), ox.m_r("+z"),
        ox.m_sub(ox.m_r("σ"), ox.m_r("i")), ox.m_rad(ox.m_sub(ox.m_r("L"), ox.m_r("i"))),
    )
    for _ in range(3):
        text()
    figure_ref = None
    doc.add_picture(chart_png((38, 52, 47, 61, 58, 66)), width=Cm(13))
    doc.paragraphs[-1].alignment = WD_ALIGN_PARAGRAPH.CENTER
    figure_ref = caption("Figure", "Weekly demand at the central warehouse, indexed")
    p = doc.add_paragraph("As ")
    ox.append(p._p, ox.field(f"REF {figure_ref} \\h", "Figure 1") + ox.run(" shows, demand drifts upward over the "
                                                                          "year, with a seasonal peak in week 40."))
    for _ in range(3):
        text()

    # Results on a landscape page, for the wide table.
    wide = doc.add_section(WD_SECTION.NEW_PAGE)
    wide.orientation = WD_ORIENT.LANDSCAPE
    wide.page_width, wide.page_height = Cm(29.7), Cm(21.0)
    ox.page_numbers(wide, "decimal")
    heading(1, "Results")
    heading(2, "Experimental setup")
    text(5, "Each scenario simulates 730 days after a warm-up of 90 days, with 30 replications.")
    caption("Table", "Results by scenario and policy, mean of 30 replications")
    columns = ("Scenario", "Policy", "Nodes", "Lead time (d)", "Demand CV", "Fill rate (%)", "Mean stock (units)",
               "Cost per day (EUR)", "Gap to bound (%)")
    results = doc.add_table(rows=1, cols=len(columns))
    results.style = "Light Grid Accent 1"
    for c, name in enumerate(columns):
        results.rows[0].cells[c].text = name
    for scenario in range(1, 7):
        for policy in ("Static", "Adaptive"):
            adaptive = policy == "Adaptive"
            stock = 1800 + 140 * scenario - (260 + 20 * scenario if adaptive else 0)
            cost = 912 + 61 * scenario - (95 + 9 * scenario if adaptive else 0)
            cells = results.add_row().cells
            for c, value in enumerate((
                f"S{scenario}", policy, str(4 + scenario % 3), str(3 + scenario % 4), f"{0.2 + 0.05 * scenario:.2f}",
                f"{95.0 + (0.4 if adaptive else 0.1) * (scenario % 3):.1f}", f"{stock:,}", f"{cost:,}",
                f"{(2.1 if adaptive else 11.4) + 0.3 * scenario:.1f}",
            )):
                cells[c].text = value
    text(4, "The adaptive policy holds less stock in every scenario, and the gap is widest when demand varies most.")

    narrow = doc.add_section(WD_SECTION.NEW_PAGE)
    narrow.orientation = WD_ORIENT.PORTRAIT
    narrow.page_width, narrow.page_height = Cm(21.0), Cm(29.7)
    ox.page_numbers(narrow, "decimal")
    heading(2, "Discussion")
    for _ in range(4):
        text()
    p = text(4, "The gain shrinks when lead times are short, since the safety stock is then small for both policies.")
    ox.add_note(doc, p, "footnote", "With lead times under two days, the difference is within the confidence "
                                    "interval of the simulation.")
    heading(2, "Threats to validity")
    for _ in range(3):
        text()
    heading(3, "Internal validity")
    for _ in range(3):
        text()
    heading(3, "External validity")
    for _ in range(3):
        text()

    heading(1, "Conclusion")
    text(5, "An adaptive base-stock policy keeps the target fill rate with less stock than a static policy, at a "
            "modest cost in data.")
    for _ in range(3):
        text()
    heading(2, "Future work")
    for _ in range(2):
        text()
    p = text(3)
    ox.add_note(doc, p, "endnote", "Axsater, S. (2015). Inventory Control, third edition. Springer, New York.")

    unnumbered("References")
    for reference in (
        "Axsater, S. (2015). Inventory Control (3rd ed.). Springer.",
        ("Clark, A. J. and Scarf, H. (1960). Optimal policies for a multi-echelon inventory problem. Management "
        "Science, 6(4), 475-490."),
        ("Graves, S. C. and Willems, S. P. (2000). Optimizing strategic safety stock placement in supply chains. "
        "Manufacturing and Service Operations Management, 2(1), 68-83."),
        "Harris, F. W. (1913). How many parts to make at once. Factory, The Magazine of Management, 10(2), 135-136.",
        "Hyndman, R. J. and Athanasopoulos, G. (2021). Forecasting: Principles and Practice (3rd ed.). OTexts.",
    ):
        p = doc.add_paragraph(reference)
        p.paragraph_format.left_indent, p.paragraph_format.first_line_indent = Cm(1), Cm(-1)
    unnumbered("Appendix A. Simulation parameters")
    params = doc.add_table(rows=1, cols=3)
    params.style = "Table Grid"
    for c, name in enumerate(("Parameter", "Value", "Source")):
        params.rows[0].cells[c].text = name
    for row in (("Warm-up", "90 days", "Pilot runs"), ("Horizon", "730 days", "Two seasons"),
                ("Replications", "30", "Confidence interval under 1 %"), ("Review period", "7 days", "Current practice")):
        cells = params.add_row().cells
        for c, value in enumerate(row):
            cells[c].text = value

    # Headers and footers: blank title page, odd and even pages, chapter name in odd headers.
    def page_footer(footer, align) -> None:
        p = footer.paragraphs[0]
        p.alignment = align
        ox.append(p._p, ox.field("PAGE", "1"))

    # add_section returns the last section's sectPr, which moves: fetch them again.
    first, main = doc.sections[0], doc.sections[1]
    first.different_first_page_header_footer = True
    first.first_page_header.paragraphs[0].text = ""
    first.first_page_footer.paragraphs[0].text = ""
    first.header.paragraphs[0].text = _THESIS_TITLE
    first.header.paragraphs[0].alignment = WD_ALIGN_PARAGRAPH.RIGHT
    first.even_page_header.paragraphs[0].text = "Sophie Laurent - MSc thesis"
    page_footer(first.footer, WD_ALIGN_PARAGRAPH.RIGHT)
    page_footer(first.even_page_footer, WD_ALIGN_PARAGRAPH.LEFT)
    main.header.is_linked_to_previous = False
    chapter = main.header.paragraphs[0]
    chapter.alignment = WD_ALIGN_PARAGRAPH.RIGHT
    ox.append(chapter._p, ox.field('STYLEREF "Heading 1" \\n', "1") + ox.run(" ")
              + ox.field('STYLEREF "Heading 1"', "Introduction"))
    main.even_page_header.is_linked_to_previous = False
    main.even_page_header.paragraphs[0].text = _THESIS_TITLE
    main.even_page_header.paragraphs[0].runs[0].italic = True

    # Contents and list of figures, now that the headings and captions are known.
    def entry(style: str, label: str, title: str, anchor: str, page: int, opening: str = "") -> str:
        number_tab = {"TOC1": 440, "TOC2": 880, "TOC3": 1320}.get(style)
        tabs = f'<w:tab w:val="left" w:pos="{number_tab}"/>' if number_tab else ""
        tabs = f'<w:tabs>{tabs}<w:tab w:val="right" w:leader="dot" w:pos="{width}"/></w:tabs>'
        inner = (ox.run(label) + ox.tab() if label else "") + ox.run(title) + ox.tab()
        inner += ox.field(f"PAGEREF {anchor} \\h", str(page))
        return ox.para(opening + ox.anchor_link(anchor, inner=inner), tabs, style=style)

    toc = ox.para(ox.run("Contents"), _UNNUMBERED, style="Heading1")
    for i, (level, number, title, anchor, page) in enumerate(outline):
        opening = ox.field_start('TOC \\o "1-3" \\h \\z \\u') if i == 0 else ""
        toc += entry(f"TOC{level}", number, title, anchor, page, opening)
    toc += ox.para(ox.field_end())
    gallery = '<w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj>'
    toc_slot._p.addprevious(ox.parse(ox.sdt(toc, 1840220931, kind=gallery))[0])
    toc_slot._p.getparent().remove(toc_slot._p)
    lof = ox.para(ox.run("List of Figures and Tables"), _UNNUMBERED, style="Heading1")
    for i, (label, number, title, anchor, page) in enumerate(figures):
        opening = ox.field_start('TOC \\h \\z \\t "Caption,1"') if i == 0 else ""
        lof += entry("TableofFigures", "", f"{label} {number}: {title}", anchor, page, opening)
    lof += ox.para(ox.field_end())
    for element in ox.parse(lof):
        lof_slot._p.addprevious(element)
    lof_slot._p.getparent().remove(lof_slot._p)

    ox.page_numbers(first, "lowerRoman", 1)
    _properties(doc, _THESIS_TITLE, "Sophie Laurent", "MSc thesis in operations research")
    doc.save(path)


# Letter


@family("letter", "docx")
def letter(path: Path) -> None:
    """A mail-merge business letter: letterhead picture, DATE and MERGEFIELD fields,
    external links, a right-aligned date and dot-leader tab stops."""
    doc = _new("Calibri", 11, 2.5)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    ox.add_styles(doc, "Hyperlink")
    right = Cm(16.0)

    section.different_first_page_header_footer = True
    head = section.first_page_header.paragraphs[0]
    head.add_run().add_picture(logo_png("NW"), width=Cm(2.0))
    head.paragraph_format.tab_stops.add_tab_stop(right, WD_TAB_ALIGNMENT.RIGHT)
    head.add_run("\tNorthwind Analytics SAS").bold = True
    address = section.first_page_header.add_paragraph("\t14 rue des Halles, 69002 Lyon, France  |  +33 4 72 00 00 00")
    address.paragraph_format.tab_stops.add_tab_stop(right, WD_TAB_ALIGNMENT.RIGHT)
    address.runs[0].font.size = Pt(9)
    later = section.header.paragraphs[0]
    later.paragraph_format.tab_stops.add_tab_stop(right, WD_TAB_ALIGNMENT.RIGHT)
    ox.append(later._p, ox.run("Northwind Analytics SAS") + ox.tab() + ox.run("Page ") + ox.field("PAGE", "2"))
    for footer in (section.first_page_footer, section.footer):
        legal = footer.paragraphs[0]
        legal.alignment = WD_ALIGN_PARAGRAPH.CENTER
        legal.text = "Northwind Analytics SAS, capital of EUR 250,000, RCS Lyon 812 345 678, VAT FR12 812345678"
        legal.runs[0].font.size = Pt(8)

    def merge(name: str) -> str:
        return ox.field(f"MERGEFIELD {name} \\* MERGEFORMAT", f"«{name}»")

    reference = doc.add_paragraph()
    reference.paragraph_format.tab_stops.add_tab_stop(right, WD_TAB_ALIGNMENT.RIGHT)
    ox.append(reference._p, ox.run("Our reference: NW/SALES/2026-114") + ox.tab() + ox.run("Lyon, ")
              + ox.field('DATE \\@ "d MMMM yyyy"', "21 November 2026"))
    doc.add_paragraph()
    for line in (
        merge("Title") + ox.run(" ") + merge("FirstName") + ox.run(" ") + merge("LastName"),
        merge("JobTitle"),
        merge("Company"),
        merge("Address1"),
        merge("PostalCode") + ox.run(" ") + merge("City"),
        merge("Country"),
    ):
        p = _paragraph(doc, ox.para(line))
        p.paragraph_format.space_after = Pt(0)
    doc.add_paragraph()
    subject = doc.add_paragraph()
    subject.add_run("Subject: renewal of your analytics subscription for 2027").bold = True
    _paragraph(doc, ox.para(ox.run("Dear ") + merge("Title") + ox.run(" ") + merge("LastName") + ox.run(",")))
    doc.add_paragraph(
        "Thank you for your trust over the past three years. Your subscription to Northwind Insights ends on "
        "31 December 2026, and we are pleased to offer you a renewal on the terms below, which include the new "
        "warehouse forecasting module released in September."
    )
    doc.add_paragraph(
        "Your account team reviewed your usage over the last twelve months: your users ran 4,800 reports and the "
        "forecasting pilot covered six of your sites. The offer reflects that usage and keeps your current price "
        "for the core platform."
    )
    for item, amount in (
        ("Core platform, 40 named users", "EUR 36,000"),
        ("Warehouse forecasting module, 6 sites", "EUR 9,600"),
        ("Premium support, 8 hours a day, 5 days a week", "EUR 4,800"),
        ("Loyalty discount, 5 %", "- EUR 2,520"),
        ("Total per year, excluding VAT", "EUR 47,880"),
    ):
        line = doc.add_paragraph()
        line.paragraph_format.left_indent = Cm(1)
        line.paragraph_format.space_after = Pt(2)
        line.paragraph_format.tab_stops.add_tab_stop(right, WD_TAB_ALIGNMENT.RIGHT, WD_TAB_LEADER.DOTS)
        run = line.add_run(f"{item}\t{amount}")
        run.bold = item.startswith("Total")
    doc.add_paragraph()
    p = doc.add_paragraph("The full price schedule and the service levels are available on ")
    ox.append(p._p, ox.hyperlink(doc.part, "https://www.northwind.example/pricing/2027", "our website")
              + ox.run(", and you can accept the offer directly in the ")
              + ox.hyperlink(doc.part, "https://portal.northwind.example/renewals?ref=NW-2026-114", "customer portal")
              + ox.run(" before 15 December 2026."))
    p = doc.add_paragraph("If you have any question, please contact your account manager, Julien Moreau, at ")
    ox.append(p._p, ox.hyperlink(doc.part, "mailto:julien.moreau@northwind.example", "julien.moreau@northwind.example")
              + ox.run(" or on +33 4 72 00 00 14."))
    for text in (
        ("The renewal also includes two training sessions for your new users, which we can run on site in Leeds or "
        "remotely, and an invitation to our customer day in Lyon on 12 March 2027."),
        ("As announced in our last newsletter, the forecasting module now covers transport costs as well as stock "
        "levels. Your account manager will be glad to show you the results of the pilot on your own data before you "
        "decide."),
        ("Should you prefer to keep your current scope, we will renew it at the same price without any action on "
        "your side. The offer above remains valid until 15 December 2026."),
    ):
        doc.add_paragraph(text)
    doc.add_paragraph("Yours sincerely,")
    doc.add_paragraph()
    doc.add_paragraph()
    signature = doc.add_paragraph()
    signature.add_run("Marie Laurent").bold = True
    signature.add_run("\nManaging Director, Northwind Analytics SAS")
    doc.add_paragraph()
    enclosures = doc.add_paragraph()
    enclosures.paragraph_format.tab_stops.add_tab_stop(Cm(3), WD_TAB_ALIGNMENT.LEFT)
    enclosures.add_run("Enclosures:\tPrice schedule 2027\n\tService level agreement, version 4.2")

    _properties(doc, "Subscription renewal offer 2027", "Marie Laurent", "Renewal letter, mail merge main document")
    doc.save(path)


# CV


@family("cv", "docx")
def cv(path: Path) -> None:
    """A two-column CV: borderless layout table with a shaded sidebar, a nested skills table,
    links, small caps headings, dot-leader tabs and bullet lists."""
    doc = _new("Calibri", 10, 1.5)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    section.top_margin = section.bottom_margin = Cm(1.5)
    ox.add_styles(doc, "Hyperlink")

    name = doc.add_paragraph()
    run = name.add_run("Elena Marchetti")
    run.font.size, run.bold = Pt(26), True
    role = doc.add_paragraph()
    run = role.add_run("Senior Data Engineer")
    run.font.size, run.font.small_caps = Pt(14), True
    run.font.color.rgb = RGBColor(0x0F, 0x76, 0x6E)

    layout = doc.add_table(rows=1, cols=2)
    layout.autofit = False
    ox.table_borders(layout, "none")
    side, main = layout.rows[0].cells
    widths = (Cm(5.6), Cm(12.4))
    for cell, width in zip((side, main), widths):
        cell.width = width
    for column, width in zip(layout.columns, widths):
        column.width = width
    ox.cell_shading(side, "E6F4F1")

    def title(cell, text: str, first: bool = False) -> None:
        p = cell.paragraphs[0] if first else cell.add_paragraph()
        p.paragraph_format.space_before = Pt(0 if first else 10)
        run = p.add_run(text)
        run.bold, run.font.small_caps, run.font.size = True, True, Pt(12)
        run.font.color.rgb = RGBColor(0x0F, 0x76, 0x6E)

    title(side, "Contact", first=True)
    side.add_paragraph("Turin, Italy (open to relocation)")
    side.add_paragraph("+39 011 555 0142")
    for label, url in (
        ("elena.marchetti@example.com", "mailto:elena.marchetti@example.com"),
        ("linkedin.com/in/elena-marchetti", "https://www.linkedin.com/in/elena-marchetti"),
        ("github.com/emarchetti", "https://github.com/emarchetti"),
    ):
        ox.append(side.add_paragraph()._p, ox.hyperlink(doc.part, url, label))
    title(side, "Skills")
    skills = side.add_table(rows=0, cols=2)
    ox.table_borders(skills, "single", 4, "9CA3AF")
    for skill, level in (("Python", "Expert"), ("SQL", "Expert"), ("Spark", "Advanced"), ("Kafka", "Advanced"),
                         ("Terraform", "Intermediate"), ("Rust", "Intermediate")):
        cells = skills.add_row().cells
        cells[0].text, cells[1].text = skill, level
    side.add_paragraph()
    title(side, "Languages")
    for language, level in (("Italian", "native"), ("English", "C2"), ("French", "B2"), ("German", "A2")):
        p = side.add_paragraph()
        p.paragraph_format.tab_stops.add_tab_stop(Cm(5.2), WD_TAB_ALIGNMENT.RIGHT, WD_TAB_LEADER.DOTS)
        p.add_run(f"{language}\t{level}")
    title(side, "Certifications")
    for item in ("Google Professional Data Engineer, 2024", "AWS Solutions Architect Associate, 2022"):
        side.add_paragraph(item, style="List Bullet")

    title(main, "Profile", first=True)
    main.add_paragraph(
        "Data engineer with nine years of experience building batch and streaming pipelines for logistics and "
        "retail companies. I design platforms that analysts trust: tested, documented and cheap to run."
    )
    title(main, "Experience")

    def job(role: str, period: str, bullets: list[str]) -> None:
        p = main.add_paragraph()
        p.paragraph_format.space_before = Pt(6)
        p.paragraph_format.tab_stops.add_tab_stop(Cm(12.0), WD_TAB_ALIGNMENT.RIGHT, WD_TAB_LEADER.DOTS)
        p.add_run(role).bold = True
        p.add_run(f"\t{period}")
        for bullet in bullets:
            main.add_paragraph(bullet, style="List Bullet")

    job("Lead Data Engineer, Northwind Analytics, Lyon", "2022 - present", [
        "Led a team of five engineers that moved 140 pipelines from nightly batches to Kafka streams.",
        "Cut the monthly cloud bill of the data platform by 38 % with partitioning and spot instances.",
        "Introduced data contracts and tests; incidents caused by schema changes fell from 9 to 1 per quarter.",
    ])
    job("Data Engineer, Contoso Retail, Milan", "2019 - 2022", [
        "Built the demand forecasting feature store used by 30 analysts and four models in production.",
        "Wrote the internal guide to Spark performance, now part of the onboarding programme.",
    ])
    job("BI Developer, Fabrikam Logistics, Turin", "2017 - 2019", [
        "Designed the warehouse KPI dashboards used daily by 12 sites.",
        "Automated the monthly reporting pack, saving three days of manual work each month.",
    ])
    title(main, "Education")
    for degree, period in (("MSc Computer Engineering, Politecnico di Torino", "2015 - 2017"),
                           ("BSc Computer Engineering, Politecnico di Torino", "2012 - 2015")):
        p = main.add_paragraph()
        p.paragraph_format.tab_stops.add_tab_stop(Cm(12.0), WD_TAB_ALIGNMENT.RIGHT, WD_TAB_LEADER.DOTS)
        p.add_run(degree).bold = True
        p.add_run(f"\t{period}")
    title(main, "Selected talks")
    p = main.add_paragraph("Streaming without tears, PyCon Italia 2025 (")
    ox.append(p._p, ox.hyperlink(doc.part, "https://www.youtube.com/watch?v=example-talk", "video") + ox.run(")"))
    main.add_paragraph("Data contracts in practice, Data Council Berlin 2024")

    footer = section.footer.paragraphs[0]
    footer.alignment = WD_ALIGN_PARAGRAPH.CENTER
    footer.add_run("References available on request").font.small_caps = True
    _properties(doc, "Elena Marchetti - CV", "Elena Marchetti", "Curriculum vitae")
    doc.save(path)


# Form


@family("form", "docx")
def form(path: Path) -> None:
    """A supplier registration form: content controls of every common kind, legacy form
    fields, and editing restricted to filling in forms."""
    doc = _new("Arial", 10, 2.0)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    ids, bookmarks = count(501238001), count(1)

    doc.add_heading("Supplier Registration Form", level=0)
    intro = doc.add_paragraph(
        "Please complete every section and return the form to procurement@northwind.example. Fields in grey are "
        "filled by the procurement office. This form takes about ten minutes to complete."
    )
    intro.runs[0].italic = True

    def form_table(rows: list[tuple[str, str]]):
        table = doc.add_table(rows=0, cols=2)
        table.style = "Table Grid"
        table.autofit = False
        for label, control in rows:
            cells = table.add_row().cells
            cells[0].width, cells[1].width = Cm(6), Cm(11)
            cells[0].text = label
            ox.cell_shading(cells[0], "F1F5F9")
            if control.startswith("<w:sdt><w:sdtPr><w:alias w:val=\"Registered address\""):
                p = cells[1].paragraphs[0]._p
                p.addprevious(ox.parse(control)[0])
                p.getparent().remove(p)
            else:
                ox.append(cells[1].paragraphs[0]._p, control)
        return table

    doc.add_heading("Part A. Company details", level=1)
    address = ox.sdt(
        ox.para(ox.run("2 Canal Wharf")) + ox.para(ox.run("Leeds LS11 5PS")) + ox.para(ox.run("United Kingdom")),
        next(ids), "Registered address",
    )
    form_table([
        ("Company name", ox.sdt_text(next(ids), "Company name", "Contoso Logistics Ltd")),
        ("Registered address", address),
        ("Date of incorporation", ox.sdt_date(next(ids), "Date of incorporation", "2014-03-17", "17 March 2014")),
        ("Company type", ox.sdt_list(next(ids), "Company type",
                                     ["Limited company", "Partnership", "Sole trader", "Public body"], 0)),
        ("Country of registration", ox.sdt_list(next(ids), "Country",
                                                ["France", "Germany", "Italy", "Spain", "United Kingdom"], 4, combo=True)),
        ("Registered for VAT", ox.sdt_checkbox(next(ids), "VAT registered", True) + ox.run(" Yes")),
        ("ISO 9001 certified", ox.sdt_checkbox(next(ids), "ISO 9001", False) + ox.run(" Yes")),
        ("Contact person", ox.sdt_text(next(ids), "Contact person", "Oliver Grant, Chief Operating Officer")),
    ])

    doc.add_heading("Part B. Bank details", level=1)
    doc.add_paragraph("These fields use legacy form fields, as required by the finance system that reads them.")
    form_table([
        ("Bank name", ox.form_text(next(bookmarks), "BankName", "Barclays Bank UK PLC", 60)),
        ("IBAN", ox.form_text(next(bookmarks), "IBAN", "GB33 BUKB 2020 1555 5555 55", 34)),
        ("BIC", ox.form_text(next(bookmarks), "BIC", "BUKBGB22", 11)),
        ("Payment terms", ox.form_dropdown(next(bookmarks), "PaymentTerms", ["30 days", "45 days", "60 days"], 0)),
        ("Remittance advice by email", ox.form_checkbox(next(bookmarks), "RemittanceEmail", True) + ox.run(" Yes")),
    ])

    doc.add_heading("Part C. Declaration", level=1)
    p = doc.add_paragraph()
    ox.append(p._p, ox.form_checkbox(next(bookmarks), "Declaration", False)
              + ox.run(" I confirm that the information in this form is accurate and complete, and I will inform "
                       "Northwind Analytics of any change."))
    p = doc.add_paragraph("Name: ")
    ox.append(p._p, ox.sdt_text(next(ids), "Signatory", "Oliver Grant") + ox.run("    Date: ")
              + ox.sdt_date(next(ids), "Signature date", "2026-11-21", "21 November 2026"))
    # Tagged `signature`, the tag PDF form export reads as a signature field.
    p = doc.add_paragraph("Signature: ")
    ox.append(p._p, ox.sdt_text(next(ids), "Signature", "(sign here)"))
    doc.add_heading("For procurement use only", level=2)
    p = doc.add_paragraph("Supplier reference: ")
    ox.append(p._p, ox.sdt(ox.run("SUP-2026-0147"), next(ids), "Supplier reference", "<w:text/>",
                           lock="contentLocked"))
    p = doc.add_paragraph("Supplier category: ")
    ox.append(p._p, ox.sdt_list(next(ids), "Category", ["Strategic", "Preferred", "Approved", "Occasional"], 2)
              + ox.run("    Reviewed: ") + ox.sdt_checkbox(next(ids), "Reviewed", False))

    footer = section.footer.paragraphs[0]
    footer.text = "Form PROC-07, version 3, November 2026"
    footer.runs[0].font.size = Pt(8)
    ox.protect_forms(doc)
    _properties(doc, "Supplier Registration Form", "Procurement office", "Form PROC-07")
    doc.save(path)


# Minutes

_AR = '<w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:rtl/><w:lang w:val="en-GB" w:bidi="ar-EG"/>'
_HE = '<w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:rtl/><w:lang w:val="en-GB" w:bidi="he-IL"/>'
_ZH = '<w:rFonts w:hint="eastAsia" w:eastAsia="SimSun"/><w:lang w:val="en-GB" w:eastAsia="zh-CN"/>'
_JA = '<w:rFonts w:hint="eastAsia" w:eastAsia="MS Mincho"/><w:lang w:val="en-GB" w:eastAsia="ja-JP"/>'


@family("minutes", "docx")
def minutes(path: Path) -> None:
    """Minutes of an international committee: right-to-left Arabic and Hebrew paragraphs,
    Chinese and Japanese paragraphs, runs mixing scripts, bilingual tables."""
    doc = _new("Calibri", 11, 2.5)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    section.header.paragraphs[0].text = "International Partners Committee - Minutes - 21 November 2026"
    section.header.paragraphs[0].runs[0].font.size = Pt(9)

    def rtl(text: str, rpr: str, style: str = "") -> Paragraph:
        return _paragraph(doc, ox.para(ox.run(text, rpr), "<w:bidi/>", style=style))

    def translation(text: str) -> None:
        p = doc.add_paragraph()
        p.add_run("Translation: ").bold = True
        p.add_run(text).italic = True

    doc.add_heading("International Partners Committee", level=0)
    doc.add_paragraph("Minutes of the meeting held in Geneva on 21 November 2026, from 9:00 to 12:30.")
    doc.add_heading("Attendees", level=1)
    for person in (
        "Isabelle Fournier, Northwind Analytics, Lyon (chair)",
        "Ahmed Hassan, Cairo office",
        "Noa Cohen, Tel Aviv office",
        "Wang Li (王丽), Shanghai office",
        "Kenichi Sato (佐藤健一), Tokyo office",
        "Tom Becker, Northwind Analytics, secretary",
    ):
        doc.add_paragraph(person, style="List Bullet")

    doc.add_heading("1. Opening", level=1)
    doc.add_paragraph(
        "The chair opened the meeting, welcomed the delegates and recalled that statements are recorded in the "
        "language in which they were made, followed by an English translation by the secretary."
    )
    doc.add_paragraph(paragraph_text(51, 3))

    doc.add_heading("2. Statements by the regional offices", level=1)
    doc.add_heading("2.1 Cairo", level=2)
    rtl("رحّب السيد أحمد حسن "
        "بالمشاركين، وأكّد أن "
        "الشراكة مع مكتب القاهرة "
        "ستساعد على تحسين خدمة "
        "العملاء في المنطقة. "
        "وأشار إلى أن فريق القاهرة "
        "سيقدّم تقريراً عن تقدّم "
        "المشروع في الاجتماع "
        "القادم.", _AR)
    translation("Mr Ahmed Hassan welcomed the participants and stressed that the partnership with the Cairo office "
                "will help improve customer service in the region. He noted that the Cairo team will present a "
                "progress report at the next meeting.")
    doc.add_heading("2.2 Tel Aviv", level=2)
    rtl("גב׳ נועה כהן הציגה את "
        "תוכנית העבודה של משרד "
        "תל אביב לרבעון הבא. היא "
        "ציינה שהצוות יתמקד בשיפור "
        "זמני המשלוח ובהכשרת "
        "עובדים חדשים.", _HE)
    translation("Ms Noa Cohen presented the work plan of the Tel Aviv office for the next quarter. She noted that the "
                "team will focus on improving delivery times and on training new employees.")
    doc.add_heading("2.3 Shanghai", level=2)
    _paragraph(doc, ox.para(ox.run(
        "上海办事处的王丽女士介绍了新仓库的建设"
        "进度。她表示，仓库预计将于明年三月投入"
        "使用，届时华东地区的配送时间将缩短一天"
        "。", _ZH)))
    translation("Ms Wang Li of the Shanghai office presented the construction progress of the new warehouse. She said "
                "that the warehouse should open next March, and that delivery times in East China will then be one "
                "day shorter.")
    doc.add_heading("2.4 Tokyo", level=2)
    _paragraph(doc, ox.para(ox.run(
        "東京事務所の佐藤健一氏は、顧客満足度調"
        "査の結果を報告しました。回答者の八割以"
        "上がサービスに満足していると答え、特に"
        "サポートの対応速度が高く評価されました"
        "。", _JA)))
    translation("Mr Kenichi Sato of the Tokyo office reported the results of the customer satisfaction survey. More "
                "than eight in ten respondents said they were satisfied with the service, and the speed of support "
                "replies was rated particularly highly.")

    doc.add_heading("3. Project name", level=1)
    _paragraph(doc, ox.para(
        ox.run("The committee adopted Polaris as the name of the joint project, written ")
        + ox.run("北极星", _ZH) + ox.run(" in Chinese, ") + ox.run("北極星", _JA)
        + ox.run(" in Japanese, ") + ox.run("نجم الشمال", _AR)
        + ox.run(" in Arabic and ") + ox.run("כוכב הצפון", _HE)
        + ox.run(" in Hebrew. The logo will show the English name only.")
    ))
    doc.add_paragraph(paragraph_text(52, 3))

    doc.add_heading("4. Actions", level=1)
    actions = doc.add_table(rows=1, cols=3)
    actions.style = "Light Grid Accent 1"
    head = actions.rows[0].cells
    head[0].text, head[2].text = "Action", "Owner"
    ox.append(head[1].paragraphs[0]._p, ox.run("الإجراء", _AR))
    head[1].paragraphs[0]._p.get_or_add_pPr().append(ox.parse("<w:bidi/>")[0])
    for english, arabic, owner in (
        ("Send the revised budget to all partners by 5 December.",
         "إرسال الميزانية المعدّلة إلى جميع الشركاء قبل ٥ ديسمبر.", "I. Fournier"),
        ("Translate the customer survey into Arabic and Hebrew.",
         "ترجمة استبيان العملاء إلى العربية والعبرية.", "A. Hassan, N. Cohen"),
        ("Prepare the opening plan of the Shanghai warehouse.", "إعداد خطة افتتاح مستودع شنغهاي.", "Wang Li"),
        ("Share the support training material with the Tokyo office.", "مشاركة مواد تدريب الدعم مع مكتب طوكيو.", "K. Sato"),
    ):
        cells = actions.add_row().cells
        cells[0].text, cells[2].text = english, owner
        p = cells[1].paragraphs[0]
        p._p.get_or_add_pPr().append(ox.parse("<w:bidi/>")[0])
        ox.append(p._p, ox.run(arabic, _AR))

    doc.add_heading("5. Glossary", level=1)
    doc.add_paragraph("The secretary will keep the following terms consistent in all translated documents.")
    glossary = doc.add_table(rows=1, cols=3)
    glossary.style = "Table Grid"
    for c, (text, rpr) in enumerate((("English", ""), ("中文", _ZH), ("日本語", _JA))):
        ox.append(glossary.rows[0].cells[c].paragraphs[0]._p, ox.run(text, rpr or "<w:b/>"))
    for english, chinese, japanese in (
        ("Warehouse", "仓库", "倉庫"),
        ("Delivery time", "配送时间", "配送時間"),
        ("Customer satisfaction", "客户满意度", "顧客満足度"),
        ("Budget", "预算", "予算"),
        ("Meeting", "会议", "会議"),
    ):
        cells = glossary.add_row().cells
        cells[0].text = english
        ox.append(cells[1].paragraphs[0]._p, ox.run(chinese, _ZH))
        ox.append(cells[2].paragraphs[0]._p, ox.run(japanese, _JA))

    doc.add_heading("6. Summary in Arabic", level=1)
    rtl("ملخص الاجتماع", _AR, style="Heading2")
    rtl("وافقت اللجنة على اسم "
        "المشروع المشترك وعلى "
        "أربعة إجراءات، وسيعقد "
        "الاجتماع القادم في القاهرة "
        "في شهر مارس.", _AR)
    translation("The committee agreed on the name of the joint project and on four actions; the next meeting will be "
                "held in Cairo in March.")
    doc.add_heading("7. Next meeting", level=1)
    doc.add_paragraph("The next meeting will be held in Cairo on 18 March 2027. The meeting closed at 12:30.")
    _properties(doc, "International Partners Committee - Minutes", "Tom Becker", "Minutes, 21 November 2026")
    doc.save(path)


# Manual

_ERRORS = (
    ("Battery low", "The charge is under 10 %.", "Charge the scanner in its cradle."),
    ("Battery too hot", "The battery is above 55 °C.", "Let the scanner cool down before charging."),
    ("Scan window dirty", "The read rate fell under 80 %.", "Clean the window with a dry cloth."),
    ("No terminal paired", "The scanner has no Bluetooth pairing.", "Pair the scanner, see Getting started."),
    ("Connection lost", "The terminal is out of range.", "Move within 30 m of the terminal."),
    ("Firmware mismatch", "The terminal expects a newer firmware.", "Update the firmware, see Maintenance."),
    ("Memory full", "More than 10,000 scans are waiting.", "Dock the scanner to upload the scans."),
    ("Trigger stuck", "The trigger stayed pressed for 30 s.", "Release the trigger; call support if it persists."),
    ("Clock not set", "The real-time clock lost its time.", "Dock the scanner to synchronise the clock."),
    ("Unknown symbology", "The code type is disabled.", "Enable the symbology in the settings."),
)


@family("manual", "docx")
def manual(path: Path) -> None:
    """A product manual: merged and nested table cells, a long table with repeated header rows,
    a two-column section, a native chart, internal links and restarting numbered steps."""
    from pptx.chart.data import CategoryChartData
    from pptx.enum.chart import XL_CHART_TYPE

    doc = _new("Calibri", 10.5, 2.0)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    ox.add_styles(doc, "Hyperlink")
    steps = ox.add_numbering(doc, [("decimal", "Step %1.", 851, 851)], "6E3A9D01")
    bookmarks = count(1)
    footer = section.footer.paragraphs[0]
    footer.alignment = WD_ALIGN_PARAGRAPH.CENTER
    ox.append(footer._p, ox.run("SC-40 User and Maintenance Manual, version 2.3 - page ") + ox.field("PAGE", "1"))

    def heading(text: str, name: str, level: int = 1) -> None:
        _paragraph(doc, ox.para(ox.bookmark(next(bookmarks), name, ox.run(text)), style=f"Heading{level}"))

    def procedure(num_id: int, items: list[str]) -> None:
        for item in items:
            _paragraph(doc, ox.para(ox.run(item), ox.numbered(num_id)))

    doc.add_heading("SC-40 Handheld Scanner", level=0)
    doc.add_paragraph("User and Maintenance Manual, version 2.3, November 2026").runs[0].italic = True
    doc.add_paragraph(
        "This manual describes how to set up, use and maintain the SC-40 handheld scanner and its charging cradle. "
        "Keep it with the device and read the safety instructions before first use."
    )
    links = doc.add_paragraph()
    links.add_run("In this manual: ").bold = True
    for i, (anchor, label) in enumerate((("Safety", "Safety"), ("GettingStarted", "Getting started"),
                                         ("Specifications", "Specifications"), ("ErrorCodes", "Error codes"),
                                         ("Maintenance", "Maintenance"), ("Battery", "Battery performance"))):
        ox.append(links._p, (ox.run(" | ") if i else "") + ox.anchor_link(anchor, label))

    heading("1. Safety", "Safety")
    for item in (
        "Do not look into the scan window while the laser is on.",
        "Use only the supplied cradle and power adapter.",
        "Do not open the battery compartment with wet hands.",
        "Keep the scanner between -20 °C and 50 °C during use, and between -30 °C and 60 °C in storage.",
    ):
        doc.add_paragraph(item, style="List Bullet")

    heading("2. Getting started", "GettingStarted")
    heading("2.1 Charging the battery", "Charging", 2)
    procedure(steps, [
        "Connect the power adapter to the cradle and to a wall socket.",
        "Place the scanner in the cradle, scan window facing up.",
        "Wait until the status light turns green; a full charge takes about four hours.",
    ])
    heading("2.2 Pairing with a terminal", "Pairing", 2)
    procedure(ox.restart_numbering(doc, steps), [
        "Turn on Bluetooth on the terminal and open the Northwind Scan app.",
        "Hold the trigger and the side button together for five seconds.",
        "Scan the pairing code shown by the app.",
        "Check that the app shows the scanner's serial number.",
    ])
    p = doc.add_paragraph("If the scanner beeps three times and shows a code, look the code up in ")
    ox.append(p._p, ox.anchor_link("ErrorCodes", "Error codes") + ox.run("."))

    heading("3. Specifications", "Specifications")
    spec = doc.add_table(rows=11, cols=4)
    spec.style = "Table Grid"
    spec.cell(0, 0).merge(spec.cell(1, 0)).text = "Group"
    spec.cell(0, 1).merge(spec.cell(1, 1)).text = "Property"
    spec.cell(0, 2).merge(spec.cell(0, 3)).text = "Value"
    spec.cell(1, 2).text, spec.cell(1, 3).text = "Standard model", "Extended battery model"
    for r in range(2):
        for c in range(4):
            ox.cell_shading(spec.cell(r, c), "DCEFEA")
    rows = [
        ("Physical", "Dimensions", "168 x 72 x 38 mm", "168 x 72 x 46 mm"),
        ("Physical", "Weight", "280 g", "335 g"),
        ("Physical", "Protection", "IP65, 1.8 m drops on concrete", None),
        ("Battery", "Capacity", "3,200 mAh", "6,400 mAh"),
        ("Battery", "Runtime at 20 °C", "12 h", "21 h"),
        ("Battery", "Charging time", "4 h", "6 h"),
        ("Connectivity", "Bluetooth", "5.2, range 30 m", None),
        ("Connectivity", "Cradle", "USB-C and Ethernet", None),
        ("Reading", "Supported symbologies", "", None),
    ]
    for r, (group, prop, standard, extended) in enumerate(rows, start=2):
        spec.cell(r, 1).text = prop
        if extended is None:
            spec.cell(r, 2).merge(spec.cell(r, 3)).text = standard
        else:
            spec.cell(r, 2).text, spec.cell(r, 3).text = standard, extended
    for group, first, last in (("Physical", 2, 4), ("Battery", 5, 7), ("Connectivity", 8, 9), ("Reading", 10, 10)):
        cell = spec.cell(first, 0).merge(spec.cell(last, 0)) if last > first else spec.cell(first, 0)
        cell.text = group
    codes = spec.cell(10, 2)
    codes.paragraphs[0].text = "Enabled by default:"
    nested = codes.add_table(rows=2, cols=2)
    nested.style = "Table Grid"
    for r, (kind, names) in enumerate((("1D", "Code 128, Code 39, EAN-13, UPC-A"),
                                       ("2D", "QR Code, Data Matrix, PDF417"))):
        nested.cell(r, 0).text, nested.cell(r, 1).text = kind, names

    heading("4. Error codes", "ErrorCodes")
    doc.add_paragraph("The scanner shows the code on its display and the app shows the message. The table lists every "
                      "code of firmware 4.x.")
    errors = doc.add_table(rows=1, cols=4)
    errors.style = "Light List Accent 1"
    for c, name in enumerate(("Code", "Message", "Cause", "What to do")):
        errors.rows[0].cells[c].text = name
    ox.repeat_header(errors.rows[0])
    for n in range(72):
        message, cause, action = _ERRORS[n % len(_ERRORS)]
        cells = errors.add_row().cells
        variant = "" if n < len(_ERRORS) else f" ({['cradle', 'terminal', 'app', 'server', 'battery pack', 'sensor', 'radio'][n // len(_ERRORS) - 1]})"
        for c, value in enumerate((f"E{100 + n:03d}", message + variant, cause, action)):
            cells[c].text = value

    two = doc.add_section(WD_SECTION.CONTINUOUS)
    ox.columns(two, 2, 567, separator=True)
    heading("5. Maintenance", "Maintenance")
    heading("5.1 Cleaning", "Cleaning", 2)
    doc.add_paragraph("Clean the scan window once a week, or every day in dusty areas. Use a dry, lint-free cloth; "
                      "never use solvents, which damage the window coating.")
    doc.add_paragraph(paragraph_text(61, 4))
    heading("5.2 Storage", "Storage", 2)
    doc.add_paragraph("Before storing the scanner for more than a month, charge the battery to about 50 % and turn "
                      "the scanner off. Check the charge every three months.")
    doc.add_paragraph(paragraph_text(62, 4))
    heading("5.3 Updating the firmware", "Firmware", 2)
    procedure(ox.restart_numbering(doc, steps), [
        "Dock the scanner in a cradle connected to the network.",
        "In the Northwind Scan app, open Devices and choose Update firmware.",
        "Wait until the status light stops blinking; do not undock the scanner during the update.",
        "Check the firmware version in Settings, About.",
    ])
    doc.add_paragraph(paragraph_text(63, 5))
    one = doc.add_section(WD_SECTION.CONTINUOUS)
    ox.columns(one, 1, 720)

    heading("6. Battery performance", "Battery")
    doc.add_paragraph("Runtime depends on the temperature. The chart shows the runtime of both models with one scan "
                      "every ten seconds, measured in our laboratory.")
    data = CategoryChartData()
    data.categories = ["-20 °C", "-10 °C", "0 °C", "10 °C", "20 °C", "30 °C", "40 °C"]
    data.add_series("Standard model (h)", (5.1, 7.4, 9.8, 11.6, 12.0, 11.7, 10.2))
    data.add_series("Extended battery (h)", (9.0, 13.1, 17.2, 20.4, 21.0, 20.6, 18.1))
    chart = ox.add_chart(doc, XL_CHART_TYPE.LINE_MARKERS, data, "Runtime by temperature", 50,
                         int(15 * ox.EMU_PER_CM), int(8 * ox.EMU_PER_CM))
    _paragraph(doc, ox.para(chart, '<w:jc w:val="center"/>'))
    p = doc.add_paragraph("Below 0 °C, keep a spare battery warm in a pocket. For the specifications, see ")
    ox.append(p._p, ox.anchor_link("Specifications", "Specifications") + ox.run("."))
    ox.renumber_drawings(doc)
    _properties(doc, "SC-40 User and Maintenance Manual", "Northwind Devices", "Product manual, version 2.3")
    doc.save(path)


# Paper


@family("paper", "docx")
def paper(path: Path) -> None:
    """A two-column research paper: one-column title and abstract, a continuous break to two
    columns, display and inline equations, and endnotes used as numbered references."""
    m = ox
    doc = _new("Times New Roman", 10, 2.0)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    section.top_margin = section.bottom_margin = Cm(2.0)
    doc.styles["Normal"].paragraph_format.space_after = Pt(4)
    seeds = count(700)

    def text(lead: str = "", n: int = 4) -> Paragraph:
        p = doc.add_paragraph((lead + " " if lead else "") + paragraph_text(next(seeds), n))
        p.alignment = WD_ALIGN_PARAGRAPH.JUSTIFY
        return p

    def cite(p: Paragraph, reference: str, after: str = "") -> None:
        ox.add_note(doc, p, "endnote", reference)
        if after:
            ox.append(p._p, ox.run(after))

    def equation(*parts: str) -> None:
        _paragraph(doc, ox.para(ox.omath_para(*parts)))

    def sub(base: str, index: str) -> str:
        return m.m_sub(m.m_r(base), m.m_r(index))

    title = doc.add_paragraph()
    title.alignment = WD_ALIGN_PARAGRAPH.CENTER
    run = title.add_run("Sparse Demand Forecasting with Hierarchical Shrinkage")
    run.font.size, run.bold = Pt(18), True
    authors = doc.add_paragraph()
    authors.alignment = WD_ALIGN_PARAGRAPH.CENTER
    for name, mark in (("Sophie Laurent", "1"), (", Daniel Okafor", "1"), (" and Mei Tanaka", "2")):
        authors.add_run(name)
        authors.add_run(mark).font.superscript = True
    affiliations = doc.add_paragraph()
    affiliations.alignment = WD_ALIGN_PARAGRAPH.CENTER
    affiliations.add_run("1").font.superscript = True
    affiliations.add_run("University of Westmere, Department of Industrial Engineering   ")
    affiliations.add_run("2").font.superscript = True
    affiliations.add_run("Kyoto Institute of Logistics").italic = True
    abstract = doc.add_paragraph()
    abstract.alignment = WD_ALIGN_PARAGRAPH.JUSTIFY
    abstract.paragraph_format.left_indent = abstract.paragraph_format.right_indent = Cm(1.5)
    abstract.add_run("Abstract. ").bold = True
    abstract.add_run(
        "Most items of a spare parts catalogue sell a few units a year, which leaves too little history to fit a "
        "forecasting model per item. We pool items through a hierarchical Poisson model whose item effects shrink "
        "towards the mean of their family. On 18,400 items of a European distributor, shrinkage lowers the root "
        "mean squared error by 14 % against Croston forecasts and by 22 % against per-item exponential smoothing, "
        "with the largest gains on items with fewer than ten sales."
    )
    keywords = doc.add_paragraph()
    keywords.paragraph_format.left_indent = Cm(1.5)
    keywords.add_run("Keywords: ").bold = True
    keywords.add_run("intermittent demand, empirical Bayes, Poisson regression, spare parts")

    body = doc.add_section(WD_SECTION.CONTINUOUS)
    ox.columns(body, 2, 425)

    doc.add_heading("1 Introduction", level=1)
    p = text("Intermittent demand is the norm in spare parts, where a part may sell twice a year and then not at all.")
    cite(p, "Croston, J. D. (1972). Forecasting and stock control for intermittent demands. Operational Research "
            "Quarterly, 23(3), 289-303.", " Croston's method separates the size of the demands from the intervals "
                                          "between them.")
    p = text()
    cite(p, "Syntetos, A. A. and Boylan, J. E. (2005). The accuracy of intermittent demand estimates. International "
            "Journal of Forecasting, 21(2), 303-314.")
    text()
    doc.add_heading("2 Model", level=1)
    text("Let the sales of item i in week t follow a Poisson distribution whose rate depends on the item and on "
         "covariates such as price and season:")
    equation(sub("y", "i,t"), m.m_r("∼"), m.m_r("Poisson", plain=True), m.m_delim(sub("λ", "i,t")))
    equation(m.m_func("log", sub("λ", "i,t")), m.m_r("="), sub("α", "i"), m.m_r("+"),
             m.m_sup(m.m_r("β"), m.m_r("⊤")), sub("x", "i,t"))
    p = doc.add_paragraph("The item effects ")
    ox.append(p._p, ox.omath(sub("α", "i")) + ox.run(" share a normal prior with mean ")
              + ox.omath(m.m_r("μ")) + ox.run(" and variance ") + ox.omath(m.m_sup(m.m_r("τ"), m.m_r("2")))
              + ox.run(", so that the posterior mean of an item with ") + ox.omath(sub("n", "i"))
              + ox.run(" weeks of history is"))
    equation(m.m_acc(sub("α", "i")), m.m_r("="),
             m.m_frac(sub("n", "i") + m.m_bar(sub("y", "i")) + m.m_r("+κμ"), sub("n", "i") + m.m_r("+κ")))
    p = doc.add_paragraph("where ")
    ox.append(p._p, ox.omath(m.m_r("κ"), m.m_r("="), m.m_frac(m.m_sup(m.m_r("σ"), m.m_r("2")),
                                                                      m.m_sup(m.m_r("τ"), m.m_r("2"))))
              + ox.run(" sets the strength of the shrinkage. Items with little history stay close to their family "
                       "mean, while items with a long history keep their own estimate."))
    text()
    doc.add_heading("3 Estimation", level=1)
    text("We estimate the family means and the prior variances by maximum marginal likelihood. Two families are "
         "correlated through the covariance matrix")
    sigma = m.m_r("σ")
    equation(m.m_r("Σ"), m.m_r("="), m.m_matrix([
        [m.m_subsup(sigma, m.m_r("1"), m.m_r("2")), m.m_r("ρ") + sub("σ", "1") + sub("σ", "2")],
        [m.m_r("ρ") + sub("σ", "1") + sub("σ", "2"), m.m_subsup(sigma, m.m_r("2"), m.m_r("2"))],
    ]))
    p = text("Accuracy is measured by the root mean squared error over the N weeks of the test period:", 2)
    equation(m.m_r("RMSE"), m.m_r("="), m.m_rad(
        m.m_frac(m.m_r("1"), m.m_r("N")) + m.m_nary("∑", m.m_r("t=1"), m.m_r("N"), m.m_sup(
            m.m_delim(sub("y", "t") + m.m_r("−") + m.m_acc(sub("y", "t"))), m.m_r("2")))))
    p = doc.add_paragraph("Sales are skewed, so we also report errors after a cube-root transform ")
    ox.append(p._p, ox.omath(m.m_rad(m.m_r("y"), m.m_r("3"))) + ox.run(", which keeps zero sales at zero."))
    cite(p, "Hyndman, R. J. and Koehler, A. B. (2006). Another look at measures of forecast accuracy. International "
            "Journal of Forecasting, 22(4), 679-688.")
    doc.add_heading("4 Experiments", level=1)
    text("The data cover 104 weeks of sales of 18,400 items in 312 families. The first 78 weeks train the models and "
         "the last 26 weeks test them.")
    table = doc.add_table(rows=1, cols=3)
    table.style = "Light List Accent 1"
    for c, name in enumerate(("Method", "RMSE", "Cube-root RMSE")):
        table.rows[0].cells[c].text = name
    for row in (("Per-item smoothing", "1.84", "0.61"), ("Croston", "1.66", "0.55"), ("Pooled, no shrinkage", "1.71", "0.58"),
                ("Hierarchical shrinkage", "1.43", "0.47")):
        cells = table.add_row().cells
        for c, value in enumerate(row):
            cells[c].text = value
    caption = doc.add_paragraph("Table 1. Test errors, mean over items.")
    caption.runs[0].italic = True
    text()
    doc.add_picture(chart_png((61, 55, 48, 39, 33, 30), 600, 360, (180, 83, 9)), width=Cm(7.8))
    caption = doc.add_paragraph("Figure 1. Error reduction by number of sales in the training period.")
    caption.runs[0].italic = True
    text()
    p = text()
    cite(p, "Efron, B. and Morris, C. (1975). Data analysis using Stein's estimator and its generalizations. "
            "Journal of the American Statistical Association, 70(350), 311-319.")
    doc.add_heading("5 Conclusion", level=1)
    text("Shrinking item effects towards their family is a cheap and robust way to forecast sparse demand.")
    text()

    # Endnotes are the references: number them 1, 2, 3 rather than Word's default i, ii, iii.
    ox.set_setting(doc, '<w:endnotePr><w:numFmt w:val="decimal"/><w:endnote w:id="-1"/><w:endnote w:id="0"/></w:endnotePr>')
    ox.set_child(doc.sections[-1]._sectPr, ox.parse('<w:endnotePr><w:numFmt w:val="decimal"/></w:endnotePr>')[0],
                 ox.SECTPR_ORDER)
    _properties(doc, "Sparse Demand Forecasting with Hierarchical Shrinkage", "Sophie Laurent", "Research paper")
    doc.save(path)


# Newsletter


@family("newsletter", "docx")
def newsletter(path: Path) -> None:
    """A staff newsletter: a masthead of DrawingML shapes, a three-column section, floating
    pictures with square and tight wrap, text boxes (sidebar and pull quote) as Word writes them."""
    doc = _new("Calibri", 10, 1.8)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    section.top_margin = section.bottom_margin = Cm(1.8)
    cm = ox.EMU_PER_CM
    seeds = count(800)

    banner = doc.add_paragraph()
    ox.append(banner._p, ox.shape(1, "rect", int(17.4 * cm), int(2.4 * cm), "0F766E", ("margin", 0), ("margin", 0),
                                  wrap="topAndBottom", text="NORTHWIND NEWS")
              + ox.shape(2, "ellipse", int(2.2 * cm), int(2.2 * cm), "F59E0B", ("margin", int(15.0 * cm)),
                         ("margin", int(0.1 * cm)), text="No. 14"))
    tagline = doc.add_paragraph()
    tagline.alignment = WD_ALIGN_PARAGRAPH.CENTER
    run = tagline.add_run("Issue 14, November 2026 - news from our sites and teams")
    run.italic, run.font.size = True, Pt(11)

    columns = doc.add_section(WD_SECTION.CONTINUOUS)
    ox.columns(columns, 3, 425)

    def article(title: str, lead: str, count_: int = 3) -> list[Paragraph]:
        doc.add_heading(title, level=2)
        out = [doc.add_paragraph(lead)]
        for _ in range(count_):
            out.append(doc.add_paragraph(paragraph_text(next(seeds), 4)))
        for p in out:
            p.alignment = WD_ALIGN_PARAGRAPH.JUSTIFY
        return out

    lyon = article("Lyon warehouse doubles its capacity",
                   "The extension of the Lyon warehouse opened on 3 November, adding 12,000 pallet places and two "
                   "loading docks. It now serves every customer south of Dijon within a day.")
    ox.append(lyon[1]._p, ox.floating_picture(doc.part, chart_png((30, 44, 52, 61, 70, 79), 600, 400), 3,
                                              int(3.6 * cm), "square", ("column", "right"), ("paragraph", 0)))
    night = article("Meet the support night shift",
                    "Since September, four engineers answer customer calls between 20:00 and 8:00. We spent a night "
                    "with them in the Lyon office.")
    ox.append(night[2]._p, ox.floating_picture(doc.part, logo_png("24", 200, (79, 70, 229)), 4, int(2.4 * cm),
                                               "tight", ("column", "left"), ("paragraph", 0)))
    quote = ox.para(ox.run("“We answered 98 % of night calls within ten minutes.”",
                           '<w:i/><w:color w:val="0F766E"/><w:sz w:val="24"/>'), '<w:jc w:val="center"/>')
    quote += ox.para(ox.run("Karim Benali, support lead", '<w:sz w:val="16"/>'), '<w:jc w:val="right"/>')
    ox.append(night[3]._p, ox.text_box(5, int(5.0 * cm), int(3.4 * cm), quote, ("column", 0), ("paragraph", 0),
                                       "topAndBottom", fill="FFFFFF", line="0F766E"))
    article("A greener fleet",
            "Eight electric vans joined the delivery fleet in Lyon and Grenoble. They cover the city centres, where "
            "low-emission zones start in January.", 2)
    article("New faces",
            "Welcome to the eleven colleagues who joined us this autumn, in sales, support and the warehouses.", 2)
    diary = "".join(
        ox.para(ox.run(t, "<w:b/>" if i == 0 else ""), '<w:spacing w:after="60"/>')
        for i, t in enumerate((
            "Dates for your diary",
            "28 November: open day at the Lyon warehouse",
            "5 December: quarterly results webcast, 11:00",
            "12 December: end-of-year party, Grenoble",
            "9 January: new travel policy comes into force",
        ))
    )
    last = doc.add_paragraph(paragraph_text(next(seeds), 3))
    ox.append(last._p, ox.text_box(6, int(5.0 * cm), int(3.6 * cm), diary, ("column", 0), ("paragraph", 0),
                                   "topAndBottom"))
    article("Survey results",
            "Thank you to the 412 colleagues who answered the autumn survey. Satisfaction with internal tools rose "
            "from 61 % to 72 %; the canteen in Grenoble remains the most discussed topic.", 2)
    end = doc.add_paragraph()
    ox.append(end._p, ox.shape(7, "roundRect", int(5.0 * cm), int(1.0 * cm), "4F46E5", ("column", 0),
                               ("paragraph", 0), wrap="topAndBottom", text="Read more on the intranet"))

    footer = section.footer.paragraphs[0]
    footer.alignment = WD_ALIGN_PARAGRAPH.CENTER
    footer.text = "Northwind News is published every month by the communication team. Send your stories to news@northwind.example."
    footer.runs[0].font.size = Pt(8)
    ox.renumber_drawings(doc)
    _properties(doc, "Northwind News, issue 14", "Communication team", "Staff newsletter, November 2026")
    doc.save(path)


# Long report

_SITES = (
    "Lyon", "Grenoble", "Marseille", "Toulouse", "Bordeaux", "Nantes", "Lille", "Strasbourg", "Dijon", "Rennes",
    "Geneva", "Zurich", "Milan", "Turin", "Barcelona", "Madrid", "Valencia", "Porto", "Lisbon", "Brussels",
    "Antwerp", "Rotterdam", "Cologne", "Frankfurt", "Munich", "Vienna", "Prague", "Warsaw", "Leeds", "Dublin",
)
_TOPICS = ("Volumes and service", "Customers", "People and safety", "Costs", "Projects", "Risks and next steps")


@family("long-report", "docx")
def long_report(path: Path) -> None:
    """About 150 pages of the report pattern (headings, lists, tables, a reused picture), for
    edits and reads on long files."""
    doc = _new("Calibri", 11, 2.2)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    section.header.paragraphs[0].text = "Northwind Analytics: Annual Operations Report 2026"
    footer = section.footer.paragraphs[0]
    footer.alignment = WD_ALIGN_PARAGRAPH.CENTER
    _page_x_of_y(footer)
    steps = ox.add_numbering(doc, [("decimal", "%1.", 720, 360)], "7F4BAE01")
    picture = chart_png().getvalue()
    seeds = count(1000)

    doc.add_heading("Annual Operations Report 2026", level=0)
    doc.add_paragraph(
        "This report reviews the year site by site: volumes and service, customers, people, costs, projects and "
        "risks. Each chapter follows the same structure so that sites can be compared, and the appendix of each "
        "chapter gives the monthly indicators."
    )
    for n, site in enumerate(_SITES, start=1):
        doc.add_heading(f"{n}. {site}", level=1)
        intro = doc.add_paragraph()
        intro.add_run("Summary. ").bold = True
        intro.add_run(paragraph_text(next(seeds), 3))
        for t, topic in enumerate(_TOPICS, start=1):
            doc.add_heading(f"{n}.{t} {topic}", level=2)
            for _ in range(4):
                doc.add_paragraph(paragraph_text(next(seeds), 5))
            if t == 1:
                for text in sentences(next(seeds), 3):
                    doc.add_paragraph(text, style="List Bullet")
        restart = ox.restart_numbering(doc, steps)
        for text in sentences(next(seeds), 3):
            _paragraph(doc, ox.para(ox.run(text), ox.numbered(restart)))
        doc.add_paragraph(f"Table {n}. Monthly indicators of the {site} site.").runs[0].italic = True
        months = ("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")
        table = doc.add_table(rows=len(months) + 2, cols=5)
        table.style = "Light Grid Accent 1"
        for c, name in enumerate(("Month", "Orders", "On time (%)", "Tickets", "Cost (kEUR)")):
            table.cell(0, c).text = name
        for r, month in enumerate(months, start=1):
            base = 900 + 37 * n + 11 * r
            for c, value in enumerate((month, f"{base:,}", f"{94 + (n + r) % 5}.{(n * r) % 10}", str(120 + (n * 7 + r * 3) % 60),
                                       str(310 + (n * 13 + r * 5) % 90))):
                table.cell(r, c).text = value
        note = table.cell(len(months) + 1, 0).merge(table.cell(len(months) + 1, 4))
        note.text = "Figures are unaudited and exclude one-off costs."
        if n % 12 == 1:
            doc.add_paragraph()
            doc.add_picture(io.BytesIO(picture), width=Cm(14))  # same bytes: one image part
            doc.add_paragraph(f"Figure {n // 12 + 1}. Orders by quarter, indexed.").runs[0].italic = True
    _properties(doc, "Annual Operations Report 2026", "Operations team", "Annual report, long document")
    doc.save(path)


# Record


_PALE = {"Done": "C6EFCE", "Late": "FFC7CE", "Open": "E7E6E6"}


@family("record", "docx")
def record(path: Path) -> None:
    """A change record co-edited in Word and Google Docs: numbered headings bound to a list, a TOC
    that takes outline levels next to a vertically merged table, key-value, history and matrix
    tables with a grey key column, merged headers and pale run shading, content controls, a comment
    thread of two people with a two-paragraph comment and a resolved one, tracked changes by one
    author, margins away from Word's defaults, Page X of Y."""
    doc = _new("Calibri", 10.5, 2.0)
    section = doc.sections[0]
    section.page_width, section.page_height = Cm(21.0), Cm(29.7)
    section.left_margin, section.right_margin = Cm(2.2), Cm(1.8)
    section.top_margin, section.bottom_margin = Cm(2.4), Cm(2.1)
    width = 9638  # text width in twips, for the right-aligned tab stops
    ox.add_styles(doc, "TOC1", "TOC2", "Hyperlink")
    headings = ox.add_numbering(doc, [("decimal", "%1.", 432, 432), ("decimal", "%1.%2.", 720, 720)], "6E3A9D01",
                                styles=("Heading1", "Heading2"))
    for level in range(2):
        ox.style_numbering(doc, f"Heading{level + 1}", headings, level)
    owner, reviewer = "Amélie Roux", "Tomás Herrera"
    editor = owner  # every tracked change is hers
    bookmarks, revisions, controls, seeds = count(1), count(500), count(1830100), count(700)
    outline = []

    section.header.paragraphs[0].text = "Change record CR-2026-014 - Warehouse management system upgrade"
    footer = section.footer.paragraphs[0]
    footer.alignment = WD_ALIGN_PARAGRAPH.RIGHT
    _page_x_of_y(footer, "Internal - ")

    def heading(level: int, title: str, inner: str = "") -> None:
        name = f"_Toc2172{len(outline) + 1:05d}"
        _paragraph(doc, ox.para(ox.bookmark(next(bookmarks), name, inner or ox.run(title)), style=f"Heading{level}"))
        outline.append((level, title, name))

    def text(n: int = 4, lead: str = "") -> Paragraph:
        return doc.add_paragraph((lead + " " if lead else "") + paragraph_text(next(seeds), n))

    def grid(rows: int, cols: int, widths: list[float]):
        table = doc.add_table(rows=rows, cols=cols)
        table.style = "Table Grid"
        for row in table.rows:
            for cell, w in zip(row.cells, widths):
                cell.width = Cm(w)
        return table

    def cell_xml(cell, content: str) -> None:
        """Replace a cell's paragraph content with raw runs."""
        p = cell.paragraphs[0]._p
        for child in list(p):
            if child.tag != qn("w:pPr"):
                p.remove(child)
        ox.append(p, content)

    title = doc.add_paragraph()
    title.alignment = WD_ALIGN_PARAGRAPH.LEFT
    r = title.add_run("Change record CR-2026-014")
    r.bold, r.font.size = True, Pt(20)
    doc.add_paragraph("Warehouse management system upgrade, regional sites").runs[0].italic = True
    toc_slot = _paragraph(doc, ox.para())

    # 1. Identification: a key-value table with a grey key column and content controls.
    heading(1, "Identification")
    text(4, "This record follows the upgrade of the warehouse management system from the request to the closure "
            "review.")
    keys = grid(6, 2, [5.0, 12.0])
    values = [
        ("Reference", ox.sdt_text(next(controls), "Reference", "CR-2026-014")),
        ("Owner", ox.sdt_text(next(controls), "Owner", owner)),
        ("Opened on", ox.sdt_date(next(controls), "Opened on", "2026-10-05", "5 October 2026")),
        ("Priority", ox.sdt_list(next(controls), "Priority", ["Low", "Medium", "High"], 2)),
        ("Status", ox.sdt_list(next(controls), "Status", ["Draft", "In review", "Approved", "Closed"], 1)),
        ("Customer impact", ox.sdt_checkbox(next(controls), "Customer impact", True) + ox.run(" Yes")),
    ]
    for row, (key, value) in zip(keys.rows, values):
        row.cells[0].text = key
        row.cells[0].paragraphs[0].runs[0].bold = True
        ox.cell_shading(row.cells[0], "D9D9D9")
        cell_xml(row.cells[1], value)

    # 2. Scope, with tracked changes by one editor.
    heading(1, "Scope")
    heading(2, "In scope")
    scope = _paragraph(doc, ox.para(
        ox.run("The upgrade covers the ")
        + ox.deleted(next(revisions), editor, LATER, "four")
        + ox.inserted(next(revisions), editor, LATER, ox.run("six"))
        + ox.run(" regional warehouses, their handheld scanners and the interface with the transport planning "
                 "tool.")
    ))
    _paragraph(doc, ox.para(
        ox.inserted(next(revisions), editor, LATER,
                    ox.run("The label printers of the two new sites are replaced as part of the same change.")),
        ox.inserted_mark(next(revisions), editor, LATER),
    ))
    heading(2, "Out of scope")
    _paragraph(doc, ox.para(
        ox.run("The finance interface ")
        + ox.deleted(next(revisions), editor, LATER, "and the yard management module ")
        + ox.run("stay on the current version until the next release window.")
    ))
    text(3)

    # 3. Impact matrix: wide, merged headers, a vertical merge next to the TOC's outline levels.
    heading(1, "Impact assessment")
    text(3, "Each site rated the impact of the change on its processes before the go-live decision.")
    sites = ["Lyon", "Grenoble", "Dijon", "Nantes", "Lille", "Rennes"]
    matrix = grid(5, 1 + len(sites), [3.2] + [2.4] * len(sites))
    header, sub = matrix.rows[0], matrix.rows[1]
    header.cells[0].text, sub.cells[0].text = "Process", ""
    first_half = header.cells[1].merge(header.cells[3])
    second_half = header.cells[4].merge(header.cells[6])
    first_half.text, second_half.text = "Existing sites", "New sites"
    for cell, site in zip(sub.cells[1:], sites):
        cell.text = site
    proc = matrix.cell(0, 0).merge(matrix.cell(1, 0))
    proc.text = "Process"
    for row, (process, ratings) in zip(matrix.rows[2:], (
        ("Receiving", "LMHLMH"), ("Picking", "MMHHLM"), ("Shipping", "LLMMHH"),
    )):
        row.cells[0].text = process
        for cell, rating in zip(row.cells[1:], ratings):
            cell.text = {"L": "Low", "M": "Medium", "H": "High"}[rating]
    for cell in header.cells + sub.cells:
        ox.cell_shading(cell, "D9D9D9")
    ox.repeat_header(header)
    _paragraph(doc, ox.para(ox.run("Ratings agreed with the site managers."), '<w:outlineLvl w:val="2"/>'))
    heading(2, "Risks")
    risks = text(4, "The main risk is a scanner firmware that the new version does not support.")
    text(4)

    # 4. Tasks: a status column with pale run shading.
    heading(1, "Tasks")
    tasks = grid(5, 3, [9.0, 4.0, 4.0])
    for cell, label in zip(tasks.rows[0].cells, ("Task", "Owner", "Status")):
        cell.text = label
        cell.paragraphs[0].runs[0].bold = True
        ox.cell_shading(cell, "D9D9D9")
    for row, (task, who, status) in zip(tasks.rows[1:], (
        ("Freeze the configuration", owner, "Done"),
        ("Test the scanner firmware", reviewer, "Late"),
        ("Train the site leads", owner, "Open"),
        ("Plan the cut-over weekend", reviewer, "Done"),
    )):
        row.cells[0].text, row.cells[1].text = task, who
        cell_xml(row.cells[2], ox.run(status, f'<w:shd w:val="clear" w:color="auto" w:fill="{_PALE[status]}"/>'))
    approval = text(3, "Approval of the cut-over plan is recorded in the next section.")

    # 5. Decision, a rich text control and more tracked changes.
    heading(1, "Decision")
    for element in ox.parse(ox.sdt(
        ox.para(ox.run("The change board approves the upgrade, subject to the firmware test passing on the "
                       "pilot site."))
        + ox.para(ox.run("Go-live is planned for the first weekend after the test report.")),
        next(controls), "Decision text",
    )):
        doc.element.body.insert(len(doc.element.body) - 1, element)
    _paragraph(doc, ox.para(
        ox.run("Decision date: ") + ox.sdt_date(next(controls), "Decision date", "2026-11-18", "18 November 2026")
        + ox.run(". Approved by ") + ox.sdt_text(next(controls), "Approver", reviewer)
        + ox.deleted(next(revisions), editor, LATER, " and the site managers")
        + ox.run(".")
    ))

    # 6. History: a dated table.
    heading(1, "History")
    history = grid(5, 4, [2.0, 3.2, 3.6, 8.2])
    for cell, label in zip(history.rows[0].cells, ("Version", "Date", "Author", "Change")):
        cell.text = label
        cell.paragraphs[0].runs[0].bold = True
        ox.cell_shading(cell, "D9D9D9")
    for row, entry in zip(history.rows[1:], (
        ("0.1", "5 October 2026", owner, "First draft"),
        ("0.2", "19 October 2026", reviewer, "Impact assessment added"),
        ("0.3", "4 November 2026", owner, "Two new sites in scope"),
        ("1.0", "18 November 2026", reviewer, "Approved by the change board"),
    )):
        for cell, value in zip(row.cells, entry):
            cell.text = value
    _paragraph(doc, ox.para(
        ox.inserted(next(revisions), editor, LATER, ox.run("Version 1.1 will record the closure review.")),
        ox.inserted_mark(next(revisions), editor, LATER),
    ))

    # Contents, now that the headings are known: in a content control, taking outline levels.
    def entry(level: int, title: str, anchor: str, opening: str = "") -> str:
        tabs = f'<w:tabs><w:tab w:val="right" w:leader="dot" w:pos="{width}"/></w:tabs>'
        inner = ox.run(title) + ox.tab() + ox.field(f"PAGEREF {anchor} \\h", "1")
        return ox.para(opening + ox.anchor_link(anchor, inner=inner), tabs, style=f"TOC{level}")

    toc = ox.para(ox.run("Contents"), '<w:numPr><w:ilvl w:val="0"/><w:numId w:val="0"/></w:numPr>', style="Heading1")
    for i, (level, title_text, anchor) in enumerate(outline):
        opening = ox.field_start('TOC \\o "1-2" \\h \\z \\u') if i == 0 else ""
        toc += entry(level, title_text, anchor, opening)
    toc += ox.para(ox.field_end())
    gallery = '<w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj>'
    toc_slot._p.addprevious(ox.parse(ox.sdt(toc, next(controls), kind=gallery))[0])
    toc_slot._p.getparent().remove(toc_slot._p)

    # Comments: a thread of two people with a two-paragraph comment and a reply, a resolved one.
    def comment(runs, text_: str, author: str, date: str, more: str = ""):
        initials = "".join(part[0] for part in author.split())
        made = doc.add_comment(runs, text=text_, author=author, initials=initials)
        if more:
            made.add_paragraph(more)
        made._comment_elm.set(qn("w:date"), date)
        return made.comment_id

    parents = {}
    question = comment(risks.runs, "Which scanner models are affected?", reviewer, DATE,
                       "If it is only the older model, we can replace them at the pilot site first.")
    parents[comment(risks.runs, "Only the older model, 40 units across three sites.", owner, LATER)] = question
    done = comment(scope.runs[:1], "Confirm the two new sites with the regional director.", reviewer, DATE)
    comment(approval.runs, "Add the board's reference number once we have it.", owner, LATER)
    ox.link_comments(doc, parents, done=frozenset({done}))

    _properties(doc, "Change record CR-2026-014", owner, "Warehouse management system upgrade")
    doc.save(path)

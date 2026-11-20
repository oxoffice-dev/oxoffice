"""Author the corpus source documents with python-docx and python-pptx.

Writes files/authored/report.docx and files/authored/deck.pptx: realistic
content that the producers (Word, PowerPoint, LibreOffice, Google) then open
and save, see produce.py.

    uv run --with python-docx==1.2.0 --with python-pptx==1.0.2 --with pillow==13.0.0 \
        tests/corpus/generate/author.py
"""

import io
from pathlib import Path

from docx import Document
from docx.enum.section import WD_ORIENT
from docx.enum.table import WD_TABLE_ALIGNMENT
from docx.enum.text import WD_BREAK
from docx.shared import Cm, Pt, RGBColor
from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.util import Inches
from pptx.util import Pt as PPt

OUT = Path(__file__).resolve().parent.parent / "files" / "authored"
OUT.mkdir(exist_ok=True)


def chart_png(width=900, height=450) -> io.BytesIO:
    image = Image.new("RGB", (width, height), "white")
    draw = ImageDraw.Draw(image)
    values = [42, 55, 61, 58, 73, 80]
    bar = width // (len(values) * 2)
    for i, value in enumerate(values):
        x = bar // 2 + i * 2 * bar
        top = height - 40 - int(value / 100 * (height - 80))
        draw.rectangle([x, top, x + bar, height - 40], fill=(79, 70, 229))
        draw.text((x, height - 30), f"Q{i % 4 + 1}", fill="black")
    draw.line([20, height - 40, width - 20, height - 40], fill="black", width=2)
    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    buffer.seek(0)
    return buffer


def report() -> None:
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
    doc.add_comment(comment_target.runs, text="Confirm with finance before sending.", author="Reviewer", initials="RV")

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
    doc.save(OUT / "report.docx")


def deck() -> None:
    prs = Presentation()
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
    prs.save(OUT / "deck.pptx")


if __name__ == "__main__":
    report()
    deck()
    print("authored", sorted(p.name for p in OUT.iterdir()))

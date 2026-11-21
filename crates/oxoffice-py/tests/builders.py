"""Synthetic documents for the tests, built with python-docx and python-pptx so that they carry the traits of a
document another program wrote. Each builder writes a file and returns its path."""
import io
import itertools
import random
import re
import zipfile

import docx
import pptx
from docx.oxml import parse_xml
from docx.oxml.ns import nsdecls
from PIL import Image
from pptx.enum.shapes import MSO_SHAPE
from pptx.util import Inches

W14 = "http://schemas.microsoft.com/office/word/2010/wordml"
W15 = "http://schemas.microsoft.com/office/word/2012/wordml"


def noise_png(side, seed):
    """A PNG of random pixels, which does not compress: about 3 * side * side bytes."""
    noise = random.Random(seed).randbytes(side * side * 3)
    buffer = io.BytesIO()
    Image.frombytes("RGB", (side, side), noise).save(buffer, format="PNG")
    return buffer.getvalue()


def edit_cost_pptx(path, slides, picture_side=0):
    """`slides` slides of 25 text boxes, 12 filled rectangles and a 4x4 table; with `picture_side`, each slide also
    holds a random-noise PNG of that side in pixels (it does not compress, about 3 bytes a pixel) and speaker notes."""
    prs = pptx.Presentation()
    for k in range(slides):
        slide = prs.slides.add_slide(prs.slide_layouts[6])
        for i in range(25):
            box = slide.shapes.add_textbox(Inches(0.2 + 0.5 * (i % 5)), Inches(0.2 + 0.5 * (i // 5)), Inches(2),
                                           Inches(0.4))
            box.text_frame.text = f"text {i}"
        for i in range(12):
            slide.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(4 + 0.4 * i), Inches(4), Inches(0.3),
                                   Inches(0.3)).fill.solid()
        table = slide.shapes.add_table(4, 4, Inches(0.5), Inches(5), Inches(6), Inches(1.5)).table
        for r in range(4):
            for c in range(4):
                table.cell(r, c).text = f"{r}.{c}"
        if picture_side:
            slide.shapes.add_picture(io.BytesIO(noise_png(picture_side, k)), Inches(7), Inches(0.5), Inches(2.5))
            slide.notes_slide.notes_text_frame.text = f"notes {k}"
    prs.save(path)
    return path


def edit_cost_docx(path, paragraphs, picture_side=0, probes=3):
    """`paragraphs` body paragraphs of 6 runs, each run with the run properties Google Docs writes on every run, and
    a 2x2 table after every 10th; with `picture_side`, every 10th paragraph also holds a random-noise PNG of that side
    in pixels (about 3 bytes a pixel), so that the package grows with the body. Two sections: the first one has a
    header, which the second one follows. The second one ends with the targets of test_docx_scaling.py, `probes` of
    each, since making one costs about as much as a timed call: paragraphs "alpha beta k" of two runs (twice as
    many), paragraphs holding a picture, paragraphs "commented k" with comment k, paragraphs with a tracked insertion
    of id 901 + k, block content controls tagged probe_k each followed by a paragraph "spot k", a repeating section
    tagged rows, 2x2 tables with "alpha k" in their first cell, and a last paragraph. Every paragraph, run and table
    row of the body carries the identifiers Word writes when it saves (saved_by_word); the header and the comments do
    not."""
    d = docx.Document()
    for i in range(paragraphs):
        p = d.add_paragraph()
        for k in range(6):
            font = p.add_run(f"run {k} of paragraph {i} ").font
            font.name, font.size, font.color.rgb = "Arial", docx.shared.Pt(10), docx.shared.RGBColor(0, 0, 0)
            font.bold = font.italic = font.small_caps = font.strike = font.underline = False
        if i % 10 == 9:
            if picture_side:
                p.add_run().add_picture(io.BytesIO(noise_png(picture_side, i)), width=docx.shared.Inches(1))
            table = d.add_table(rows=2, cols=2)
            for c, cell in enumerate(table._cells):
                cell.text = f"cell {c} of table {i // 10}"
    d.sections[0].header.is_linked_to_previous = False
    d.sections[0].header.paragraphs[0].text = "header"
    d.add_section()  # the second section follows the first one's header
    d.add_paragraph("the second section")
    for k in range(2 * probes):
        d.add_paragraph("alpha ").add_run(f"beta {k}")
    for k in range(probes):
        d.add_paragraph().add_run().add_picture(io.BytesIO(noise_png(8, 50 + k)), width=docx.shared.Inches(1))
    for k in range(probes):
        p = d.add_paragraph(f"commented {k}")
        d.add_comment(p.runs, text=f"comment {k}", author="Reviewer", initials="RV")
    for k in range(probes):
        d.add_paragraph(f"tracked {k} ")._p.append(parse_xml(
            f'<w:ins {nsdecls("w")} w:id="{901 + k}" w:author="Editor" w:date="2026-01-01T00:00:00Z"><w:r>'
            f'<w:t>inserted {k}</w:t></w:r></w:ins>'))
    for k in range(probes):
        d.add_paragraph(f"spot {k}")._p.addprevious(parse_xml(
            f'<w:sdt {nsdecls("w")}><w:sdtPr><w:tag w:val="probe{k}"/></w:sdtPr><w:sdtContent><w:p><w:r>'
            f'<w:t>control {k}</w:t></w:r></w:p></w:sdtContent></w:sdt>'))
    d.add_paragraph("after the rows")._p.addprevious(parse_xml(
        f'<w:sdt {nsdecls("w")} xmlns:w15="{W15}"><w:sdtPr><w:tag w:val="rows"/><w15:repeatingSection/></w:sdtPr>'
        '<w:sdtContent><w:sdt><w:sdtPr><w15:repeatingSectionItem/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>row</w:t>'
        '</w:r></w:p></w:sdtContent></w:sdt></w:sdtContent></w:sdt>'))
    for k in range(probes):
        d.add_table(rows=2, cols=2).cell(0, 0).text = f"alpha {k}"
    d.add_paragraph("end")
    d.save(path)
    return saved_by_word(lean_template(path))


def saved_by_word(path):
    """Give every paragraph, run and table row of the body the identifiers Word writes on them when it saves:
    w14:paraId and w14:textId, w:rsidR and w:rsidRDefault. A document Word or Google Docs saved has them on every
    element, and the library reads such paragraphs on other routes than the bare ones python-docx writes."""
    ids = itertools.count(0x1000)
    with zipfile.ZipFile(path) as z:
        parts = {name: z.read(name) for name in z.namelist()}
    xml = parts["word/document.xml"].decode()
    if "xmlns:w14=" not in xml:
        xml = xml.replace("<w:document ", f'<w:document xmlns:w14="{W14}" ', 1)
    xml = re.sub(r"<w:p(?=[ />])", lambda m: f'<w:p w14:paraId="{next(ids):08X}" w14:textId="77777777" '
                 'w:rsidR="006333BD" w:rsidRDefault="0044216F"', xml)
    xml = re.sub(r"<w:r(?=[ />])", '<w:r w:rsidR="006333BD"', xml)
    xml = re.sub(r"<w:tr(?=[ />])", lambda m: f'<w:tr w14:paraId="{next(ids):08X}" w14:textId="77777777" '
                 'w:rsidR="006333BD"', xml)
    parts["word/document.xml"] = xml.encode()
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    return path


def lean_template(path):
    """Drop from a python-docx document the 98 table styles of its template but Table Normal and Table Grid, and its
    stylesWithEffects part: 750 kB that would weigh more than the body."""
    with zipfile.ZipFile(path) as z:
        parts = {name: z.read(name) for name in z.namelist() if name != "word/stylesWithEffects.xml"}
    parts["word/styles.xml"] = re.sub(
        r'<w:style w:type="table"(?:(?!</w:style>).)*?w:styleId="(?!TableNormal"|TableGrid")[^"]*".*?</w:style>', "",
        parts["word/styles.xml"].decode(), flags=re.S).encode()
    for name, pattern in (("word/_rels/document.xml.rels", r"<Relationship [^>]*stylesWithEffects[^>]*/>"),
                          ("[Content_Types].xml", r"<Override [^>]*stylesWithEffects[^>]*/>")):
        parts[name] = re.sub(pattern, "", parts[name].decode()).encode()
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    return path

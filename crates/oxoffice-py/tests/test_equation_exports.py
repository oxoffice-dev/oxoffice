"""Document.to_html writes a paragraph's equations as MathML and Document.to_markdown as LaTeX, in their place."""

import oxoffice.docx as oxdocx


def test_equations_reach_html_and_markdown_in_their_place():
    doc = oxdocx.Document()
    paragraph = doc.add_paragraph("Let ")
    paragraph.add_equation("x=1", format="latex")
    doc.add_paragraph().add_equation(r"\frac{a}{b}", format="latex", display=True)

    html = doc.to_html()
    assert "Let <math" in html
    assert '<math display="block"' in html
    assert "<mfrac>" in html

    markdown = doc.to_markdown()
    assert "Let $x=1$" in markdown or "Let $x = 1$" in markdown
    assert r"$$\frac{a}{b}$$" in markdown


def test_equations_reach_rtf_and_odt_in_their_place():
    import io
    import zipfile

    doc = oxdocx.Document()
    paragraph = doc.add_paragraph("Let ")
    paragraph.add_equation("x=1", format="latex")
    paragraph.add_run(" hold.")

    rtf = doc.to_rtf()
    text = rtf.bytes.decode("ascii")
    assert r"{\mmath{\*\moMath " in text
    assert text.index("Let ") < text.index(r"\mmath") < text.index(" hold.")
    assert not [d for d in rtf.diagnostics if "equation" in d.message]

    odt = doc.to_odt()
    archive = zipfile.ZipFile(io.BytesIO(odt.bytes))
    content = archive.read("content.xml").decode()
    assert content.index("Let") < content.index('<draw:object xlink:href="./Object 1"') < content.index("hold.")
    assert archive.read("Object 1/content.xml").decode().count("<math ") == 1
    assert "application/vnd.oasis.opendocument.formula" in archive.read("META-INF/manifest.xml").decode()

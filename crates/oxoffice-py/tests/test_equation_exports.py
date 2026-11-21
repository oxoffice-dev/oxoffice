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

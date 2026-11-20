# oxoffice

Read, write, edit, compare and render Word and PowerPoint files from Python,
natively. No Microsoft Office, LibreOffice or conversion service is needed.

```sh
python -m pip install oxoffice
```

`oxoffice` requires CPython 3.10 or newer. One wheel per platform serves
every compatible Python version through the stable ABI.

## Word

```python
from oxoffice.docx import Document

doc = Document()
doc.add_paragraph("Hello from oxoffice.")
doc.save("hello.docx")
```

## PowerPoint

```python
from oxoffice.pptx import Presentation

deck = Presentation()
deck.save("deck.pptx")
```

## Command line

The wheel also installs the `oxoffice` command, the same command line as the
native binary:

```sh
oxoffice docx convert hello.docx --to pdf -o hello.pdf
oxoffice pptx text deck.pptx --json
python -m oxoffice --help
```

## License

MIT. oxoffice is a friendly fork of rdocx and rpptx by Atul Sharma
(tensorbee), see NOTICE.

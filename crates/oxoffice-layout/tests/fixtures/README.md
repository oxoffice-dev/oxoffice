# System fallback test fonts

Two subsets of Noto Sans CJK SC that stand in for the host's system fonts in
`tests/system_fallback.rs`, so the system-font fallback is tested the same way on
every machine. They are test data only and never bundled.

- `OxofficeFallbackTest.otf`, family `Oxoffice Fallback Test`: `猫犬山川`.
- `OxofficeNamedTest.otf`, family `Oxoffice Named Test`: `猫犬山川你好こABCabc`.

None of `猫犬山川` is in the bundled fonts; `你好` is (Noto Sans SC subset) and `こ` too (Noto Sans JP subset). Licence: SIL Open Font
License 1.1, see `LICENSE-NotoSansCJK`; the family names avoid the reserved
name "Source".

Source: the `Noto Sans CJK SC` face of `NotoSansCJK-Regular.ttc` from Debian's
`fonts-noto-cjk` 1:20240730 (SHA-256
`b76b0433203017ca80401b2ee0dd69350349871c4b19d504c34dbdd80541690a`).
Reproduce with FontTools:

```python
from fontTools import subset
from fontTools.ttLib import TTCollection, TTFont

face = next(f for f in TTCollection("NotoSansCJK-Regular.ttc").fonts
            if f["name"].getDebugName(1) == "Noto Sans CJK SC")
face.save("sc.otf")

def make(out, family, text):
    font = TTFont("sc.otf")
    options = subset.Options()
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.notdef_outline = True
    options.layout_features = []
    options.hinting = False
    options.desubroutinize = True
    subsetter = subset.Subsetter(options)
    subsetter.populate(text=text)
    subsetter.subset(font)
    postscript = family.replace(" ", "")
    for record in font["name"].names:
        if record.nameID in (1, 4, 16, 21):
            record.string = family
        elif record.nameID in (6, 20):
            record.string = postscript
        elif record.nameID == 3:
            record.string = postscript + "-fixture"
        elif record.nameID in (2, 17, 22):
            record.string = "Regular"
    cff = font["CFF "].cff
    cff.fontNames = [postscript]
    top = cff.topDictIndex[0]
    for key in ("FullName", "FamilyName"):
        if hasattr(top, key):
            setattr(top, key, family)
    font.save(out)

make("OxofficeFallbackTest.otf", "Oxoffice Fallback Test", "猫犬山川")
make("OxofficeNamedTest.otf", "Oxoffice Named Test", "猫犬山川你好こABCabc")
```

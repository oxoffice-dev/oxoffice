"""Length values used by the public oxoffice.pptx API."""


class Length(int):
    """An integer length measured in English Metric Units (EMU).

    Every EMU length argument takes a Length as given, or a bare int as EMU. A
    bare int that would round to zero hundredths of a point, where font sizes
    and spacing are stored, is refused, and so are bool and float, except that
    a float space_before or space_after is a line multiple. line_spacing and
    bullet_size, where a bare number is a multiple, are not EMU length
    arguments.
    """

    _EMUS_PER_INCH = 914400
    _EMUS_PER_PT = 12700

    @property
    def inches(self) -> float:
        return int(self) / self._EMUS_PER_INCH

    @property
    def pt(self) -> float:
        return int(self) / self._EMUS_PER_PT

    @property
    def emu(self) -> int:
        return int(self)


class Inches(Length):
    """A length constructed from inches."""

    def __new__(cls, inches: float) -> "Inches":
        return super().__new__(cls, int(inches * cls._EMUS_PER_INCH))


class Pt(Length):
    """A length constructed from points."""

    def __new__(cls, points: float) -> "Pt":
        return super().__new__(cls, int(points * cls._EMUS_PER_PT))


__all__ = ["Length", "Inches", "Pt"]

#!/usr/bin/env python3
"""Generate the checked-in function table of oxoffice-xlsx-formula.

    python3 tools/gen-xlsx-functions/generate.py MS-XLSX.docx MS-XLS.docx \
        > crates/oxoffice-xlsx-formula/src/functions/table.rs

The two documents are Microsoft's open specifications (learn.microsoft.com,
Office file formats); the table was written from MS-XLSX revision 31.0 of
2026-11-18 and MS-XLS of 2025-08-19:

    https://officeprotocoldocs-f5hpbjgea6b8gneq.b02.azurefd.net/files/MS-XLSX/%5bMS-XLSX%5d-261118.docx
    https://officeprotocoldocs-f5hpbjgea6b8gneq.b02.azurefd.net/files/MS-XLS/%5bMS-XLS%5d-250819.docx

From MS-XLSX section 2.2.2 come the worksheet functions
(`function-call`, `ref-function-call`, the future and worksheet-only calls),
each one's argument rule (`abs-params = "(" argument-expression ")"`), and
the future-function table that says which names are stored with `_xlfn.` or
`_xlfn._xlws.`. The minimum and maximum argument counts are read from the
rules. MS-XLS (the Ftab table, `if-params = val, *2(ref / val)`) is the cross
check: every function both list has the same minimum (the maximum differs
where xlsx allows 255 arguments and xls 30). OVERRIDES and ADDITIONS below
say what the documents get wrong or lack, and why.
"""

import re
import sys
import zipfile
import xml.etree.ElementTree as ET

W = '{http://schemas.openxmlformats.org/wordprocessingml/2006/main}'

# Rules MS-XLSX writes wrongly: the function's documented signature wins.
OVERRIDES = {
    # FILTER(array, include, [if_empty]); MS-XLSX asks for 3 to 4.
    'FILTER': (2, 3),
    # MAP(array1, lambda_or_array<#>), two arguments at least; MS-XLSX asks for 3.
    'MAP': (2, 253),
    # SWITCH(expression, value1, result1, ...); MS-XLSX asks for 4.
    'SWITCH': (3, 254),
    # CEILING.PRECISE(number, [significance]); MS-XLSX asks for 2.
    'CEILING.PRECISE': (1, 2),
    # FLOOR.PRECISE(number, [significance]); MS-XLSX asks for 2.
    'FLOOR.PRECISE': (1, 2),
    # SHEET([value]) and SHEETS([reference]): "(" argument ")", the one
    # argument optional.
    'SHEET': (0, 1),
    'SHEETS': (0, 1),
}

# Functions MS-XLSX does not list, with their stored prefix. XMATCH and
# ENCODEURL are in LibreOffice's OOXML table and XlsxWriter's future list;
# ANCHORARRAY, SINGLE, ARRAYTOTEXT, VALUETOTEXT and IMAGE in XlsxWriter's; PY
# is MS-XLSX's solo-function; the others are later Excel functions, stored as
# every function after Excel 2007 is.
ADDITIONS = {
    'ANCHORARRAY': ('_xlfn.', 1, 1),
    'ARRAYTOTEXT': ('_xlfn.', 1, 2),
    'DETECTLANGUAGE': ('_xlfn.', 1, 1),
    'ENCODEURL': ('_xlfn.', 1, 1),
    'GROUPBY': ('_xlfn.', 3, 8),
    'IMAGE': ('_xlfn.', 1, 5),
    'PERCENTOF': ('_xlfn.', 2, 2),
    'PIVOTBY': ('_xlfn.', 4, 11),
    'PY': ('_xlfn._xlws.', 2, 254),
    'REGEXEXTRACT': ('_xlfn.', 2, 4),
    'REGEXREPLACE': ('_xlfn.', 3, 5),
    'REGEXTEST': ('_xlfn.', 2, 3),
    'SINGLE': ('_xlfn.', 1, 1),
    'STOCKHISTORY': ('_xlfn.', 2, 11),
    'TRANSLATE': ('_xlfn.', 1, 3),
    'TRIMRANGE': ('_xlfn.', 1, 3),
    'VALUETOTEXT': ('_xlfn.', 1, 2),
    'XMATCH': ('_xlfn.', 2, 4),
}


def paragraphs(path):
    root = ET.fromstring(zipfile.ZipFile(path).read('word/document.xml'))
    lines = []
    for p in root.iter(W + 'p'):
        lines.append(''.join(
            (el.text or '') if el.tag == W + 't' else '\t' if el.tag == W + 'tab' else ''
            for el in p.iter()))
    return lines


def line_starting(lines, prefix):
    return next(line for line in lines if line.startswith(prefix))


# A tiny ABNF reader: the (minimum, maximum) number of arguments a rule takes.
SLOTS = {'argument', 'argument-expression', 'ref-argument-expression', 'parameter-name',
         'lambda-parameter-name', 'whole-number-part', 'guid', 'string-constant', 'val', 'ref'}
NOTHING = {'space', 'whitespace', 'emptyparams'}


def counts(rule):
    tokens = [t for t in re.findall(r'"[^"]*"|\d*\*\d*|[\[\]()/,]|[a-z0-9-]+', rule)
              if t != ',']

    def alternatives(i):
        found = []
        while True:
            low, high, i = sequence(i)
            found.append((low, high))
            if i < len(tokens) and tokens[i] == '/':
                i += 1
                continue
            return min(f[0] for f in found), max(f[1] for f in found), i

    def sequence(i):
        low = high = 0
        while i < len(tokens) and tokens[i] not in ('/', ')', ']'):
            a, b, i = element(i)
            low, high = low + a, high + b
        return low, high, i

    def element(i):
        repeat = None
        if re.fullmatch(r'\d*\*\d*', tokens[i]):
            at_least, at_most = tokens[i].split('*')
            repeat = (int(at_least or 0), int(at_most or 10**6))
            i += 1
        token = tokens[i]
        if token in ('(', '['):
            low, high, i = alternatives(i + 1)
            i += 1
            if token == '[':
                low = 0
        elif token.startswith('"') or token in NOTHING:
            low = high = 0
            i += 1
        elif token in SLOTS:
            low = high = 1
            i += 1
        else:
            raise SystemExit(f'unknown element {token!r} in {rule}')
        if repeat:
            low, high = low * repeat[0], high * repeat[1]
        return low, high, i

    low, high, i = alternatives(0)
    assert i == len(tokens), rule
    return low, high


def main():
    xlsx = paragraphs(sys.argv[1])
    xls = paragraphs(sys.argv[2])
    rules = {}
    for line in xlsx:
        found = re.match(r'^([a-z0-9.-]+-params) *= *(.*)$', line)
        if found:
            rules[found.group(1)] = found.group(2)
    calls = {}
    for start in ('ref-function-call =', 'function-call =', 'future-function-call =',
                  'worksheet-only-function-call ='):
        line = line_starting(xlsx, start).replace('\t', '').replace('—', '-')
        for name, rule in re.findall(r'"([A-Z0-9._]+)" ([a-z0-9-]+-params)', line):
            calls.setdefault(name, rule)

    # The stored prefix: the future-function table, then the names the
    # grammar puts in its "_xlfn." group but the table lacks.
    prefixes = {}
    table = xlsx.index('Future function')
    for line in xlsx[table + 1:]:
        if line.startswith('The function syntax'):
            break
        name = line.strip()
        for prefix in ('_xlfn._xlws.', '_xlfn.', ''):
            if name.startswith(prefix):
                prefixes[name[len(prefix):]] = prefix
                break
    group = line_starting(xlsx, 'future-function-list =').split(')) /')[0]
    for name in re.findall(r'"([A-Z0-9._]+)"', group):
        prefixes.setdefault(name, '_xlfn.')

    # The cross check with MS-XLS.
    xls_rules = {}
    for i, line in enumerate(xls):
        if re.fullmatch(r'0x[0-9A-F]{4}', line.strip()):
            for follow in xls[i + 2:i + 6]:
                if 'params =' in follow:
                    xls_rules[xls[i + 1].strip()] = follow.split('=', 1)[1].strip()
                    break

    functions = {}
    for name, rule in calls.items():
        low, high = counts(rules[rule])
        # A future name in Ftab is a macro function of the same name (SORT).
        if name in xls_rules and name not in prefixes:
            xls_low, _ = counts(xls_rules[name])
            if xls_low != low and name not in OVERRIDES:
                raise SystemExit(f'{name}: MS-XLSX minimum {low}, MS-XLS {xls_low}')
        functions[name] = (prefixes.get(name, ''), *OVERRIDES.get(name, (low, high)))
    for name, entry in ADDITIONS.items():
        assert name not in functions, name
        functions[name] = entry
    for name in prefixes:
        assert name in functions, f'{name} has a prefix but no rule'

    print('// Written by tools/gen-xlsx-functions/generate.py; see its docstring. Do not edit by hand.')
    print()
    print('use super::{Function, Prefix};')
    print()
    print('pub(super) static FUNCTIONS: &[Function] = &[')
    kinds = {'': 'Prefix::None', '_xlfn.': 'Prefix::Xlfn', '_xlfn._xlws.': 'Prefix::XlfnXlws'}
    for name in sorted(functions):
        prefix, low, high = functions[name]
        print(f'    Function::new("{name}", {kinds[prefix]}, {low}, {high}),')
    print('];')


main()

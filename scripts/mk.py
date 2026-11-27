import sys, zipfile
base, out, body = sys.argv[1], sys.argv[2], sys.argv[3]
zin = zipfile.ZipFile(base)
zout = zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED)
for it in zin.infolist():
    data = zin.read(it.filename)
    if it.filename == 'word/document.xml':
        s = data.decode()
        a = s.index('<w:body>') + len('<w:body>')
        b = s.index('<w:sectPr')
        s = s[:a] + body + s[b:]
        data = s.encode()
    zout.writestr(it, data)
zout.close()

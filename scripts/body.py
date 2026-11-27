import sys, zipfile, re
for f in sys.argv[1:]:
    z = zipfile.ZipFile(f)
    s = z.read('word/document.xml').decode()
    b = s[s.find('<w:body>'):s.find('<w:sectPr')]
    b = re.sub(r' w14:(paraId|textId)="[^"]*"', '', b)
    b = re.sub(r' w:rsid\w*="[^"]*"', '', b)
    print('==', f); print(b)

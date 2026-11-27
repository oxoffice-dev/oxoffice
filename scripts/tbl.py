import sys, zipfile, re
for f in sys.argv[1:]:
    s = zipfile.ZipFile(f).read('word/document.xml').decode()
    b = s[s.find('<w:tbl>'):s.find('</w:tbl>')+8]
    b = re.sub(r' w14:(paraId|textId)="[^"]*"', '', b)
    b = re.sub(r' w:rsid\w*="[^"]*"', '', b)
    b = re.sub(r'<w:tcBorders>.*?</w:tcBorders>', '', b)
    b = re.sub(r'<w:tblBorders>.*?</w:tblBorders>', '<w:tblBorders/>', b)
    print('==', f); print(b)

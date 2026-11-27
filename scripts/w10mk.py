import sys, zipfile, re
src, out, variant = sys.argv[1], sys.argv[2], sys.argv[3]
z = zipfile.ZipFile(src)
parts = {n: z.read(n) for n in z.namelist()}
def g(n): return parts[n].decode()
REF = '<w:r><w:rPr><w:rStyle w:val="Marquedecommentaire"/></w:rPr><w:commentReference w:id="%d"/></w:r>'
# replies: (id, paraId, durableId, parentParaId, done, text)
root = ('11DEBDA3', '4C66E30E')
if variant == 'root-done-only':
    replies = [(1, '22AAAA01', '5A5A0001', '11DEBDA3', '0', 'Reply one')]
    root_done = '1'
elif variant == 'reply-to-reply':
    replies = [(1, '22AAAA01', '5A5A0001', '11DEBDA3', '0', 'Reply one'),
               (2, '22AAAA02', '5A5A0002', '22AAAA01', '0', 'Reply to reply one')]
    root_done = '0'
doc = g('word/document.xml')
starts = ''.join('<w:commentRangeStart w:id="%d"/>' % r[0] for r in replies)
ends = ''.join('<w:commentRangeEnd w:id="%d"/>' % r[0] + REF % r[0] for r in replies)
doc = doc.replace('<w:commentRangeStart w:id="0"/>', '<w:commentRangeStart w:id="0"/>' + starts)
i = doc.index('<w:commentReference w:id="0"/></w:r>') + len('<w:commentReference w:id="0"/></w:r>')
doc = doc[:i] + ends + doc[i:]
parts['word/document.xml'] = doc.encode()
com = g('word/comments.xml')
m = re.search(r'<w:comment w:id="0".*?</w:comment>', com, re.S).group(0)
add = ''
for (cid, pid, did, par, done, txt) in replies:
    c = m.replace('w:id="0"', 'w:id="%d"' % cid).replace('11DEBDA3', pid).replace('Root comment', txt)
    c = re.sub(r'w14:textId="[^"]*"', 'w14:textId="77777777"', c)
    add += c
parts['word/comments.xml'] = com.replace('</w:comments>', add + '</w:comments>').encode()
ex = g('word/commentsExtended.xml').replace('w15:paraId="11DEBDA3" w15:done="0"', 'w15:paraId="11DEBDA3" w15:done="%s"' % root_done)
ex = ex.replace('</w15:commentsEx>', ''.join('<w15:commentEx w15:paraId="%s" w15:paraIdParent="%s" w15:done="%s"/>' % (r[1], r[3], r[4]) for r in replies) + '</w15:commentsEx>')
parts['word/commentsExtended.xml'] = ex.encode()
ids = g('word/commentsIds.xml').replace('</w16cid:commentsIds>', ''.join('<w16cid:commentId w16cid:paraId="%s" w16cid:durableId="%s"/>' % (r[1], r[2]) for r in replies) + '</w16cid:commentsIds>')
parts['word/commentsIds.xml'] = ids.encode()
cx = g('word/commentsExtensible.xml').replace('</w16cex:commentsExtensible>', ''.join('<w16cex:commentExtensible w16cex:durableId="%s" w16cex:dateUtc="2026-11-27T17:55:00Z"/>' % r[2] for r in replies) + '</w16cex:commentsExtensible>')
parts['word/commentsExtensible.xml'] = cx.encode()
zo = zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED)
for it in z.infolist():
    zo.writestr(it, parts[it.filename])
zo.close()

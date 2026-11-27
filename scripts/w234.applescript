on run argv
  set base to item 1 of argv
  set which to item 2 of argv
  set L to load script POSIX file (base & "/lib.scpt")
  set info to ""
  tell application "Microsoft Word"
    with timeout of 90 seconds
      if which is "w2" then
        open (POSIX file (base & "/w2/w2-input.docx"))
        set d to active document
        set track revisions of d to true
        set info to "before: " & L's revs(d)
        set p to L's posOf(d, "three")
        select (L's rng(d, p, p))
        type text selection text "MAC "
        set p to L's posOf(d, "four")
        select (L's rng(d, p, p + 4))
        type backspace selection
        set info to info & "after: " & L's revs(d)
        save as d file name (base & "/w2/w2-output.docx") file format format document
        close active document saving no
      else if which is "w3" then
        set d to make new document
        insert text "Hello world." at text object of d
        set track revisions of d to true
        set p to L's posOf(d, ".")
        select (L's rng(d, p, p))
        type text selection text " alpha beta gamma"
        set p to L's posOf(d, "beta")
        set bold of font object of (L's rng(d, p, p + 4)) to true
        set info to "same session: " & L's revs(d)
        save as d file name (base & "/w3/w3-same-session.docx") file format format document
        close active document saving no
      else if which is "w3b" then
        open (POSIX file (base & "/w3/w3-same-session.docx"))
        set d to active document
        set track revisions of d to true
        set p to L's posOf(d, "gamma")
        set bold of font object of (L's rng(d, p, p + 5)) to true
        set info to "reopened, bold gamma: " & L's revs(d)
        save as d file name (base & "/w3/w3-reopened.docx") file format format document
        close active document saving no
      else if which is "w4" then
        open (POSIX file (base & "/w4/w4-input.docx"))
        set d to active document
        set track revisions of d to true
        set info to "before: " & L's revs(d)
        set p to L's posOf(d, "target")
        set italic of font object of (L's rng(d, p, p + 6)) to true
        set info to info & "after: " & L's revs(d)
        save as d file name (base & "/w4/w4-other-bold-then-mac-italic.docx") file format format document
        close active document saving no
      else if which is "w4s1" then
        set d to make new document
        insert text "The target word." at text object of d
        set track revisions of d to true
        set p to L's posOf(d, "target")
        set bold of font object of (L's rng(d, p, p + 6)) to true
        set info to L's revs(d)
        save as d file name (base & "/w4/w4-same-author-step1-bold.docx") file format format document
        close active document saving no
      else if which is "w4s2" then
        open (POSIX file (base & "/w4/w4-same-author-step1-bold.docx"))
        set d to active document
        set track revisions of d to true
        set p to L's posOf(d, "target")
        set italic of font object of (L's rng(d, p, p + 6)) to true
        set info to L's revs(d)
        save as d file name (base & "/w4/w4-same-author-step2-italic.docx") file format format document
        close active document saving no
      end if
    end timeout
  end tell
  return info
end run

on run argv
  set base to item 1 of argv
  set L to load script POSIX file (base & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      open (POSIX file (base & "/w1/w1-step1.docx"))
      set d to active document
      set track revisions of d to true
      set info to "before: " & L's revs(d)
      set p to L's posOf(d, "beta")
      select (L's rng(d, p, p))
      type text selection text "XYZ "
      set p to L's posOf(d, "gamma")
      select (L's rng(d, p, p + 5))
      type backspace selection
      set info to info & "after: " & L's revs(d) & "text=[" & (content of text object of d) & "]"
      save as d file name (base & "/w1/w1-step2.docx") file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

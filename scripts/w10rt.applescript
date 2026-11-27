on run argv
  set src to item 1 of argv
  set outp to item 2 of argv
  set L to load script POSIX file (item 3 of argv & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      open (POSIX file src)
      set d to active document
      set n0 to count of Word comments of d
      set p to L's posOf(d, "text.")
      select (L's rng(d, p, p + 4))
      make new Word comment at d with properties {comment text:"Second root added by Word"}
      set info to "comments before=" & n0 & " after=" & (count of Word comments of d)
      save as d file name outp file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

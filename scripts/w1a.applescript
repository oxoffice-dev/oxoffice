on run argv
  set L to load script POSIX file ((item 1 of argv) & "/lib.scpt")
  set target to (item 1 of argv) & "/w1/w1-step1.docx"
  tell L to setUser("Mac", "MC")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      set d to make new document
      set track revisions of d to false
      insert text "Base sentence one. End." at text object of d
      set track revisions of d to true
      set p to L's posOf(d, " End.")
      select (L's rng(d, p, p))
      type text selection text " alpha beta gamma"
      set info to L's revs(d)
      save as d file name target file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

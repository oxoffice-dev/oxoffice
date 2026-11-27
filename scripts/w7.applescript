on run argv
  set base to item 1 of argv
  set mode to item 2 of argv
  set L to load script POSIX file (base & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      set d to make new document
      insert text "P1 first moved." & return & "P2 second moved." & return & "P3 stays." & return & "P4 stays." & return & "P5 last." at text object of d
      set track revisions of d to true
      set s to start of content of text object of paragraph 1 of d
      set e to end of content of text object of paragraph 2 of d
      set r to L's rng(d, s, e)
      if mode is "relocate" then
        set view type of view of active window to outline view
        relocate r direction relocate down
        set view type of view of active window to print view
      else
        select r
        cut object selection
        set p to start of content of text object of paragraph 3 of d
        select (L's rng(d, p, p))
        paste object selection
      end if
      set info to L's revs(d) & "text=[" & (content of text object of d) & "]"
      save as d file name (base & "/w7/w7-" & mode & ".docx") file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

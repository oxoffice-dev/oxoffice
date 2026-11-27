on run argv
  set base to item 1 of argv
  set mode to item 2 of argv
  set L to load script POSIX file (base & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      set d to make new document
      insert text "The notice period is 30 days." & return & "Payment within 30 days of invoice." & return & "Last paragraph to delete." at text object of d
      if mode is "replace" then save as d file name (base & "/w12/w12-input.docx") file format format document
      set d to active document
      set track revisions of d to true
      if mode is "replace" then
        execute find (find object of text object of d) find text "30 days" replace with "45 days" replace replace all
      else if mode is "dellast-whole" then
        set r to text object of paragraph 3 of d
        select r
        type backspace selection
      else if mode is "dellast-frommark" then
        set s to end of content of text object of paragraph 2 of d
        select (L's rng(d, s - 1, end of content of text object of d))
        type backspace selection
      end if
      set info to L's revs(d) & "text=[" & (content of text object of d) & "]"
      save as d file name (base & "/w12/w12-" & mode & ".docx") file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

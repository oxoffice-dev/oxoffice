on run argv
  set base to item 1 of argv
  set mode to item 2 of argv
  set L to load script POSIX file (base & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      if mode is "base" then
        set d to make new document
        insert text "Before table." & return & "After table." at text object of d
        set p to start of content of text object of paragraph 2 of d
        set t to make new table at d with properties {text object:(L's rng(d, p, p)), number of rows:3, number of columns:3}
        repeat with r from 1 to 3
          repeat with c from 1 to 3
            set content of text object of (get cell from table t row r column c) to ("r" & r & "c" & c)
          end repeat
        end repeat
        save as d file name (base & "/w8/w8-input.docx") file format format document
        close active document saving no
        return "base ok"
      end if
      open (POSIX file (base & "/w8/w8-input.docx"))
      set d to active document
      set track revisions of d to true
      set t to table 1 of d
      if mode is "rowheight" then
        set height rule of row 1 of t to row height exactly
        set height of row 1 of t to 40
      else if mode is "shading" then
        set background pattern color index of shading of (get cell from table t row 2 column 2) to yellow
      else if mode is "inscol" then
        select text object of (get cell from table t row 1 column 2)
        insert columns (selection)
      else if mode is "delcol" then
        delete column 2 of t
      else if mode is "merge" then
        merge cell (get cell from table t row 1 column 1) with (get cell from table t row 1 column 2)
      else if mode is "insrow" then
        select text object of (get cell from table t row 2 column 1)
        insert rows selection
      else if mode is "delrow" then
        delete row 2 of t
      end if
      set info to L's revs(d)
      save as d file name (base & "/w8/w8-" & mode & ".docx") file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

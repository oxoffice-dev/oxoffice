on run argv
  set base to item 1 of argv
  set L to load script POSIX file (base & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      set d to make new document
      insert text "Para one list." & return & "Para two style." & return & "Para three both." at text object of d
      save as d file name (base & "/w5/w5-input.docx") file format format document
      set d to active document
      set track revisions of d to true
      apply number default (list format of text object of paragraph 1 of d)
      set style of text object of paragraph 2 of d to style heading1
      set style of text object of paragraph 3 of d to style heading1
      apply bullet default (list format of text object of paragraph 3 of d)
      set info to L's revs(d)
      save as d file name (base & "/w5/w5-output.docx") file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

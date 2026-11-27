on run argv
  set src to item 1 of argv
  set pre to item 2 of argv
  set L to load script POSIX file (item 3 of argv & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      open (POSIX file src)
      set d to active document
      set info to "open: " & L's revs(d) & linefeed
      accept all revisions d
      set info to info & "accept all text=[" & (content of text object of d) & "] revs=" & (count of revisions of d) & linefeed
      save as d file name (pre & "-accepted.docx") file format format document
      close active document saving no
      open (POSIX file src)
      set d to active document
      reject all revisions d
      set info to info & "reject all text=[" & (content of text object of d) & "] revs=" & (count of revisions of d) & linefeed
      save as d file name (pre & "-rejected.docx") file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

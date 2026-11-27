on cinfo(d)
  set out to ""
  tell application "Microsoft Word"
    set n to count of Word comments of d
    set out to "comments=" & n & " revisions=" & (count of revisions of d)
    repeat with i from 1 to n
      set c to Word comment i of d
      set out to out & " | #" & i & " by " & (author of c) & " on [" & (content of scope of c) & "] text [" & (content of comment text of c) & "]"
    end repeat
  end tell
  return out
end cinfo
on run argv
  set base to item 1 of argv
  set mode to item 2 of argv
  set L to load script POSIX file (base & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      if mode is "w9make" then
        set d to make new document
        insert text "Keep this obsolete word here." at text object of d
        set track revisions of d to true
        set p to L's posOf(d, "here")
        select (L's rng(d, p, p))
        type text selection text "INSERTED "
        set p to L's posOf(d, "obsolete")
        select (L's rng(d, p, p + 8))
        type backspace selection
        set track revisions of d to false
        repeat with i from 1 to (count of revisions of d)
          set rv to revision i of d
          if revision type of rv is revision insert then
            select (text object of rv)
            make new Word comment at d with properties {comment text:"Comment on inserted text"}
          end if
        end repeat
        repeat with i from 1 to (count of revisions of d)
          set rv to revision i of d
          if revision type of rv is revision delete then
            select (text object of rv)
            make new Word comment at d with properties {comment text:"Comment on deleted text"}
          end if
        end repeat
        set info to my cinfo(d)
        save as d file name (base & "/w9/w9-word-made.docx") file format format document
        close active document saving no
        return info
      else if mode is "w10" then
        set d to make new document
        insert text "Comment target text." at text object of d
        set p to L's posOf(d, "target")
        select (L's rng(d, p, p + 6))
        make new Word comment at d with properties {comment text:"Root comment"}
        set info to my cinfo(d)
        save as d file name (base & "/w10/w10-word-root-comment.docx") file format format document
        close active document saving no
        return info
      else
        -- accept / reject a given file: argv 3 = src, argv 4 = out prefix
        set src to item 3 of argv
        set pre to item 4 of argv
        open (POSIX file src)
        set d to active document
        set info to "open: " & my cinfo(d) & linefeed
        accept all revisions d
        set info to info & "accept all: " & my cinfo(d) & linefeed
        save as d file name (pre & "-accepted.docx") file format format document
        close active document saving no
        open (POSIX file src)
        set d to active document
        reject all revisions d
        set info to info & "reject all: " & my cinfo(d) & linefeed
        save as d file name (pre & "-rejected.docx") file format format document
        close active document saving no
        return info
      end if
    end timeout
  end tell
end run

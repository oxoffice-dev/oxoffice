on run argv
  set base to item 1 of argv
  set mode to item 2 of argv
  set L to load script POSIX file (base & "/lib.scpt")
  tell application "Microsoft Word"
    with timeout of 90 seconds
      if mode is "input" then
        set d to make new document
        insert text "Intro" & return & "Body text one." & return & "Second" & return & "Body text two." at text object of d
        set style of text object of paragraph 1 of d to style heading1
        set style of text object of paragraph 3 of d to style heading1
        set p to start of content of text object of paragraph 1 of d
        insert text "Contents" & return at (L's rng(d, p, p))
        set style of text object of paragraph 1 of d to style normal
        set p to end of content of text object of paragraph 1 of d
        set rr to L's rng(d, p, p)
        set toc to make new table of contents at d with properties {text object:rr, upper heading level:1, lower heading level:3, use heading styles:true}
        set info to "tocs=" & (count of tables of contents of d) & " text=[" & (content of text object of d) & "]"
        save as d file name (base & "/w13/w13-input.docx") file format format document
        close active document saving no
        return info
      end if
      open (POSIX file (base & "/w13/w13-input.docx"))
      set d to active document
      set track revisions of d to true
      set e to end of content of text object of d
      insert text return & "New section" at (L's rng(d, e - 1, e - 1))
      set n to count of paragraphs of d
      set style of text object of paragraph n of d to style heading1
      update (table of contents 1 of d)
      set info to L's revs(d) & "text=[" & (content of text object of d) & "]"
      save as d file name (base & "/w13/w13-updated-tracked.docx") file format format document
      close active document saving no
    end timeout
  end tell
  return info
end run

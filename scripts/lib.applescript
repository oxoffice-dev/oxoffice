on setUser(nm, ini)
  tell application "Microsoft Word"
    set user name to nm
    set user initials to ini
  end tell
end setUser
on rng(d, s, e)
  tell application "Microsoft Word"
    return create range d start s end e
  end tell
end rng
on posOf(d, needle)
  tell application "Microsoft Word"
    set t to content of text object of d
  end tell
  return (offset of needle in t) - 1
end posOf
on revs(d)
  set out to ""
  tell application "Microsoft Word"
    set n to count of revisions of d
    repeat with i from 1 to n
      set r to revision i of d
      set out to out & "  rev" & i & ": " & (revision type of r as string) & " by " & (author of r) & " [" & (content of text object of r) & "]" & linefeed
    end repeat
  end tell
  return "revisions=" & n & linefeed & out
end revs

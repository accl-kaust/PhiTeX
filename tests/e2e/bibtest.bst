% BibTeX builtins and style-file errors, compared with bibtex by xtask e2e
% a test style

ENTRY { author title note } { cnt } { lab }

INTEGERS { i n }

STRINGS { s g }

MACRO {jan} {"January"}

MACRO {jan} {"again"}

FUNCTION {show} { duplicate$ write$ newline$ }

FUNCTION {names}
{ author num.names$ 'n :=
  #1 'i :=
  { i n #1 + < }
  { author i "{vv~}{ll}{, jj}{, f.}" format.name$ show pop$
    author i "{ff }{vv }{ll}{ jj}" format.name$ show pop$
    author i "{f.~}{l}" format.name$ show pop$
    author i "{ll{}}{f{}}{v{-}}" format.name$ show pop$
    i #1 + 'i := }
  while$
  author #99 "{ll}" format.name$ show pop$
}

FUNCTION {cases}
{ title "t" change.case$ show pop$
  title "l" change.case$ show pop$
  title "u" change.case$ show pop$
  title "x" change.case$ show pop$
  title purify$ show pop$
  title #5 text.prefix$ show pop$
  title text.length$ int.to.str$ show pop$
  title width$ int.to.str$ show pop$
  title #3 #4 substring$ show pop$
  title #-3 #4 substring$ show pop$
  title #2 #-1 substring$ show pop$
  title add.period$ show pop$
  note empty$ int.to.str$ write$ newline$
  note missing$ int.to.str$ write$ newline$
  title "A" * "" * s * 'g :=
  g show pop$
  title 'lab :=
  lab #2 #500 substring$ 'lab :=
  cite$ 'lab :=
}

FUNCTION {article} { names cases "x" chr.to.int$ int.to.str$ show pop$ #300 int.to.chr$ show pop$ }

FUNCTION {book} { article }

FUNCTION {misc} { #1 #2 "a" * pop$ pop$ undefinedfn pop$ }

FUNCTION {default.type} { misc }

FUNCTION {recurse} { recurse }


FUNCTION {bad2} { #12x skip$ }

READ

FUNCTION {presort} { title purify$ "l" change.case$ #1 entry.max$ substring$ 'sort.key$ := }

ITERATE {presort}

SORT

FUNCTION {begin} { preamble$ write$ newline$ quote$ write$ "stack" #1 stack$ "a" #2 top$ top$ newline$ }

EXECUTE {begin}

ITERATE {call.type$}

REVERSE {cite$}

EXECUTE {cite$}

ITERATE {missing$}

EXECUTE {sort.key$}

FUNCTION {long} { "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbb" write$ "x y z w" write$ newline$ }

EXECUTE {long}

bogus command

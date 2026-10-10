//! TeX's help messages (tex.web §79: `help0`…`help6`), by the error's
//! message: an error whose help depends only on its message finds it
//! here (the longest prefix of the message that has an entry); one whose
//! help depends on where it happened gives its own (`Exec::error`).

/// (the message, or its beginning; the help lines in reading order)
const HELPS: &[(&str, &[&str])] = &[
    ("A <box> was supposed to be here", &[
        r"I was expecting to see \hbox or \vbox or \copy or \box or",
        r"something like that. So you might find something missing in",
        r"your output. But keep trying; you can fix this later.",
    ]),
    ("Ambiguous; you need another { and }", &[
        r"I'm ignoring this fraction specification, since I don't",
        r"know whether a construction like `x \over y \over z'",
        r"means `{x \over y} \over z' or `x \over {y \over z}'.",
    ]),
    ("Argument of ", &[
        r"I've run across a `}' that doesn't seem to match anything.",
        r"For example, `\def\a#1{...}' and `\a}' would produce",
        r"this error. If you simply proceed now, the `\par' that",
        r"I've just inserted will cause me to report a runaway",
        r"argument that might be the root of the problem. But if",
        r"your `}' was spurious, just type `2' and it will go away.",
    ]),
    ("Arithmetic overflow", &[
        r"I can't carry out that multiplication or division,",
        r"since the result is out of range.",
    ]),
    ("Bad \\patterns", &[r"(See Appendix H.)"]),
    ("Bad \\prevgraf", &[r"I allow only nonnegative values here."]),
    ("Bad space factor", &[r"I allow only values in the range 1..32767 here."]),
    ("Bad character code", &[r"A character number must be between 0 and 255.", r"I changed this one to zero."]),
    ("Bad delimiter code", &[
        r"A numeric delimiter code must be between 0 and 2^{27}-1.",
        r"I changed this one to zero.",
    ]),
    ("Bad mathchar", &[r"A mathchar number must be between 0 and 32767.", r"I changed this one to zero."]),
    ("Bad number", &[r"Since I expected to read a number between 0 and 15,", r"I changed this one to zero."]),
    ("Bad register code", &[r"A register number must be between 0 and 255.", r"I changed this one to zero."]),
    ("Dimension too large", &[
        r"I can't work with sizes bigger than about 19 feet.",
        r"Continue and I'll use the largest value I can.",
    ]),
    ("Display math should end with $$", &[
        r"The `$' that I just saw supposedly matches a previous `$$'.",
        r"So I shall assume that you typed `$$' both times.",
    ]),
    ("Double subscript", &[r"I treat `x_1_2' essentially like `x_1{}_2'."]),
    ("Double superscript", &[r"I treat `x^1^2' essentially like `x^1{}^2'."]),
    ("Duplicate pattern", &[r"(See Appendix H.)"]),
    ("Extra alignment tab has been changed to ", &[
        r"You have given more \span or & marks than there were",
        r"in the preamble to the \halign or \valign now in progress.",
        r"So I'll assume that you meant to type \cr instead.",
    ]),
    ("Extra \\or", &[r"I'm ignoring this; it doesn't match any \if."]),
    ("Extra \\fi", &[r"I'm ignoring this; it doesn't match any \if."]),
    ("Extra \\else", &[r"I'm ignoring this; it doesn't match any \if."]),
    ("Extra \\endcsname", &[r"I'm ignoring this, since I wasn't doing a \csname."]),
    ("Extra \\right", &[r"I'm ignoring a \right that had no matching \left."]),
    ("Extra }, or forgotten ", &[
        r"I've deleted a group-closing symbol because it seems to be",
        r"spurious, as in `$x}$'. But perhaps the } is legitimate and",
        r"you forgot something else, as in `\hbox{$x}'. In such cases",
        r"the way to recover is to insert both the forgotten and the",
        r"deleted material, e.g., by typing `I$}'.",
    ]),
    ("Extra ", &[r"Things are pretty mixed up, but I think the worst is over."]),
    ("File ended within ", &[r"This \read has unbalanced braces."]),
    ("Forbidden control sequence found", &[
        r"I suspect you have forgotten a `}', causing me",
        r"to read past where you wanted me to stop.",
        r"I'll try to recover; but if the error is serious,",
        r"you'd better type `E' or `X' now and fix your file.",
    ]),
    ("Huge page cannot be shipped out", &[
        r"The page just created is more than 18 feet tall or",
        r"more than 18 feet wide, so I suspect something went wrong.",
    ]),
    ("Illegal magnification has been changed to 1000", &[r"The magnification ratio must be between 1 and 32768."]),
    ("Illegal math \\discretionary", &[
        r"Sorry: The third part of a discretionary break must be",
        r"empty, in math formulas. I had to delete your third part.",
    ]),
    ("Illegal parameter number in definition of ", &[
        r"You meant to type ## instead of #, right?",
        r"Or maybe a } was forgotten somewhere earlier, and things",
        r"are all screwed up? I'm going to assume that you meant ##.",
    ]),
    ("Illegal unit of measure (pt inserted)", &[
        r"Dimensions can be in units of em, ex, in, pt, pc,",
        r"cm, mm, dd, cc, bp, or sp; but yours is a new one!",
        r"I'll assume that you meant to say pt, for printer's points.",
        r"To recover gracefully from this error, it's best to",
        r"delete the erroneous units; e.g., type `2' to delete",
        r"two letters. (See Chapter 27 of The TeXbook.)",
    ]),
    ("Illegal unit of measure (replaced by filll)", &[r"I dddon't go any higher than filll."]),
    ("Illegal unit of measure (mu inserted)", &[
        r"The unit of measurement in math glue must be mu.",
        r"To recover gracefully from this error, it's best to",
        r"delete the erroneous units; e.g., type `2' to delete",
        r"two letters. (See Chapter 27 of The TeXbook.)",
    ]),
    ("Improper alphabetic constant", &[
        r"A one-character control sequence belongs after a ` mark.",
        r"So I'm essentially inserting \0 here.",
    ]),
    ("Improper `at' size (", &[
        r"I can only handle fonts at positive sizes that are",
        r"less than 2048pt, so I've changed what you said to 10pt.",
    ]),
    ("Improper discretionary list", &[r"Discretionary lists must contain only boxes and kerns."]),
    ("Improper \\halign", &[
        r"Displays can use special alignments (like \eqalignno)",
        r"only if nothing but the alignment itself is between $$'s.",
        r"So I've deleted the formulas that preceded this alignment.",
    ]),
    ("Improper \\valign", &[
        r"Displays can use special alignments (like \eqalignno)",
        r"only if nothing but the alignment itself is between $$'s.",
        r"So I've deleted the formulas that preceded this alignment.",
    ]),
    ("Improper \\hyphenation", &[
        r"Hyphenation exceptions must contain only letters",
        r"and hyphens. But continue; I'll forgive and forget.",
    ]),
    ("Improper \\setbox", &[
        r"Sorry, \setbox is not allowed after \halign in a display,",
        r"or between \accent and an accented character.",
    ]),
    ("Improper \\spacefactor", &[
        r"You can refer to \spacefactor only in horizontal mode;",
        r"you can refer to \prevdepth only in vertical mode; and",
        r"neither of these is meaningful inside \write. So",
        r"I'm forgetting what you said and using zero instead.",
    ]),
    ("Improper \\prevdepth", &[
        r"You can refer to \spacefactor only in horizontal mode;",
        r"you can refer to \prevdepth only in vertical mode; and",
        r"neither of these is meaningful inside \write. So",
        r"I'm forgetting what you said and using zero instead.",
    ]),
    ("Incompatible glue units", &[r"I'm going to assume that 1mu=1pt when they're mixed."]),
    ("Incompatible list can't be unboxed", &[
        r"Sorry, Pandora. (You sneaky devil.)",
        r"I refuse to unbox an \hbox in vertical mode or vice versa.",
        r"And I can't open any boxes in math mode.",
    ]),
    ("Incompatible magnification (", &[
        r"I can handle only one magnification ratio per job. So I've",
        r"reverted to the magnification you used earlier on this run.",
    ]),
    ("Incomplete ", &[
        r"A forbidden control sequence occurred in skipped text.",
        r"This kind of error happens when you say `\if...' and forget",
        r"the matching `\fi'. I've inserted a `\fi'; this might work.",
    ]),
    ("Infinite glue shrinkage found in a paragraph", &[
        r"The paragraph just ended includes some glue that has",
        r"infinite shrinkability, e.g., `\hskip 0pt minus 1fil'.",
        r"Such glue doesn't belong there---it allows a paragraph",
        r"of any length to fit on one line. But it's safe to proceed,",
        r"since the offensive shrinkability has been made finite.",
    ]),
    ("Infinite glue shrinkage found in box being split", &[
        r"The box you are \vsplitting contains some infinitely",
        r"shrinkable glue, e.g., `\vss' or `\vskip 0pt minus 1fil'.",
        r"Such glue doesn't belong there; but you can safely proceed,",
        r"since the offensive shrinkability has been made finite.",
    ]),
    ("Infinite glue shrinkage found on current page", &[
        r"The page about to be output contains some infinitely",
        r"shrinkable glue, e.g., `\vss' or `\vskip 0pt minus 1fil'.",
        r"Such glue doesn't belong there; but you can safely proceed,",
        r"since the offensive shrinkability has been made finite.",
    ]),
    ("Infinite glue shrinkage inserted from ", &[
        r"The correction glue for page breaking with insertions",
        r"must have finite shrinkability. But you may proceed,",
        r"since the offensive shrinkability has been made finite.",
    ]),
    ("Insertions can only be added to a vbox", &[
        r"Tut tut: You're trying to \insert into a",
        r"\box register that now contains an \hbox.",
        r"Proceed, and I'll discard its present contents.",
    ]),
    ("Leaders not followed by proper glue", &[
        r"You should say `\leaders <box or rule><hskip or vskip>'.",
        r"I found the <box or rule>, but there's no suitable",
        r"<hskip or vskip>, so I'm ignoring these leaders.",
    ]),
    ("Limit controls must follow a math operator", &[r"I'm ignoring this misplaced \limits or \nolimits command."]),
    ("Math formula deleted: Insufficient extension fonts", &[
        r"Sorry, but I can't typeset math unless \textfont 3",
        r"and \scriptfont 3 and \scriptscriptfont 3 have all",
        r"the \fontdimen values needed in math extension fonts.",
    ]),
    ("Math formula deleted: Insufficient symbol fonts", &[
        r"Sorry, but I can't typeset math unless \textfont 2",
        r"and \scriptfont 2 and \scriptscriptfont 2 have all",
        r"the \fontdimen values needed in math symbol fonts.",
    ]),
    ("Misplaced \\noalign", &[
        r"I expect to see \noalign only after the \cr of",
        r"an alignment. Proceed, and I'll ignore this case.",
    ]),
    ("Misplaced \\omit", &[
        r"I expect to see \omit only after tab marks or the \cr of",
        r"an alignment. Proceed, and I'll ignore this case.",
    ]),
    ("Missing $$ inserted", &[
        r"Displays can use special alignments (like \eqalignno)",
        r"only if nothing but the alignment itself is between $$'s.",
    ]),
    ("Missing $ inserted", &[
        r"I've inserted a begin-math/end-math symbol since I think",
        r"you left one out. Proceed, with fingers crossed.",
    ]),
    ("Missing control sequence inserted", &[
        r"Please don't say `\def cs{...}', say `\def\cs{...}'.",
        r"I've inserted an inaccessible control sequence so that your",
        r"definition will be completed without mixing me up too badly.",
        r"You can recover graciously from this error, if you're",
        r"careful; see exercise 27.2 in The TeXbook.",
    ]),
    ("Missing delimiter (. inserted)", &[
        r"I was expecting to see something like `(' or `\{' or",
        r"`\}' here. If you typed, e.g., `{' instead of `\{', you",
        r"should probably delete the `{' by typing `1' now, so that",
        r"braces don't get unbalanced. Otherwise just proceed.",
        r"Acceptable delimiters are characters whose \delcode is",
        r"nonnegative, or you can use `\delimiter <delimiter code>'.",
    ]),
    ("Missing font identifier", &[
        r"I was looking for a control sequence whose",
        r"current meaning has been defined by \font.",
    ]),
    ("Missing \\cr inserted", &[r"I'm guessing that you meant to end an alignment here."]),
    ("Missing { inserted", &[
        r"A left brace was mandatory here, so I've put one in.",
        r"You might want to delete and/or insert some corrections",
        r"so that I will find a matching right brace soon.",
        r"(If you're confused by all this, try typing `I}' now.)",
    ]),
    ("Missing = inserted for ", &[r"I was expecting to see `<', `=', or `>'. Didn't."]),
    ("Missing # inserted in alignment preamble", &[
        r"There should be exactly one # between &'s, when an",
        r"\halign or \valign is being set up. In this case you had",
        r"none, so I've put one in; maybe that will work.",
    ]),
    ("Missing number, treated as zero", &[
        r"A number should have been here; I inserted `0'.",
        r"(If you can't figure out why I needed to see a number,",
        r"look up `weird error' in the index to The TeXbook.)",
    ]),
    ("Missing \\endcsname inserted", &[
        r"The control sequence marked <to be read again> should",
        r"not appear between \csname and \endcsname.",
    ]),
    ("Missing `to' inserted", &[
        r"I'm working on `\vsplit<box number> to <dimen>';",
        r"will look for the <dimen> next.",
    ]),
    ("Missing ", &[
        r"I've inserted something that you may have forgotten.",
        r"(See the <inserted text> above.)",
        r"With luck, this will get me unwedged. But if you",
        r"really didn't forget anything, try typing `2' now; then",
        r"my insertion and my current dilemma will both disappear.",
    ]),
    ("Nonletter", &[r"(See Appendix H.)"]),
    ("Not a letter", &[
        r"Letters in \hyphenation words must have \lccode>0.",
        r"Proceed; I'll ignore the character I just read.",
    ]),
    ("Number too big", &[
        r#"I can only go up to 2147483647='17777777777="7FFFFFFF,"#,
        r"so I'm using that number instead of yours.",
    ]),
    ("Only one # is allowed per tab", &[
        r"There should be exactly one # between &'s, when an",
        r"\halign or \valign is being set up. In this case you had",
        r"more than one, so I'm ignoring all but the first.",
    ]),
    ("Output loop---", &[
        r"I've concluded that your \output is awry; it never does a",
        r"\shipout, so I'm shipping \box255 out myself. Next time",
        r"increase \maxdeadcycles if you want me to be more patient!",
    ]),
    ("Output routine didn't use all of ", &[
        r"Your \output commands should empty \box255,",
        r"e.g., by saying `\shipout\box255'.",
        r"Proceed; I'll discard its present contents.",
    ]),
    ("Paragraph ended before ", &[
        r"I suspect you've forgotten a `}', causing me to apply this",
        r"control sequence to too much text. How can we recover?",
        r"My plan is to forget the whole thing and hope for the best.",
    ]),
    ("Parameters must be numbered consecutively", &[
        r"I've inserted the digit you should have used after the #.",
        r"Type `1' to delete what you did use.",
    ]),
    ("Please use ", &[
        r"I'm changing \accent to \mathaccent here; wish me luck.",
        r"(Accents are not the same in formulas as they are in text.)",
    ]),
    ("TeX capacity exceeded, sorry [", &[
        r"If you really absolutely need more capacity,",
        r"you can ask a wizard to enlarge me.",
    ]),
    ("Text line contains an invalid character", &[
        r"A funny symbol that I can't read has just been input.",
        r"Continue, and I'll forget that it ever happened.",
    ]),
    ("Too late for ", &[r"All patterns must be given before typesetting begins."]),
    ("Too many }'s", &[
        r"You've closed more groups than you opened.",
        r"Such booboos are generally harmless, so keep going.",
    ]),
    ("Unbalanced output routine", &[
        r"Your sneaky output routine has problematic {'s and/or }'s.",
        r"I can't handle that very well; good luck.",
    ]),
    ("Unbalanced write command", &[
        r"On this page there's a \write with fewer real {'s than }'s.",
        r"I can't handle that very well; good luck.",
    ]),
    ("Undefined control sequence", &[
        r"The control sequence at the end of the top line",
        r"of your error message was never \def'ed. If you have",
        r"misspelled it (e.g., `\hobx'), type `I' and the correct",
        r"spelling (e.g., `I\hbox'). Otherwise just continue,",
        r"and I'll forget about whatever was undefined.",
    ]),
    ("Use of ", &[
        r"If you say, e.g., `\def\a1{...}', then you must always",
        r"put `1' after `\a', since control sequence names are",
        r"made up of letters only. The macro here has not been",
        r"followed by the required stuff, so I'm ignoring it.",
    ]),
    ("You already have nine parameters", &[
        r"I'm going to ignore the # sign you just used,",
        r"as well as the token that followed it.",
    ]),
    ("You can't dump inside a group", &[r"`{...\dump}' is a no-no."]),
    ("You can't use a prefix with `", &[r"I'll pretend you didn't say \long or \outer or \global."]),
    ("You can't use `\\hrule' here except with leaders", &[
        r"To put a horizontal rule in an hbox or an alignment,",
        r"you should use \leaders or \hrulefill (see The TeXbook).",
    ]),
    ("You can't use `", &[
        r"Sorry, but I'm not programmed to handle this case;",
        r"I'll just pretend that you didn't ask for it.",
        r"If you're in the wrong mode, you might be able to",
        r"return to the right one by typing `I}' or `I$' or `I\par'.",
    ]),
    ("\\vsplit needs a \\vbox", &[
        r"The box you are trying to split is an \hbox.",
        r"I can't split such a box, so I'll leave it alone.",
    ]),
];

/// The help of an error whose help is its message's.
pub(super) fn help_for(msg: &str) -> &'static [&'static str] {
    let mut best: Option<(usize, &'static [&'static str])> = None;
    for &(p, h) in HELPS {
        if msg.starts_with(p) && best.is_none_or(|(n, _)| p.len() > n) {
            best = Some((p.len(), h));
        }
    }
    best.map_or(&[], |(_, h)| h)
}

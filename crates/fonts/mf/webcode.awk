BEGIN{sec=0; intext=1}
/^@[ *]/ { sec++; intext=1; t=$0; if ($0 ~ /^@\*/) { print "@@@@ [" sec "] " substr(t,1,70) } else print "@@ [" sec "]"; if ($0 ~ /@>=/ || $0 ~ /^@ @<[^@]*$/ || $0 ~ /^@ @[dp]/) { intext=0; print } ; next }
/^@d|^@p|^@<.*@>=|^@f/ { intext=0 }
/^@ @<.*@>=/ { intext=0 }
/^@<[^@]*$/ { intext=0 }
/^@ @<[^@]*$/ { intext=0 }
{ if (!intext && $0 !~ /^@\^|^@\.|^@:/ && $0 != "") print }

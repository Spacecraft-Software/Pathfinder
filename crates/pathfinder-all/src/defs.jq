# Pathfinder: jq 1.8.1's `format`, a C builtin: an @-format by name
# (M20, P-067). Here, because `@csv` and `@tsv` are `jaq_fmts`'s.
def format($f): if $f == "text" then @text elif $f == "json" then @json elif $f == "csv" then @csv elif $f == "tsv" then @tsv elif $f == "html" then @html elif $f == "uri" then @uri elif $f == "urid" then @urid elif $f == "sh" then @sh elif $f == "base64" then @base64 elif $f == "base64d" then @base64d else error(if $f >= "" and $f < [] then $f else $f | _jq_quoted end + " is not a valid format") end;

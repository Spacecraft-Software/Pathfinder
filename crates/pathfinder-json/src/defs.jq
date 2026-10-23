# Conversion
def _totype(p; e): if p then . else fromjson | if p then . else e end end;
# Pathfinder: `tonumber` and `toboolean` are native, as jq 1.8.1's.

# Arrays
def transpose: [range([.[] | length] | max) as $i | [.[][$i]]];

# Indexing
def in(xs)    : . as $x | xs | has     ($x);
def inside(xs): . as $x | xs | contains($x);
def  index($i): indices($i)[ 0];
def rindex($i): indices($i)[-1];

# Formatting
def @json: tojson;

# Pathfinder: jq 1.8.1's `delpaths` (`jv_delpaths`, native) and its `del`,
# which deletes the paths together.
def delpaths($paths): _delpaths($paths);
def del(f): delpaths([path(f)]);
# jq's `setpath` is a C builtin: its last argument runs outermost.
def setpath(p; x): x as $x | p as $p | _setpath($p; $x);

# Pathfinder: jq 1.8.1's own definitions (src/builtin.jq) of names jaq lacks
# or defines differently (M20, T-027).
def tostream: path(def r: (.[]?|r), .; r) as $p | getpath($p) | reduce path(.[]?) as $q ([$p, .]; [$p+$q]);
def fromstream(i): {x: null, e: false} as $init | foreach i as $i ($init; if .e then $init else . end | if $i|length == 2 then setpath(["e"]; $i[0]|length==0) | setpath(["x"]+$i[0]; $i[1]) else setpath(["e"]; $i[0]|length==1) end; if .e then .x else empty end);
def truncate_stream(stream): . as $n | null | stream | . as $input | if (.[0]|length) > $n then setpath([0];$input[0][$n:]) else empty end;
def IN(s): any(s == .; .);
def IN(src; s): any(s == src; .);
def INDEX(stream; idx_expr): reduce stream as $row ({}; .[$row|idx_expr|tostring] = $row);
def INDEX(idx_expr): INDEX(.[]; idx_expr);
def JOIN($idx; idx_expr): [.[] | [., $idx[idx_expr]]];
def JOIN($idx; stream; idx_expr): stream | [., $idx[idx_expr]];
def JOIN($idx; stream; idx_expr; join_expr): stream | [., $idx[idx_expr]] | join_expr;
def trimstr($val): ltrimstr($val)|rtrimstr($val);
def have_decnum: true;
def have_literal_numbers: true;
def skip($n; expr): if $n > 0 then foreach expr as $item ($n; . - 1; if . < 0 then $item else empty end) elif $n == 0 then expr else error("skip doesn't support negative count") end;
def limit($n; expr): if $n > 0 then label $out | foreach expr as $item ($n; . - 1; $item, if . <= 0 then break $out else empty end) elif $n == 0 then empty else error("limit doesn't support negative count") end;
def nth($n; g): if $n < 0 then error("nth doesn't support negative indices") else first(skip($n; g)) end;
def _flatten($x): reduce .[] as $i ([]; if $i | type == "array" and $x != 0 then . + ($i | _flatten($x - 1)) else . + [$i] end);
def flatten($x): if $x < 0 then error("flatten depth must not be negative") else _flatten($x) end;
def flatten: _flatten(-1);
def join($x): reduce .[] as $i (null; (if . == null then "" else . + $x end) + ($i | if type == "boolean" or type == "number" then tostring else . // "" end)) // "";
def pick(pathexps): . as $in | reduce path(pathexps) as $a (null; setpath($a; $in | getpath($a)));
def from_entries: map({(.key // .Key // .name // .Name): (if has("value") then .value else .Value end)}) | add | .//={};
def with_entries(f): to_entries | map(f) | from_entries;

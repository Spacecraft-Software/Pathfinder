# Conversion
def totype(p; e): if p then . else fromjson | if p then . else e end end;
def tonumber : totype(isnumber ; error("cannot parse as number" ));
def toboolean: totype(isboolean; error("cannot parse as boolean"));

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
